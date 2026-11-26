//! Separable Gaussian blur (parallel), with automatic downscaling for big sigmas.

use crate::image::{ImageF32, MaskF32};
use rayon::prelude::*;

fn kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as usize;
    let mut k: Vec<f32> = (0..=2 * r)
        .map(|i| {
            let x = i as f32 - r as f32;
            (-x * x / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let s: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k
}

/// Generic separable blur over `ch` interleaved channels.
fn blur_buf(data: &mut [f32], w: usize, h: usize, ch: usize, sigma: f32) {
    if sigma < 0.3 || w == 0 || h == 0 {
        return;
    }
    let k = kernel(sigma);
    let r = (k.len() / 2) as isize;
    // Horizontal
    let src = data.to_vec();
    data.par_chunks_exact_mut(w * ch).enumerate().for_each(|(y, row)| {
        let srow = &src[y * w * ch..(y + 1) * w * ch];
        for x in 0..w {
            for c in 0..ch {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    let sx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                    acc += srow[sx * ch + c] * kv;
                }
                row[x * ch + c] = acc;
            }
        }
    });
    // Vertical
    let src = data.to_vec();
    data.par_chunks_exact_mut(w * ch).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            for c in 0..ch {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    let sy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
                    acc += src[(sy * w + x) * ch + c] * kv;
                }
                row[x * ch + c] = acc;
            }
        }
    });
}

/// Gaussian blur at full resolution. Large sigmas are computed on a downscaled
/// copy and upsampled (visually identical for smooth results, much faster).
pub fn gaussian(img: &ImageF32, sigma: f32) -> ImageF32 {
    let factor = if sigma > 8.0 { ((sigma / 4.0) as usize).max(1) } else { 1 };
    if factor > 1 {
        let mut small = img.downsample(factor);
        let (w, h) = (small.w, small.h);
        blur_buf(&mut small.data, w, h, 3, sigma / factor as f32);
        small.resize(img.w, img.h)
    } else {
        let mut out = img.clone();
        blur_buf(&mut out.data, img.w, img.h, 3, sigma);
        out
    }
}

pub fn gaussian_in_place(img: &mut ImageF32, sigma: f32) {
    let (w, h) = (img.w, img.h);
    blur_buf(&mut img.data, w, h, 3, sigma);
}

pub fn gaussian_mask(m: &mut MaskF32, sigma: f32) {
    let (w, h) = (m.w, m.h);
    blur_buf(&mut m.data, w, h, 1, sigma);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_preserves_mean() {
        let mut img = ImageF32::new(64, 64);
        img.set(32, 32, [100.0, 50.0, 10.0]);
        let b = gaussian(&img, 3.0);
        let sum: f32 = b.data.iter().step_by(3).sum();
        assert!((sum - 100.0).abs() < 0.5, "sum {sum}");
    }
}
