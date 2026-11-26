//! Preview parameter export for the GPU shader. Rust is the single source of
//! truth: Kotlin uploads every entry of `uniforms` with `glUniform*` by name
//! (no hand-mapping), compiles the shader with `defines`, and uploads LUTs.
//! New effect parameters only need a new uniform here + use in GLSL.

use crate::lens::aperture::SHAPE_IDS;
use crate::lut::{self, PREVIEW_LUT_SIZE};
use crate::preset::{Aperture, BlurField, FilmKind, IrVariant};
use crate::recipe::Resolved;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct PreviewParams {
    /// uniform name → 1..4 floats (int-like values are passed as floats).
    pub uniforms: BTreeMap<String, Vec<f32>>,
    /// `#define` names for shader variants.
    pub defines: Vec<String>,
    /// Stable key for caching compiled programs (hash of defines).
    pub program_key: String,
    pub lut_size: usize,
}

/// Light-leak blob slots in `final.frag` (`uLeakBlob[]`), shared by all the
/// recipe's leaks: two or three leaks of up to [`crate::effects::LEAK_MAX_BLOBS`].
pub const PREVIEW_LEAK_SLOTS: usize = 8;

fn u(m: &mut BTreeMap<String, Vec<f32>>, k: &str, v: &[f32]) {
    m.insert(k.to_string(), v.to_vec());
}

