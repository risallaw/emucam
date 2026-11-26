//! Film grain in display (density-like) space. Multi-octave blurred hash noise,
//! scaled to the physical 36 mm frame so grain looks the same at any resolution,
//! with amplitude ∝ sqrt(y(1−y)) (binomial coverage variance → strongest in mids).

use crate::blur::gaussian_mask;
use crate::image::{ImageF32, MaskF32};
use crate::noise::{gauss3, pcg3d};
use crate::preset::GrainSpec;
use rayon::prelude::*;

// (size multiplier, weight). No coarse (2×) octave: at phone resolutions it
// read as mottled smear over fine detail rather than grain.
const OCTAVES: [(f32, f32); 2] = [(0.5, 0.7), (1.0, 0.3)];

fn noise_field(w: usize, h: usize, seed: u32, layer: u32, sigma: f32) -> MaskF32 {
    let mut m = MaskF32::new(w, h);
    m.data
        .par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = gauss3(pcg3d([
                    x as u32,
                    y as u32,
                    seed ^ layer.wrapping_mul(0x9E37_79B9),
                ]));
            }
        });
    if sigma >= 0.3 {
        gaussian_mask(&mut m, sigma);
        // Blurred unit white noise has std 1/(2·sqrt(pi)·σ); renormalize.
        let norm = 2.0 * std::f32::consts::PI.sqrt() * sigma;
        m.data.par_iter_mut().for_each(|v| *v *= norm);
    }
    m
}

/// Physical grain size in output pixels (36 mm frame width).
pub fn grain_sigma_true(spec: &GrainSpec, width: usize) -> f32 {
    spec.size_um / 36_000.0 * width as f32
}

pub fn grain_sigma_px(spec: &GrainSpec, width: usize) -> f32 {
    grain_sigma_true(spec, width).max(0.5)
}

/// Grain smaller than a pixel averages out: scale amplitude down accordingly.
pub fn grain_resolution_scale(spec: &GrainSpec, width: usize) -> f32 {
    (grain_sigma_true(spec, width) / 0.6).clamp(0.15, 1.0)
}

/// Apply grain to a display-space image.
pub fn apply(img: &mut ImageF32, spec: &GrainSpec, user: f32, seed: u64, mono: bool) {
    let amount = spec.amount * user;
    if amount <= 0.0 {
        return;
    }
    let (w, h) = (img.w, img.h);
    let s = grain_sigma_px(spec, w);
    let seed32 = (seed ^ (seed >> 32)) as u32;
    let wsum: f32 = OCTAVES.iter().map(|(_, wt)| wt * wt).sum::<f32>().sqrt();
    let mut lum = MaskF32::new(w, h);
    for (i, (mul, wt)) in OCTAVES.iter().enumerate() {
        let f = noise_field(w, h, seed32, i as u32, s * mul);
        lum.data
            .iter_mut()
            .zip(f.data.iter())
            .for_each(|(a, b)| *a += b * wt / wsum);
    }
    let chroma = if mono {
        0.0
    } else {
        spec.chroma.clamp(0.0, 1.0)
    };
    let chans: Vec<MaskF32> = if chroma > 0.0 {
        (0..3)
            .map(|c| noise_field(w, h, seed32, 10 + c, s))
            .collect()
    } else {
        vec![]
    };
    let a_l = (1.0 - chroma).sqrt();
    let a_c = chroma.sqrt();
    // 0.10: at 12 MP the old 0.18 (≈6/255 rms in mids) erased text-sized
    // detail; this keeps visible texture without eating the image.
    let amp = amount * 0.10 * grain_resolution_scale(spec, w);
    img.par_map_pixels(|x, y, p| {
        let i = y * w + x;
        let n = lum.data[i];
        for c in 0..3 {
            let nc = if chroma > 0.0 {
                a_l * n + a_c * chans[c].data[i]
            } else {
                n
            };
            let v = p[c].clamp(0.0, 1.0);
            p[c] = v + amp * nc * (v * (1.0 - v)).sqrt();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grain_is_mean_preserving_and_midtone_peaked() {
        let spec = GrainSpec {
            amount: 0.5,
            size_um: 15.0,
            chroma: 0.0,
        };
        let mut mid = ImageF32::filled(512, 512, [0.5; 3]);
        let mut dark = ImageF32::filled(512, 512, [0.02; 3]);
        apply(&mut mid, &spec, 1.0, 7, true);
        apply(&mut dark, &spec, 1.0, 7, true);
        let stats = |img: &ImageF32| {
            let n = (img.w * img.h) as f32;
            let m: f32 = img.data.iter().step_by(3).sum::<f32>() / n;
            let v: f32 = img
                .data
                .iter()
                .step_by(3)
                .map(|x| (x - m) * (x - m))
                .sum::<f32>()
                / n;
            (m, v.sqrt())
        };
        let (mm, ms) = stats(&mid);
        let (_, ds) = stats(&dark);
        assert!((mm - 0.5).abs() < 0.01, "mean {mm}");
        assert!(ms > ds * 2.0, "mid std {ms} dark std {ds}");
    }

    #[test]
    fn deterministic_for_seed() {
        let spec = GrainSpec {
            amount: 0.3,
            size_um: 15.0,
            chroma: 0.3,
        };
        let mut a = ImageF32::filled(64, 64, [0.4; 3]);
        let mut b = a.clone();
        apply(&mut a, &spec, 1.0, 42, false);
        apply(&mut b, &spec, 1.0, 42, false);
        assert_eq!(a.data, b.data);
    }
}
