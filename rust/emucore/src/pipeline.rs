//! Full-quality render pipeline (stage order is physical):
//!
//! lens: lateral CA → bokeh → glow/veil → vignette → optical effects
//! film: exposure → exposure effects (leaks) → IR → halation → develop (tone)
//!       → grain → emulsion effects → print → print effects

use crate::color::linear_to_srgb;
use crate::effects::{self, Effect, FxCtx};
use crate::film::{grain, halation, infrared, ToneXform};
use crate::image::ImageF32;
use crate::lens::{self, bokeh, Geometry};
use crate::preset::{FilmKind, Slot};
use crate::recipe::{effect_seed, Resolved};
use crate::Error;

/// Progress / cancellation hooks (FFI polls an atomic; CLI prints).
pub struct Hooks<'a> {
    pub progress: &'a (dyn Fn(f32) + Sync),
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl Default for Hooks<'_> {
    fn default() -> Self {
        Hooks { progress: &|_| {}, cancelled: &|| false }
    }
}

fn check(h: &Hooks, p: f32) -> Result<(), Error> {
    if std::env::var_os("EMUCAM_PROF").is_some() {
        use std::sync::Mutex;
        static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        let mut l = LAST.lock().unwrap();
        if let Some(t) = *l {
            eprintln!("stage → {p:.2}: {:.2?}", t.elapsed());
        }
        *l = Some(std::time::Instant::now());
    }
    if (h.cancelled)() {
        return Err(Error::Cancelled);
    }
    (h.progress)(p);
    Ok(())
}

/// Render a linear-light image into a display-encoded (sRGB, 0..1) image.
pub fn render(src: &ImageF32, res: &Resolved, hooks: &Hooks) -> Result<ImageF32, Error> {
    let o = &res.optics;
    let g = Geometry::new(src.w, src.h, res.focus_center, res.focus_angle, o.field);
    let film = res.film.as_ref();

    // Instantiate effects with their seeds, grouped by slot.
    let fx: Vec<(Box<dyn Effect>, u64)> = res
        .effects
        .iter()
        .enumerate()
        .filter_map(|(i, e)| effects::build(e).map(|b| (b, effect_seed(e, res.seed, i))))
        .collect();
    let negative = film.map(|f| matches!(f.kind, FilmKind::ColorNegative | FilmKind::BwNegative | FilmKind::BwInfrared)).unwrap_or(false);
    let mono = film.map(|f| matches!(f.kind, FilmKind::BwNegative | FilmKind::BwInfrared)).unwrap_or(false);
    let run_slot = |img: &mut ImageF32, slot: Slot| {
        for (e, seed) in fx.iter().filter(|(e, _)| e.slot() == slot) {
            e.apply(img, &FxCtx { seed: *seed, negative, mono });
        }
    };

    check(hooks, 0.02)?;
    // ---- lens
    let mut img = lens::lateral_ca(src, o.ca_lateral, &g);
    check(hooks, 0.08)?;
    img = bokeh::render(&img, o, &g);
    check(hooks, 0.45)?;
    lens::glow(&mut img, o.glow, o.glow_radius);
    lens::veil(&mut img, o.veil);
    lens::vignette(&mut img, o.vignette, &g);
    run_slot(&mut img, Slot::Optical);
    check(hooks, 0.55)?;

    // ---- film (exposure space)
    let ev = res.params.exposure.clamp(-5.0, 5.0).exp2();
    if (ev - 1.0).abs() > 1e-4 {
        img.data.iter_mut().for_each(|v| *v *= ev);
    }
    run_slot(&mut img, Slot::Exposure);
    if let Some(f) = film {
        if let Some(ir) = &f.ir {
            infrared::apply(&mut img, ir);
        }
        check(hooks, 0.62)?;
        halation::apply(&mut img, &f.halation, res.params.halation);
        check(hooks, 0.70)?;
        let tone = ToneXform::new(f, &res.params);
        img.par_map_pixels(|_, _, p| {
            let y = tone.develop([p[0], p[1], p[2]]);
            p.copy_from_slice(&y);
        });
        check(hooks, 0.78)?;
        grain::apply(&mut img, &f.grain, res.params.grain, res.seed, tone.is_mono);
        check(hooks, 0.88)?;
        run_slot(&mut img, Slot::Emulsion);
        img.par_map_pixels(|_, _, p| {
            let y = tone.print([p[0], p[1], p[2]]);
            p.copy_from_slice(&y);
        });
    } else {
        img.data.iter_mut().for_each(|v| *v = linear_to_srgb(*v).clamp(0.0, 1.0));
        run_slot(&mut img, Slot::Emulsion);
    }
    run_slot(&mut img, Slot::Print);
    img.data.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    check(hooks, 0.95)?;
    Ok(img)
}
