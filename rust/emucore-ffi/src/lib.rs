//! JNI bridge for Android (`com.emucam.nativebridge.EmuNative`).
//!
//! Control plane passes JSON strings (catalog, recipes, preview params) so new
//! presets/params never need binding changes. The develop path runs on a Rust
//! thread; Kotlin polls progress/status via an opaque handle.
//! Every entry point is wrapped in `catch_unwind` → Java RuntimeException.

use emucore::depth::{nearness, DepthKind, ModelLayout};
use emucore::{
    depth_input_jpeg, develop_jpeg, preview, resolve, Catalog, DevelopOptions, Hooks, RawDepth,
    RawSeg, Recipe,
};
use jni::objects::{JByteArray, JByteBuffer, JClass, JFloatArray, JString};
use jni::sys::{jboolean, jbyteArray, jfloat, jfloatArray, jint, jlong, jstring};
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
fn guard<'l, T: Default>(
    env: &mut JNIEnv<'l>,
    f: impl FnOnce(&mut JNIEnv<'l>) -> Result<T, String>,
) -> T {
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
    env.get_string(s)
        .map(|s| s.into())
        .map_err(|e| e.to_string())
}

fn new_jstring(env: &mut JNIEnv, s: &str) -> Result<jstring, String> {
    env.new_string(s)
        .map(|s| s.into_raw())
        .map_err(|e| e.to_string())
}

fn recipe(env: &mut JNIEnv, s: &JString) -> Result<Recipe, String> {
    Recipe::from_json(&jstr(env, s)?).map_err(|e| e.to_string())
}

// ------------------------------------------------------------ control plane

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_version<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
) -> jstring {
    guard(&mut env, |env| new_jstring(env, emucore::VERSION))
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_catalogJson<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
) -> jstring {
    guard(&mut env, |env| new_jstring(env, &catalog().to_ui_json()))
}

/// Load an external preset pack directory (films/, lenses/, bodies/).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_loadPack<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    path: JString<'l>,
) {
    guard(&mut env, |env| {
        let p = jstr(env, &path)?;
        let mut c = catalog();
        c.load_pack(std::path::Path::new(&p))
            .map_err(|e| e.to_string())?;
        *CATALOG.write().unwrap() = Some(c);
        Ok(())
    })
}

/// Normalize a recipe (fill defaults, migrate) and return its JSON.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_normalizeRecipe<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    json: JString<'l>,
) -> jstring {
    guard(&mut env, |env| {
        let r = recipe(env, &json)?;
        resolve(&r, &catalog()).map_err(|e| e.to_string())?;
        new_jstring(env, &r.to_json())
    })
}

