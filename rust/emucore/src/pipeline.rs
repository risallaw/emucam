//! Full-quality render pipeline (stage order is physical):
//!
//! lens: lateral CA → bokeh → glow/veil → vignette → optical effects
//! film: exposure → exposure effects (leaks) → IR → halation → develop (tone)
//!       → grain → emulsion effects → print → print effects

use crate::color::linear_to_srgb;
use crate::depth::{self, DepthField};
use crate::effects::{self, Effect, FxCtx};
use crate::film::{grain, halation, infrared, ToneXform};
use crate::image::{ImageF32, MaskF32};
use crate::lens::{self, bokeh, Geometry};
use crate::preset::BlurField;
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
        Hooks {
            progress: &|_| {},
            cancelled: &|| false,
        }
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
    render_with_depth(src, res, None, hooks)
}

/// Whether a depth map would change this render: the lens blurs, responds to
/// depth, and the user hasn't turned depth off. Apps skip depth inference
/// otherwise.
pub fn wants_depth(res: &Resolved) -> bool {
    let o = &res.optics;
    res.params.depth && o.rmax > 0.0 && o.field != BlurField::None && o.depth_scale > 0.0
}

/// Build the depth field for this render, or None when it isn't wanted or
/// there is no map.
fn depth_field(src: &ImageF32, res: &Resolved, near: Option<&MaskF32>) -> Option<DepthField> {
    if !wants_depth(res) {
        return None;
    }
    let near = near?;
    let t0 = std::time::Instant::now();
    let full = depth::upsample_guided(near, src);
    let focus = depth::focus_nearness(near, res.focus_center);
    if std::env::var_os("EMUCAM_PROF").is_some() {
        eprintln!(
            "depth: map {}x{} focus {:?} → nearness {focus:.3}, upsampled in {:.2?}",
            near.w,
            near.h,
            res.focus_center,
            t0.elapsed()
        );
    }
    Some(DepthField { near: full, focus })
}

/// Like [`render`], with an optional nearness map (any resolution, photo
/// orientation, 0 far … 1 near — see [`depth::nearness`]) for depth defocus.
pub fn render_with_depth(
    src: &ImageF32,
    res: &Resolved,
    near: Option<&MaskF32>,
    hooks: &Hooks,
) -> Result<ImageF32, Error> {
    render_with_aux(src, res, near, None, hooks)
}

/// Whether a vegetation map would change this render: only infrared films
/// use it. Apps skip segmentation inference otherwise.
pub fn wants_veg(res: &Resolved) -> bool {
    res.film.as_ref().is_some_and(|f| f.ir.is_some())
}

