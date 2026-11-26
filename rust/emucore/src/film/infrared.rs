//! Infrared film emulation from visible RGB. Phone sensors block IR, so we
//! estimate the near-IR record: vegetation (high NIR reflectance) is detected
//! with the ExG index on chromaticity, skies (low NIR) with a blue-dominance
//! mask and a vertical prior. Then the film's layer remap is applied.

use crate::blur::gaussian_mask;
use crate::color::{hue_rotate, luma, mix3, smoothstep};
use crate::image::{ImageF32, MaskF32};
use crate::preset::{IrSpec, IrVariant};
use rayon::prelude::*;

pub struct IrMasks {
    pub veg: MaskF32,
    pub sky: MaskF32,
}

#[inline]
pub fn veg_index(c: [f32; 3], lo: f32, hi: f32) -> f32 {
    let s = c[0] + c[1] + c[2] + 1e-6;
    let (r, g, b) = (c[0] / s, c[1] / s, c[2] / s);
    let exg = 2.0 * g - r - b;
    smoothstep(lo, hi, exg) * smoothstep(0.01, 0.06, luma(c))
}

#[inline]
pub fn sky_index(c: [f32; 3], v: f32) -> f32 {
    let s = c[0] + c[1] + c[2] + 1e-6;
    let (r, g, b) = (c[0] / s, c[1] / s, c[2] / s);
    smoothstep(0.04, 0.14, b - r.max(g)) * smoothstep(0.02, 0.1, luma(c)) * smoothstep(0.9, 0.3, v)
}

/// Pseudo near-IR record. Most materials reflect NIR roughly like red light;
/// chlorophyll foliage reflects ~4–5× its green (Wood effect); clear sky and
/// water have very little NIR.
#[inline]
pub fn pseudo_ir(c: [f32; 3], veg: f32, sky: f32, ir: &IrSpec) -> f32 {
    // warm materials (brick, skin, wood) reflect more NIR; blue ones less
    let base = (0.75 * c[0] + 0.25 * c[1] + 0.4 * (c[0] - c[2])).max(0.0);
    let foliage = c[1] * ir.k_veg;
    let v = base + (foliage.max(base) - base) * veg;
    v * (1.0 - ir.k_sky * sky)
}

/// Build low-res masks (≈512 px wide), blurred so edges never show.
pub fn masks(img: &ImageF32, ir: &IrSpec) -> IrMasks {
    let factor = (img.w / 512).max(1);
    let small = img.downsample(factor);
    let (w, h) = (small.w, small.h);
    let mut veg = MaskF32::new(w, h);
    let mut sky = MaskF32::new(w, h);
    veg.data
        .par_chunks_exact_mut(w)
        .zip(sky.data.par_chunks_exact_mut(w))
        .enumerate()
        .for_each(|(y, (vr, sr))| {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                let c = small.get(x, y);
                vr[x] = veg_index(c, ir.veg_lo, ir.veg_hi);
                sr[x] = sky_index(c, v);
            }
        });
    let sigma = (w as f32 * 0.005).max(0.8);
    gaussian_mask(&mut veg, sigma);
    gaussian_mask(&mut sky, sigma * 2.0);
    IrMasks { veg, sky }
}

/// Per-pixel IR remap given mask values.
#[inline]
pub fn remap(c: [f32; 3], veg: f32, sky: f32, ir: &IrSpec) -> [f32; 3] {
    let irv = pseudo_ir(c, veg, sky, ir);
    let [r, g, b] = c;
    match ir.variant {
        IrVariant::Aerochrome => [irv, r, g],
        IrVariant::AerochromeOrange => [irv * 1.05, r * 0.95, 0.6 * g + 0.2 * r],
        IrVariant::Mono => [irv; 3],
        IrVariant::Golden => [irv, 0.35 * g + 0.55 * irv, 0.9 * b + 0.1 * r],
        IrVariant::Purple => {
            let rot = hue_rotate(c, 150.0).map(|v| v.max(0.0) * 1.5);
            let base = hue_rotate(c, -12.0).map(|v| v.max(0.0));
            mix3(base, rot, veg)
        }
    }
}

pub fn apply(img: &mut ImageF32, ir: &IrSpec) {
    let m = masks(img, ir);
    let (w, h) = (img.w as f32, img.h as f32);
    img.par_map_pixels(|x, y, p| {
        let u = (x as f32 + 0.5) / w;
        let v = (y as f32 + 0.5) / h;
        let veg = m.veg.sample_uv(u, v);
        let sky = m.sky.sample_uv(u, v);
        let o = remap([p[0], p[1], p[2]], veg, sky, ir);
        p.copy_from_slice(&o);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(v: IrVariant) -> IrSpec {
        IrSpec { variant: v, k_veg: 4.5, k_sky: 0.7, veg_lo: 0.02, veg_hi: 0.12 }
    }

    #[test]
    fn foliage_turns_red_in_aerochrome() {
        let leaf = [0.08, 0.30, 0.05];
        let veg = veg_index(leaf, 0.02, 0.12);
        assert!(veg > 0.9);
        let o = remap(leaf, veg, 0.0, &spec(IrVariant::Aerochrome));
        assert!(o[0] > o[1] * 2.0 && o[0] > o[2], "{o:?}");
    }

    #[test]
    fn sky_darkens_in_mono_ir() {
        let skyc = [0.25, 0.4, 0.8];
        let s = sky_index(skyc, 0.1);
        assert!(s > 0.5, "sky {s}");
        let o = remap(skyc, 0.0, s, &spec(IrVariant::Mono));
        assert!(o[0] < luma(skyc) * 0.7);
    }

    #[test]
    fn foliage_is_bright_in_mono_ir() {
        let leaf = [0.05, 0.12, 0.03];
        let o = remap(leaf, 1.0, 0.0, &spec(IrVariant::Mono));
        assert!(o[0] > luma(leaf) * 3.0, "{o:?}");
    }
}