/// Preview shader parameters (uniforms + defines) for a recipe.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_previewParamsJson<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    json: JString<'l>,
) -> jstring {
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
            let ptr = env
                .get_direct_buffer_address(buf)
                .map_err(|e| e.to_string())?;
            let cap = env
                .get_direct_buffer_capacity(buf)
                .map_err(|e| e.to_string())?;
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

fn develop_options(
    max_dim: jint,
    quality: jint,
    depth: Option<RawDepth>,
    veg: Option<RawSeg>,
) -> DevelopOptions {
    DevelopOptions {
        max_dim: if max_dim > 0 {
            Some(max_dim as usize)
        } else {
            None
        },
        jpeg_quality: quality.clamp(50, 100) as u8,
        depth,
        veg,
    }
}

/// Spawn the develop thread; returns the job handle.
fn spawn_job(bytes: Vec<u8>, r: Recipe, opts: DevelopOptions) -> Result<jlong, String> {
    let cat = catalog();
    let job = Arc::new(Job {
        progress: AtomicU32::new(0f32.to_bits()),
        status: AtomicU8::new(RUNNING),
        cancel: AtomicBool::new(false),
        result: Mutex::new(None),
        error: Mutex::new(String::new()),
    });
    let worker = job.clone();
    std::thread::Builder::new()
        .name("emucore-develop".into())
        .spawn(move || {
            let w2 = worker.clone();
            let w3 = worker.clone();
            let progress = move |p: f32| w2.progress.store(p.to_bits(), Ordering::Relaxed);
            let cancelled = move || w3.cancel.load(Ordering::Relaxed);
            let hooks = Hooks {
                progress: &progress,
                cancelled: &cancelled,
            };
            let out = catch_unwind(AssertUnwindSafe(|| {
                develop_jpeg(&bytes, &r, &cat, &opts, &hooks)
            }));
            match out {
                Ok(Ok(v)) => {
                    *worker.result.lock().unwrap() = Some(v);
                    worker.status.store(DONE, Ordering::Release);
                }
                Ok(Err(emucore::Error::Cancelled)) => {
                    worker.status.store(CANCELLED, Ordering::Release)
                }
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
        spawn_job(bytes, r, develop_options(max_dim, quality, None, None))
    })
}

fn depth_kind(kind: jint) -> Result<DepthKind, String> {
    match kind {
        0 => Ok(DepthKind::Depth),
        1 => Ok(DepthKind::InverseDepth),
        k => Err(format!("unknown depth kind {k}")),
    }
}

/// Like `developStart`, plus the raw depth-network output for this photo
/// (`depth` may be null; `kind`: 0 = depth, 1 = inverse depth).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developStartDepth<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    input: JByteArray<'l>,
    json: JString<'l>,
    max_dim: jint,
    quality: jint,
    depth: JFloatArray<'l>,
    dw: jint,
    dh: jint,
    kind: jint,
) -> jlong {
    guard(&mut env, |env| {
        let bytes = env.convert_byte_array(&input).map_err(|e| e.to_string())?;
        let r = recipe(env, &json)?;
        let raw = raw_depth(env, &depth, dw, dh, kind)?;
        spawn_job(bytes, r, develop_options(max_dim, quality, raw, None))
    })
}

fn raw_depth(
    env: &mut JNIEnv,
    depth: &JFloatArray,
    dw: jint,
    dh: jint,
    kind: jint,
) -> Result<Option<RawDepth>, String> {
    if depth.is_null() {
        return Ok(None);
    }
    Ok(Some(RawDepth {
        w: dw.max(0) as usize,
        h: dh.max(0) as usize,
        data: float_array(env, depth)?,
        kind: depth_kind(kind)?,
    }))
}

/// Like `developStartDepth`, plus raw segmentation logits (`veg`, NCHW,
/// `vegClasses` planes of `vegW`×`vegH`; may be null) for infrared films.
/// Negative dims become 0, which makes the map unusable (colour-only).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developStartAux<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    input: JByteArray<'l>,
    json: JString<'l>,
    max_dim: jint,
    quality: jint,
    depth: JFloatArray<'l>,
    dw: jint,
    dh: jint,
    kind: jint,
    veg: JFloatArray<'l>,
    veg_classes: jint,
    veg_w: jint,
    veg_h: jint,
) -> jlong {
    guard(&mut env, |env| {
        let bytes = env.convert_byte_array(&input).map_err(|e| e.to_string())?;
        let r = recipe(env, &json)?;
        let raw = raw_depth(env, &depth, dw, dh, kind)?;
        let seg = if veg.is_null() {
            None
        } else {
            Some(RawSeg {
                classes: veg_classes.max(0) as usize,
                w: veg_w.max(0) as usize,
                h: veg_h.max(0) as usize,
                data: float_array(env, &veg)?,
            })
        };
        spawn_job(bytes, r, develop_options(max_dim, quality, raw, seg))
    })
}

fn float_array(env: &mut JNIEnv, a: &JFloatArray) -> Result<Vec<f32>, String> {
    let n = env.get_array_length(a).map_err(|e| e.to_string())? as usize;
    let mut v = vec![0.0f32; n];
    env.get_float_array_region(a, 0, &mut v)
        .map_err(|e| e.to_string())?;
    Ok(v)
}

fn new_float_array(env: &mut JNIEnv, v: &[f32]) -> Result<jfloatArray, String> {
    let len = i32::try_from(v.len()).map_err(|_| "array too large")?;
    let out = env.new_float_array(len).map_err(|e| e.to_string())?;
    env.set_float_array_region(&out, 0, v)
        .map_err(|e| e.to_string())?;
    Ok(out.into_raw())
}

/// Whether developing with `recipe` would use a depth map (skip inference if not).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_wantsDepth<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    json: JString<'l>,
) -> jboolean {
    guard(&mut env, |env| {
        let r = recipe(env, &json)?;
        let res = resolve(&r, &catalog()).map_err(|e| e.to_string())?;
        Ok(emucore::pipeline::wants_depth(&res) as jboolean)
    })
}

