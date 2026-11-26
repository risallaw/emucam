//! Halation: light passing through the emulsion reflects off the film base and
//! re-exposes the (deepest) red layer around bright sources. Modelled as a
//! highlight-thresholded, sum-of-Gaussians glow with per-channel radii.

use crate::blur::gaussian_mask;
use crate::color::luma;
use crate::image::{ImageF32, MaskF32};
use crate::preset::HalationSpec;
use rayon::prelude::*;

/// σ as fraction of width and weights of the three kernel lobes.
pub const LOBES: [(f32, f32); 3] = [(0.003, 0.5), (0.008, 0.35), (0.02, 0.15)];
/// Per-channel radius ratio R:G:B.
pub const CH_RADIUS: [f32; 3] = [1.4, 1.0, 0.7];

/// Halation source: ramps from 0 at `threshold` to 1 at clipping (JPEG input
/// is clipped at 1.0), and keeps growing for brighter (exposure-boosted or
/// leaked) light, softly limited.
#[inline]
pub fn source(c: [f32; 3], threshold: f32) -> f32 {
    let t = threshold.clamp(0.0, 0.98);
    let e = (luma(c) - t).max(0.0) / (1.0 - t);
    e / (1.0 + 0.25 * e)
}

pub fn apply(img: &mut ImageF32, spec: &HalationSpec, user: f32) {
    let k = spec.strength * user;
    if k <= 0.0 {
        return;
    }
    let factor = (img.w / 1024).max(1);
    let small = img.downsample(factor);
    let (w, h) = (small.w, small.h);
    let mut src = MaskF32::new(w, h);
    src.data.par_iter_mut().enumerate().for_each(|(i, v)| {
        let c = [small.data[i * 3], small.data[i * 3 + 1], small.data[i * 3 + 2]];
        *v = source(c, spec.threshold);
    });
    // Per channel accumulated halo.
    let mut halo: Vec<MaskF32> = Vec::with_capacity(3);
    for (c, ch_r) in CH_RADIUS.iter().enumerate() {
        let mut acc = MaskF32::new(w, h);
        if spec.tint[c] > 1e-4 {
            for (sig, wt) in LOBES {
                let mut m = src.clone();
                gaussian_mask(&mut m, sig * w as f32 * spec.radius * ch_r);
                acc.data.iter_mut().zip(m.data.iter()).for_each(|(a, b)| *a += wt * b);
            }
        }
        halo.push(acc);
    }
    let (fw, fh) = (img.w as f32, img.h as f32);
    let tint = spec.tint;
    img.par_map_pixels(|x, y, p| {
        let u = (x as f32 + 0.5) / fw;
        let v = (y as f32 + 0.5) / fh;
        for c in 0..3 {
            if tint[c] > 1e-4 {
                p[c] += k * tint[c] * halo[c].sample_uv(u, v) * 4.0;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_halation_below_threshold() {
        let mut img = ImageF32::filled(64, 64, [0.5, 0.5, 0.5]);
        let before = img.data.clone();
        apply(&mut img, &HalationSpec { strength: 1.0, ..Default::default() }, 1.0);
        assert_eq!(before, img.data);
    }

    #[test]
    fn bright_point_gets_red_halo() {
        let mut img = ImageF32::filled(256, 256, [0.05, 0.05, 0.05]);
        for y in 126..130 {
            for x in 126..130 {
                img.set(x, y, [50.0, 50.0, 50.0]);
            }
        }
        apply(&mut img, &HalationSpec { strength: 0.3, ..Default::default() }, 1.0);
        let p = img.get(128, 136);
        assert!(p[0] > p[1] && p[1] > p[2], "{p:?}");
    }
}