/// Like [`render_with_depth`], plus an optional vegetation-probability map
/// (any resolution, photo orientation, 0..1 — see [`crate::seg::vegetation`])
/// that gates the infrared film's foliage detection.
pub fn render_with_aux(
    src: &ImageF32,
    res: &Resolved,
    near: Option<&MaskF32>,
    veg: Option<&MaskF32>,
    hooks: &Hooks,
) -> Result<ImageF32, Error> {
    let o = &res.optics;
    // Fisheye lenses reshape the whole frame first: everything after (blur,
    // vignette, film) happens in the fisheye image, and aux maps follow it.
    // Aux maps nothing will read are left alone.
    let warp = lens::projection::Warp::new(o);
    let (warped, near_w, veg_w) = match &warp {
        Some(wp) => (
            Some(wp.image(src)),
            near.filter(|_| wants_depth(res))
                .map(|m| wp.mask(m, src.w, src.h)),
            veg.filter(|_| wants_veg(res))
                .map(|m| wp.mask(m, src.w, src.h)),
        ),
        None => (None, None, None),
    };
    let src = warped.as_ref().unwrap_or(src);
    let (near, veg) = if warp.is_some() {
        (near_w.as_ref(), veg_w.as_ref())
    } else {
        (near, veg)
    };
    let g = Geometry::new(src.w, src.h, res.focus_center, res.focus_angle, o.field);
    let dfield = depth_field(src, res, near);
    let film = res.film.as_ref();

    // Instantiate effects with their seeds, grouped by slot.
    let fx: Vec<(Box<dyn Effect>, u64)> = res
        .effects
        .iter()
        .enumerate()
        .filter_map(|(i, e)| effects::build(e).map(|b| (b, effect_seed(e, res.seed, i))))
        .collect();
    let negative = film
        .map(|f| {
            matches!(
                f.kind,
                FilmKind::ColorNegative | FilmKind::BwNegative | FilmKind::BwInfrared
            )
        })
        .unwrap_or(false);
    let mono = film
        .map(|f| matches!(f.kind, FilmKind::BwNegative | FilmKind::BwInfrared))
        .unwrap_or(false);
    let run_slot = |img: &mut ImageF32, slot: Slot| {
        for (e, seed) in fx.iter().filter(|(e, _)| e.slot() == slot) {
            e.apply(
                img,
                &FxCtx {
                    seed: *seed,
                    rotation: res.capture_rotation,
                    negative,
                    mono,
                },
            );
        }
    };

    check(hooks, 0.02)?;
    // ---- lens
    let mut img = lens::lateral_ca(src, o.ca_lateral, &g);
    check(hooks, 0.08)?;
    img = bokeh::render_with_depth(&img, o, &g, dfield.as_ref());
    check(hooks, 0.45)?;
    lens::glow(&mut img, o.glow, o.glow_radius);
    lens::veil(&mut img, o.veil);
    lens::vignette(&mut img, o.vignette, &g);
    if let Some(wp) = &warp {
        wp.clip_circle(&mut img);
    }
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
            infrared::apply(&mut img, ir, veg);
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
        img.data
            .iter_mut()
            .for_each(|v| *v = linear_to_srgb(*v).clamp(0.0, 1.0));
        run_slot(&mut img, Slot::Emulsion);
    }
    run_slot(&mut img, Slot::Print);
    img.data.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    check(hooks, 0.95)?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::recipe::{resolve, Recipe};

    fn wants(lens: &str, depth: bool) -> bool {
        let mut r = Recipe {
            lens: Some(lens.into()),
            ..Default::default()
        };
        r.params.depth = depth;
        wants_depth(&resolve(&r, Catalog::builtin()).unwrap())
    }

    #[test]
    fn veg_is_wanted_only_by_infrared_films() {
        let wants_film = |film: &str| {
            let r = Recipe {
                film: Some(film.into()),
                ..Default::default()
            };
            wants_veg(&resolve(&r, Catalog::builtin()).unwrap())
        };
        assert!(wants_film("aero-ir"));
        assert!(!wants_film("portrait-400"));
        assert!(!wants_veg(
            &resolve(&Recipe::default(), Catalog::builtin()).unwrap()
        ));
    }

    #[test]
    fn depth_is_wanted_only_by_lenses_that_focus() {
        assert!(wants("helios-58", true));
        assert!(
            wants("sweet-50", true),
            "optical sweet spot still adds focus blur"
        );
        assert!(!wants("helios-58", false), "user turned depth off");
        assert!(!wants("standard", true), "no blur");
        assert!(!wants("pinhole", true), "no focus plane");
    }

    /// Veil/glow spread light, but nothing reaches outside a circular
    /// fisheye's image circle (the preview draws it black too).
    #[test]
    fn circular_fisheye_border_stays_black() {
        let r = Recipe {
            lens: Some("circular-8".into()),
            ..Default::default()
        };
        let res = resolve(&r, Catalog::builtin()).unwrap();
        assert!(res.optics.veil > 0.0, "test needs a lens that veils");
        let src = ImageF32::filled(60, 40, [0.5, 0.5, 0.5]);
        let out = render(&src, &res, &Hooks::default()).unwrap();
        assert_eq!(out.get(0, 0), [0.0; 3]);
        assert!(out.get(30, 20)[0] > 0.3);
    }
}
