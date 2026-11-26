//! JNI bridge for Android (`com.emucam.nativebridge.EmuNative`).
//!
//! Control plane passes JSON strings (catalog, recipes, preview params) so new
//! presets/params never need binding changes. The develop path runs on a Rust
//! thread; Kotlin polls progress/status via an opaque handle.
//! Every entry point is wrapped in `catch_unwind` → Java RuntimeException.

use emucore::{develop_jpeg, preview, resolve, Catalog, DevelopOptions, Hooks, Recipe};
use jni::objects::{JByteArray, JByteBuffer, JClass, JString};
use jni::sys::{jbyteArray, jfloat, jint, jlong, jstring};
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};

// ------------------------------------------------------------- catalog state

static CATALOG: RwLock<Option<Catalog>> = RwLock::new(None);

fn catalog() -> Catalog {
    if let Some(c) = CATALOG.read().unwrap().as_ref() {
        return c.clone();
    }
    Catalog::builtin().clone()
}

// ------------------------------------------------------------------ helpers

fn throw(env: &mut JNIEnv, msg: &str) {
    let _ = env.throw_new("java/lang/RuntimeException", msg);
}

/// Run `f`, converting `Err` and panics into a Java exception + default value.
fn guard<'l, T: Default>(env: &mut JNIEnv<'l>, f: impl FnOnce(&mut JNIEnv<'l>) -> Result<T, String>) -> T {
    match catch_unwind(AssertUnwindSafe(|| f(env))) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            throw(env, &e);
            T::default()
        }
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            throw(env, &format!("emucore panic: {msg}"));
            T::default()
        }
    }
}

fn jstr(env: &mut JNIEnv, s: &JString) -> Result<String, String> {
    env.get_string(s).map(|s| s.into()).map_err(|e| e.to_string())
}

fn new_jstring(env: &mut JNIEnv, s: &str) -> Result<jstring, String> {
    env.new_string(s).map(|s| s.into_raw()).map_err(|e| e.to_string())
}

fn recipe(env: &mut JNIEnv, s: &JString) -> Result<Recipe, String> {
    Recipe::from_json(&jstr(env, s)?).map_err(|e| e.to_string())
}

// ------------------------------------------------------------ control plane

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_version<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>) -> jstring {
    guard(&mut env, |env| new_jstring(env, emucore::VERSION))
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_catalogJson<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>) -> jstring {
    guard(&mut env, |env| new_jstring(env, &catalog().to_ui_json()))
}

/// Load an external preset pack directory (films/, lenses/, bodies/).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_loadPack<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, path: JString<'l>) {
    guard(&mut env, |env| {
        let p = jstr(env, &path)?;
        let mut c = catalog();
        c.load_pack(std::path::Path::new(&p)).map_err(|e| e.to_string())?;
        *CATALOG.write().unwrap() = Some(c);
        Ok(())
    })
}

/// Normalize a recipe (fill defaults, migrate) and return its JSON.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_normalizeRecipe<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, json: JString<'l>) -> jstring {
    guard(&mut env, |env| {
        let r = recipe(env, &json)?;
        resolve(&r, &catalog()).map_err(|e| e.to_string())?;
        new_jstring(env, &r.to_json())
    })
}

/// Preview shader parameters (uniforms + defines) for a recipe.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_previewParamsJson<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, json: JString<'l>) -> jstring {
    guard(&mut env, |env| {
        let r = recipe(env, &json)?;
        let res = resolve(&r, &catalog()).map_err(|e| e.to_string())?;
        let p = serde_json::to_string(&preview::params(&res)).map_err(|e| e.to_string())?;
        new_jstring(env, &p)
    })
}

/// Bake develop + print LUTs (RGBA16F, 33³) into two direct ByteBuffers.
/// Returns LUT size, or 0 on failure.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_bakeLuts<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    json: JString<'l>,
    develop: JByteBuffer<'l>,
    print: JByteBuffer<'l>,
) -> jint {
    guard(&mut env, |env| {
        let r = recipe(env, &json)?;
        let res = resolve(&r, &catalog()).map_err(|e| e.to_string())?;
        let (d, p) = preview::luts_f16(&res);
        for (buf, data) in [(&develop, &d), (&print, &p)] {
            let ptr = env.get_direct_buffer_address(buf).map_err(|e| e.to_string())?;
            let cap = env.get_direct_buffer_capacity(buf).map_err(|e| e.to_string())?;
            if cap < data.len() {
                return Err(format!("LUT buffer too small: {cap} < {}", data.len()));
            }
            // SAFETY: direct buffer of at least data.len() bytes, owned by the JVM for this call.
            unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()) };
        }
        Ok(emucore::lut::PREVIEW_LUT_SIZE as jint)
    })
}