/// Whether developing with `recipe` would use a vegetation map (infrared
/// films only; skip segmentation inference if not).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_wantsVeg<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    json: JString<'l>,
) -> jboolean {
    guard(&mut env, |env| {
        let r = recipe(env, &json)?;
        let res = resolve(&r, &catalog()).map_err(|e| e.to_string())?;
        Ok(emucore::pipeline::wants_veg(&res) as jboolean)
    })
}

/// Depth-network input tensor (ImageNet-normalised) for a JPEG, in the
/// network's portrait layout `w`×`h` (landscape photos are rotated).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_depthInput<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    input: JByteArray<'l>,
    w: jint,
    h: jint,
    nchw: jboolean,
) -> jfloatArray {
    guard(&mut env, |env| {
        if w <= 0 || h <= 0 {
            return Err(format!("bad layout {w}x{h}"));
        }
        let bytes = env.convert_byte_array(&input).map_err(|e| e.to_string())?;
        let layout = ModelLayout {
            w: w as usize,
            h: h as usize,
            nchw: nchw != 0,
        };
        let t = depth_input_jpeg(&bytes, layout).map_err(|e| e.to_string())?;
        new_float_array(env, &t)
    })
}

/// Normalise a portrait-oriented raw depth output to nearness (0 far … 1
/// near) for the live preview. Returns null when the output is unusable.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_nearness<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    raw: JFloatArray<'l>,
    w: jint,
    h: jint,
    kind: jint,
) -> jfloatArray {
    guard(&mut env, |env| {
        let data = float_array(env, &raw)?;
        let (w, h) = (w.max(0) as usize, h.max(0) as usize);
        match nearness(&data, w, h, depth_kind(kind)?, w, h) {
            Some(m) => new_float_array(env, &m.data),
            None => Ok(std::ptr::null_mut()),
        }
    })
}

/// Raw segmentation logits (`classes` NCHW planes of `w`×`h`) → per-pixel
/// vegetation probability (0..1, `w`×`h`) for the live infrared preview.
/// Returns null when the output is unusable (see `seg::vegetation`).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_vegetation<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    raw: JFloatArray<'l>,
    classes: jint,
    w: jint,
    h: jint,
) -> jfloatArray {
    guard(&mut env, |env| {
        let data = float_array(env, &raw)?;
        let (classes, w, h) = (
            classes.max(0) as usize,
            w.max(0) as usize,
            h.max(0) as usize,
        );
        match emucore::seg::vegetation(&data, classes, w, h) {
            Some(m) => new_float_array(env, &m.data),
            None => Ok(std::ptr::null_mut()),
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developProgress(
    _env: JNIEnv,
    _c: JClass,
    h: jlong,
) -> jfloat {
    job(h)
        .map(|j| f32::from_bits(j.progress.load(Ordering::Relaxed)))
        .unwrap_or(0.0)
}

/// 0 running, 1 done, 2 failed, 3 cancelled.
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developStatus(
    _env: JNIEnv,
    _c: JClass,
    h: jlong,
) -> jint {
    job(h)
        .map(|j| j.status.load(Ordering::Acquire) as jint)
        .unwrap_or(FAILED as jint)
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developResult<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    h: jlong,
) -> jbyteArray {
    guard(&mut env, |env| {
        let j = job(h).ok_or("null handle")?;
        let data = j
            .result
            .lock()
            .unwrap()
            .take()
            .ok_or("no result (not done or already taken)")?;
        env.byte_array_from_slice(&data)
            .map(|a| a.into_raw())
            .map_err(|e| e.to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developError<'l>(
    mut env: JNIEnv<'l>,
    _c: JClass<'l>,
    h: jlong,
) -> jstring {
    guard(&mut env, |env| {
        let msg = job(h)
            .map(|j| j.error.lock().unwrap().clone())
            .unwrap_or_default();
        new_jstring(env, &msg)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developCancel(
    _env: JNIEnv,
    _c: JClass,
    h: jlong,
) {
    if let Some(j) = job(h) {
        j.cancel.store(true, Ordering::Relaxed);
    }
}

/// Release the handle. Safe while the worker still runs (it holds its own Arc).
#[no_mangle]
pub extern "system" fn Java_com_emucam_nativebridge_EmuNative_developFree(
    _env: JNIEnv,
    _c: JClass,
    h: jlong,
) {
    if h != 0 {
        // SAFETY: matches Arc::into_raw in developStart; called once by Kotlin.
        unsafe { drop(Arc::from_raw(h as *const Job)) };
    }
}