pub fn params(res: &Resolved) -> PreviewParams {
    let o = &res.optics;
    let mut m = BTreeMap::new();
    let mut defines = vec![];

    // ---- lens
    let field = match o.field {
        BlurField::None => 0.0,
        BlurField::Uniform => 1.0,
        BlurField::SweetSpot => 2.0,
        BlurField::Slice => 3.0,
    };
    if o.rmax > 0.0 && o.field != BlurField::None {
        defines.push("LENS_BLUR".into());
    }
    u(&mut m, "uField", &[field]);
    if let Some(wp) = crate::lens::projection::Warp::new(o) {
        defines.push("LENS_FISHEYE".into());
        u(&mut m, "uProj", &wp.uniform());
    }
    u(&mut m, "uFocusCenter", &res.focus_center);
    u(&mut m, "uFocusAngle", &[res.focus_angle.to_radians()]);
    u(&mut m, "uS0", &[o.s0]);
    u(&mut m, "uFalloff", &[o.falloff]);
    u(&mut m, "uRmax", &[o.rmax]);
    // depth defocus (mirrors lens::Defocus): scale, field is optical, wanted
    u(
        &mut m,
        "uDepthParams",
        &[
            o.depth_scale,
            if o.field_optical { 1.0 } else { 0.0 },
            if crate::pipeline::wants_depth(res) {
                1.0
            } else {
                0.0
            },
        ],
    );
    u(
        &mut m,
        "uDepthDelta",
        &[crate::depth::SHARP_DELTA, crate::depth::FULL_BLUR_DELTA],
    );
    let (ap_kind, ap_a, ap_b) = match &o.aperture {
        Aperture::Circle => (0.0, 0.0, 0.0),
        Aperture::Polygon { blades, rotation } => (1.0, *blades as f32, rotation.to_radians()),
        Aperture::Shape { id } => (
            2.0,
            SHAPE_IDS.iter().position(|s| s == id).unwrap_or(0) as f32,
            0.0,
        ),
        Aperture::Slit { ratio, rotation } => (3.0, *ratio, rotation.to_radians()),
    };
    u(&mut m, "uAperture", &[ap_kind, ap_a, ap_b]);
    u(
        &mut m,
        "uApArea",
        &[crate::lens::aperture::area(&o.aperture)],
    );
    u(&mut m, "uSwirl", &[o.swirl]);
    u(&mut m, "uCatsEye", &[o.cats_eye]);
    u(&mut m, "uBubble", &[o.bubble]);
    u(&mut m, "uOnion", &[o.onion]);
    u(&mut m, "uCaLateral", &[o.ca_lateral]);
    u(&mut m, "uCaLong", &[o.ca_long]);
    u(&mut m, "uGlow", &[o.glow, o.glow_radius]);
    if o.glow > 0.0 {
        defines.push("LENS_GLOW".into());
    }
    u(&mut m, "uVignette", &[o.vignette]);
    u(&mut m, "uVeil", &[o.veil]);
    u(
        &mut m,
        "uHighlightK",
        &[
            crate::lens::bokeh::highlight_k(o.highlight_gain),
            crate::lens::bokeh::HL_KNEE,
        ],
    );

    // ---- film
    u(
        &mut m,
        "uExposure",
        &[res.params.exposure.clamp(-5.0, 5.0).exp2()],
    );
    let mut mono = 0.0;
    if let Some(f) = &res.film {
        defines.push("FILM".into());
        if matches!(f.kind, FilmKind::BwNegative | FilmKind::BwInfrared) {
            mono = 1.0;
        }
        if let Some(ir) = &f.ir {
            defines.push("IR".into());
            let v = match ir.variant {
                IrVariant::Aerochrome => 0.0,
                IrVariant::AerochromeOrange => 1.0,
                IrVariant::Mono => 2.0,
                IrVariant::Golden => 3.0,
                IrVariant::Purple => 4.0,
            };
            u(&mut m, "uIr", &[v, ir.k_veg, ir.k_sky, ir.k_skin]);
            u(&mut m, "uIrVeg", &[ir.veg_lo, ir.veg_hi]);
        }
        let h = &f.halation;
        let hs = h.strength * res.params.halation;
        if hs > 0.0 {
            defines.push("HALATION".into());
        }
        u(&mut m, "uHalation", &[hs, h.threshold, h.radius]);
        u(&mut m, "uHalationTint", &h.tint);
        let gr = &f.grain;
        let ga = gr.amount * res.params.grain;
        if ga > 0.0 {
            defines.push("GRAIN".into());
        }
        u(
            &mut m,
            "uGrain",
            &[ga, gr.size_um, if mono > 0.0 { 0.0 } else { gr.chroma }],
        );
    }
    u(&mut m, "uMono", &[mono]);

    // ---- simple preview-able effects (others are capture-only for now)
    let mut fx_vig = 0.0;
    let mut frame = [0.0f32; 2];
    let mut leak: Vec<([f32; 4], [f32; 3], [f32; 2])> = vec![];
    for (i, e) in res.effects.iter().enumerate() {
        match e.kind.as_str() {
            "vignette" => fx_vig += e.strength,
            "frame" => {
                frame[0] = e.f("thickness", 0.035) * e.strength;
                frame[1] = match e.s("style", "black") {
                    "white" => 1.0,
                    "rebate" => 2.0,
                    _ => 0.0,
                };
            }
            "omni" => {
                let o = crate::effects::omni::Omni::from_spec(e);
                defines.push("FX_OMNI".into());
                u(
                    &mut m,
                    "uOmni",
                    &[o.wand as f32, o.position, o.reach, o.strength],
                );
            }
            "light_leak" => {
                defines.push("FX_LEAK".into());
                // the exact blobs the capture draws: same generator, same seed
                // as pipeline (effect index i in res.effects). Every leak of the
                // recipe (body, lens and user ones) shares the slots.
                let seed = crate::recipe::effect_seed(e, res.seed, i);
                let (placed, blobs) = crate::effects::leak_blobs(e, seed);
                let pos = [placed.unwrap_or(0.0), placed.map_or(0.0, |_| 1.0)];
                for b in blobs {
                    leak.push(([b.at[0], b.at[1], b.r, b.inten * e.strength], b.col, pos));
                }
            }
            _ => {}
        }
    }
    if !leak.is_empty() {
        if leak.len() > PREVIEW_LEAK_SLOTS {
            eprintln!(
                "preview: {} light-leak blobs, showing {PREVIEW_LEAK_SLOTS}",
                leak.len()
            );
        }
        // every slot always uploaded (unused: intensity 0): GL keeps old
        // values in a cached program
        for k in 0..PREVIEW_LEAK_SLOTS {
            let (b, col, pos) =
                leak.get(k)
                    .copied()
                    .unwrap_or(([0.0, 0.0, 1.0, 0.0], [0.0; 3], [0.0; 2]));
            u(&mut m, &format!("uLeakBlob[{k}]"), &b);
            u(&mut m, &format!("uLeakCol[{k}]"), &col);
            u(&mut m, &format!("uLeakPos[{k}]"), &pos);
        }
    }
    u(&mut m, "uFxVignette", &[fx_vig]);
    u(&mut m, "uFrame", &frame);

    defines.sort();
    defines.dedup();
    let program_key = defines.join("+");
    PreviewParams {
        uniforms: m,
        defines,
        program_key,
        lut_size: PREVIEW_LUT_SIZE,
    }
}

