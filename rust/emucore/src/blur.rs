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

/// Generic separable blur over `ch` interleaved channels, clamped borders.
///
/// Written as row-wide multiply-adds (for each tap: `out[j] += src[j+off]·k`)
/// so the inner loop vectorises; every output still sums its taps in the
/// same order from 0.0, so results are bit-identical to the per-pixel loop
/// (see `matches_naive_clamped_blur`). ~3× faster, one temp buffer
/// instead of two.
fn blur_buf(data: &mut [f32], w: usize, h: usize, ch: usize, sigma: f32) {
    if sigma < 0.3 || w == 0 || h == 0 {
        return;
    }
    let k = kernel(sigma);
    let r = k.len() / 2;
    let stride = w * ch;
    // Horizontal: data → tmp.
    let mut tmp = vec![0.0f32; data.len()];
    tmp.par_chunks_exact_mut(stride)
        .zip(data.par_chunks_exact(stride))
        .for_each(|(row, srow)| {
            if w > 2 * r {
                // interior x ∈ [r, w−r): no clamping needed
                let (j0, j1) = (r * ch, (w - r) * ch);
                for (i, &kv) in k.iter().enumerate() {
                    let src = &srow[i * ch..i * ch + (j1 - j0)];
                    for (a, &v) in row[j0..j1].iter_mut().zip(src) {
                        *a += v * kv;
                    }
                }
            }
            for x in (0..w).filter(|&x| x < r || x + r >= w) {
                for c in 0..ch {
                    let mut acc = 0.0;
                    for (i, kv) in k.iter().enumerate() {
                        let sx = (x + i).saturating_sub(r).min(w - 1);
                        acc += srow[sx * ch + c] * kv;
                    }
                    row[x * ch + c] = acc;
                }
            }
        });
    // Vertical: tmp → data (clamp the row, not the pixel).
    data.par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(y, row)| {
            row.fill(0.0);
            for (i, &kv) in k.iter().enumerate() {
                let sy = (y + i).saturating_sub(r).min(h - 1);
                for (a, &v) in row.iter_mut().zip(&tmp[sy * stride..(sy + 1) * stride]) {
                    *a += v * kv;
                }
            }
        });
}

/// Gaussian blur at full resolution. Large sigmas are computed on a downscaled
/// copy and upsampled (visually identical for smooth results, much faster).
pub fn gaussian(img: &ImageF32, sigma: f32) -> ImageF32 {
    let b = Blurred::new(img, sigma);
    if b.img.w == img.w && b.img.h == img.h {
        b.img
    } else {
        b.img.resize(img.w, img.h)
    }
}

/// A Gaussian blur kept at the reduced resolution it was computed at, read
/// in full-resolution pixel coordinates. Same values as [`gaussian`] at pixel
/// centres, without materialising the full-size copy (≈144 MB at 12 MP) when
/// the caller only samples it.
pub struct Blurred {
    img: ImageF32,
    sx: f32,
    sy: f32,
}

impl Blurred {
    pub fn new(img: &ImageF32, sigma: f32) -> Self {
        let factor = if sigma > 8.0 {
            ((sigma / 4.0) as usize).max(1)
        } else {
            1
        };
        let mut small = img.downsample(factor);
        let (w, h) = (small.w, small.h);
        blur_buf(&mut small.data, w, h, 3, sigma / factor as f32);
        Blurred {
            sx: w as f32 / img.w as f32,
            sy: h as f32 / img.h as f32,
            img: small,
        }
    }

    /// Bilinear sample at full-resolution pixel coordinates.
    #[inline]
    pub fn sample(&self, fx: f32, fy: f32) -> [f32; 3] {
        self.img.sample(fx * self.sx, fy * self.sy)
    }

    #[inline]
    pub fn sample_uv(&self, u: f32, v: f32) -> [f32; 3] {
        self.img.sample_uv(u, v)
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

    /// The original per-pixel formulation, kept as the reference.
    fn naive(data: &[f32], w: usize, h: usize, ch: usize, sigma: f32) -> Vec<f32> {
        let k = kernel(sigma);
        let r = (k.len() / 2) as isize;
        let mut a = data.to_vec();
        for y in 0..h {
            for x in 0..w {
                for c in 0..ch {
                    let mut acc = 0.0;
                    for (i, kv) in k.iter().enumerate() {
                        let sx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                        acc += data[(y * w + sx) * ch + c] * kv;
                    }
                    a[(y * w + x) * ch + c] = acc;
                }
            }
        }
        let mut b = a.clone();
        for y in 0..h {
            for x in 0..w {
                for c in 0..ch {
                    let mut acc = 0.0;
                    for (i, kv) in k.iter().enumerate() {
                        let sy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
                        acc += a[(sy * w + x) * ch + c] * kv;
                    }
                    b[(y * w + x) * ch + c] = acc;
                }
            }
        }
        b
    }

    #[test]
    fn matches_naive_clamped_blur() {
        // sizes below, at and above the kernel width (interior empty / tiny)
        for (w, h, ch, sigma) in [
            (37, 23, 3, 2.5),
            (5, 40, 1, 3.0),
            (64, 3, 3, 1.2),
            (13, 13, 1, 2.0),
        ] {
            let data: Vec<f32> = (0..w * h * ch)
                .map(|i| ((i * 7919) % 101) as f32 / 101.0)
                .collect();
            let mut got = data.clone();
            blur_buf(&mut got, w, h, ch, sigma);
            assert_eq!(got, naive(&data, w, h, ch, sigma), "{w}x{h}x{ch} σ{sigma}");
        }
    }

    #[test]
    fn blur_preserves_mean() {
        let mut img = ImageF32::new(64, 64);
        img.set(32, 32, [100.0, 50.0, 10.0]);
        let b = gaussian(&img, 3.0);
        let sum: f32 = b.data.iter().step_by(3).sum();
        assert!((sum - 100.0).abs() < 0.5, "sum {sum}");
    }
}

#[cfg(test)]
mod blurred_tests {
    use super::*;

    #[test]
    fn blurred_matches_gaussian_at_pixel_centres_without_the_full_copy() {
        let mut img = ImageF32::new(97, 61);
        img.par_map_pixels(|x, y, p| {
            p.copy_from_slice(&[
                ((x * 7 + y * 3) % 11) as f32 / 10.0,
                (x % 5) as f32 / 4.0,
                (y % 3) as f32 / 2.0,
            ])
        });
        for sigma in [3.0, 20.0] {
            let full = gaussian(&img, sigma);
            let b = Blurred::new(&img, sigma);
            for (x, y) in [(0, 0), (13, 40), (96, 60), (50, 30)] {
                let a = full.get(x, y);
                let c = b.sample(x as f32 + 0.5, y as f32 + 0.5);
                for k in 0..3 {
                    assert!(
                        (a[k] - c[k]).abs() < 1e-5,
                        "σ {sigma} ({x},{y}): {a:?} vs {c:?}"
                    );
                }
            }
        }
        assert!(
            Blurred::new(&img, 20.0).img.data.len() * 4 < img.data.len(),
            "kept small"
        );
    }
}
