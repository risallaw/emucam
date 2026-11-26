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
    u(&mut m, "uFocusCenter", &res.focus_center);
    u(&mut m, "uFocusAngle", &[res.focus_angle.to_radians()]);
    u(&mut m, "uS0", &[o.s0]);
    u(&mut m, "uFalloff", &[o.falloff]);
    u(&mut m, "uRmax", &[o.rmax]);
    let (ap_kind, ap_a, ap_b) = match &o.aperture {
        Aperture::Circle => (0.0, 0.0, 0.0),
        Aperture::Polygon { blades, rotation } => (1.0, *blades as f32, rotation.to_radians()),
        Aperture::Shape { id } => (2.0, SHAPE_IDS.iter().position(|s| s == id).unwrap_or(0) as f32, 0.0),
        Aperture::Slit { ratio, rotation } => (3.0, *ratio, rotation.to_radians()),
    };
    u(&mut m, "uAperture", &[ap_kind, ap_a, ap_b]);
    u(&mut m, "uApArea", &[crate::lens::aperture::area(&o.aperture)]);
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
    u(&mut m, "uHighlightK", &[crate::lens::bokeh::highlight_k(o.highlight_gain), crate::lens::bokeh::HL_KNEE]);

    // ---- film
    u(&mut m, "uExposure", &[res.params.exposure.clamp(-5.0, 5.0).exp2()]);
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
            u(&mut m, "uIr", &[v, ir.k_veg, ir.k_sky]);
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
        u(&mut m, "uGrain", &[ga, gr.size_um, if mono > 0.0 { 0.0 } else { gr.chroma }]);
    }
    u(&mut m, "uMono", &[mono]);
    u(&mut m, "uSeed", &[(res.seed % 65_536) as f32]);

    // ---- simple preview-able effects (others are capture-only for now)
    let mut fx_vig = 0.0;
    let mut frame = [0.0f32; 2];
    for e in &res.effects {
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
            "light_leak" => {
                defines.push("FX_LEAK".into());
                u(&mut m, "uLeak", &[e.strength, e.f("size", 0.35)]);
                u(&mut m, "uLeakColor", &e.rgb("color", [1.0, 0.35, 0.08]));
            }
            _ => {}
        }
    }
    u(&mut m, "uFxVignette", &[fx_vig]);
    u(&mut m, "uFrame", &frame);

    defines.sort();
    defines.dedup();
    let program_key = defines.join("+");
    PreviewParams { uniforms: m, defines, program_key, lut_size: PREVIEW_LUT_SIZE }
}

/// Convenience: both LUTs as RGBA16F bytes.
pub fn luts_f16(res: &Resolved) -> (Vec<u8>, Vec<u8>) {
    let l = lut::bake(res, PREVIEW_LUT_SIZE);
    (l.develop.to_rgba_f16_bytes(), l.print.to_rgba_f16_bytes())
}