// ------------------------------------------------------------- develop jobs

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const FAILED: u8 = 2;
const CANCELLED: u8 = 3;

struct Job {
    progress: AtomicU32,
    status: AtomicU8,
    cancel: AtomicBool,
    result: Mutex<Option<Vec<u8>>>,
    error: Mutex<String>,
}

fn job<'a>(h: jlong) -> Option<&'a Job> {
    if h == 0 {
        None
    } else {
        // SAFETY: handle came from Arc::into_raw in developStart and is kept
        // alive until developFree.
        Some(unsafe { &*(h as *const Job) })
    }
}

/// Start developing `input` (JPEG bytes) with `recipe`. Returns a handle.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developStart<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    input: JByteArray<'l>,
    json: JString<'l>,
    max_dim: jint,
    quality: jint,
) -> jlong {
    guard(&mut env, |env| {
        let bytes = env.convert_byte_array(&input).map_err(|e| e.to_string())?;
        let r = recipe(env, &json)?;
        let cat = catalog();
        let job = Arc::new(Job {
            progress: AtomicU32::new(0f32.to_bits()),
            status: AtomicU8::new(RUNNING),
            cancel: AtomicBool::new(false),
            result: Mutex::new(None),
            error: Mutex::new(String::new()),
        });
        let worker = job.clone();
        let opts = DevelopOptions {
            max_dim: if max_dim > 0 { Some(max_dim as usize) } else { None },
            jpeg_quality: quality.clamp(50, 100) as u8,
        };
        std::thread::Builder::new()
            .name("emucore-develop".into())
            .spawn(move || {
                let w2 = worker.clone();
                let w3 = worker.clone();
                let progress = move |p: f32| w2.progress.store(p.to_bits(), Ordering::Relaxed);
                let cancelled = move || w3.cancel.load(Ordering::Relaxed);
                let hooks = Hooks { progress: &progress, cancelled: &cancelled };
                let out = catch_unwind(AssertUnwindSafe(|| develop_jpeg(&bytes, &r, &cat, &opts, &hooks)));
                match out {
                    Ok(Ok(v)) => {
                        *worker.result.lock().unwrap() = Some(v);
                        worker.status.store(DONE, Ordering::Release);
                    }
                    Ok(Err(emucore::Error::Cancelled)) => worker.status.store(CANCELLED, Ordering::Release),
                    Ok(Err(e)) => {
                        *worker.error.lock().unwrap() = e.to_string();
                        worker.status.store(FAILED, Ordering::Release);
                    }
                    Err(_) => {
                        *worker.error.lock().unwrap() = "panic during develop".into();
                        worker.status.store(FAILED, Ordering::Release);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Arc::into_raw(job) as jlong)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developProgress(_env: JNIEnv, _c: JClass, h: jlong) -> jfloat {
    job(h).map(|j| f32::from_bits(j.progress.load(Ordering::Relaxed))).unwrap_or(0.0)
}

/// 0 running, 1 done, 2 failed, 3 cancelled.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developStatus(_env: JNIEnv, _c: JClass, h: jlong) -> jint {
    job(h).map(|j| j.status.load(Ordering::Acquire) as jint).unwrap_or(FAILED as jint)
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developResult<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, h: jlong) -> jbyteArray {
    guard(&mut env, |env| {
        let j = job(h).ok_or("null handle")?;
        let data = j.result.lock().unwrap().take().ok_or("no result (not done or already taken)")?;
        env.byte_array_from_slice(&data).map(|a| a.into_raw()).map_err(|e| e.to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developError<'l>(mut env: JNIEnv<'l>, _c: JClass<'l>, h: jlong) -> jstring {
    guard(&mut env, |env| {
        let msg = job(h).map(|j| j.error.lock().unwrap().clone()).unwrap_or_default();
        new_jstring(env, &msg)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developCancel(_env: JNIEnv, _c: JClass, h: jlong) {
    if let Some(j) = job(h) {
        j.cancel.store(true, Ordering::Relaxed);
    }
}

/// Release the handle. Safe while the worker still runs (it holds its own Arc).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developFree(_env: JNIEnv, _c: JClass, h: jlong) {
    if h != 0 {
        // SAFETY: matches Arc::into_raw in developStart; called once by Kotlin.
        unsafe { drop(Arc::from_raw(h as *const Job)) };
    }
}