/// Convenience: both LUTs as RGBA16F bytes.
pub fn luts_f16(res: &Resolved) -> (Vec<u8>, Vec<u8>) {
    let l = lut::bake(res, PREVIEW_LUT_SIZE);
    (l.develop.to_rgba_f16_bytes(), l.print.to_rgba_f16_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::effects::{build, FxCtx};
    use crate::image::ImageF32;
    use crate::recipe::{effect_seed, resolve, Recipe};

    /// final.frag `leak()` evaluated on the CPU from the uploaded uniforms.
    fn shader_leak(p: &PreviewParams, q: [f32; 2], aspect: f32) -> [f32; 3] {
        let un = |k: &str| p.uniforms[k].clone();
        let mut acc = [0.0f32; 3];
        for k in 0..PREVIEW_LEAK_SLOTS {
            let b = un(&format!("uLeakBlob[{k}]"));
            let col = un(&format!("uLeakCol[{k}]"));
            let pos = un(&format!("uLeakPos[{k}]"));
            if b[3] <= 0.0 {
                continue;
            }
            let mut ctr = [b[0], b[1]];
            if pos[1] > 0.5 {
                let (an, o) = crate::effects::edge_point(pos[0], aspect, 1.0);
                ctr = [
                    (an[0] - o[1] * b[0] + o[0] * b[1]) / aspect,
                    an[1] + o[0] * b[0] + o[1] * b[1],
                ];
            }
            let d = [(q[0] - ctr[0]) * aspect / b[2], (q[1] - ctr[1]) / b[2]];
            let f = (-(d[0] * d[0] + d[1] * d[1]) * 2.5).exp() * b[3];
            for c in 0..3 {
                acc[c] += col[c] * f;
            }
        }
        acc
    }

    /// The preview leak equals the leaks the capture pipeline draws for the
    /// same recipe seed: placed or not, wherever they sit in the effect list,
    /// and all of them when body, lens and user each bring one.
    #[test]
    fn preview_leak_matches_capture() {
        let placed: crate::preset::EffectSpec = toml::from_str(
            "kind = \"light_leak\"\nseed_mode = \"per_shot\"\n[params]\nposition = 300.0\ncount = 3",
        )
        .unwrap();
        let before: crate::preset::EffectSpec = toml::from_str("kind = \"vignette\"").unwrap();
        let cases = [
            (None, "toy-45", vec![]),
            (None, "standard", vec![before, placed.clone()]),
            // body leak + lens leak (+ a placed user leak): three leaks, 7 blobs
            (Some("toy-cam"), "toy-45", vec![]),
            (Some("toy-cam"), "toy-45", vec![placed]),
        ];
        for (body, lens, extra) in cases {
            let r = Recipe {
                body: body.map(Into::into),
                lens: body.is_none().then(|| lens.into()),
                effects: extra,
                seed: 987_654_321,
                ..Default::default()
            };
            let res = resolve(&r, Catalog::builtin()).unwrap();
            let p = params(&res);
            let (w, h) = (60, 40);
            let mut img = ImageF32::new(w, h);
            let mut leaks = 0;
            for (i, e) in res.effects.iter().enumerate() {
                if e.kind != "light_leak" {
                    continue;
                }
                leaks += 1;
                build(e).unwrap().apply(
                    &mut img,
                    &FxCtx {
                        rotation: 0,
                        seed: effect_seed(e, res.seed, i),
                        negative: false,
                        mono: false,
                    },
                );
            }
            assert!(leaks > 0);
            let mut max = 0.0f32;
            for y in 0..h {
                for x in 0..w {
                    let q = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
                    let s = shader_leak(&p, q, w as f32 / h as f32);
                    let c = img.get(x, y);
                    for k in 0..3 {
                        assert!(
                            (s[k] - c[k]).abs() < 1e-4,
                            "{body:?} {lens} ({x},{y}) {s:?} vs {c:?}"
                        );
                    }
                    max = max.max(c[0]);
                }
            }
            assert!(max > 0.05, "{lens}: the leak is visible");
        }
    }

    /// A cached GL program keeps old uniform values: an unplaced leak after a
    /// placed one must reset the placement, and unused slots must be off.
    #[test]
    fn unplaced_leak_resets_placement() {
        let r = Recipe {
            lens: Some("toy-45".into()),
            ..Default::default()
        };
        let p = params(&resolve(&r, Catalog::builtin()).unwrap());
        for k in 0..PREVIEW_LEAK_SLOTS {
            assert_eq!(p.uniforms[&format!("uLeakPos[{k}]")], vec![0.0, 0.0]);
        }
        assert_eq!(
            p.uniforms[&format!("uLeakBlob[{}]", PREVIEW_LEAK_SLOTS - 1)][3],
            0.0
        );
    }
}
