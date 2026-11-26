//! Film emulation: exposure-space transforms (IR, halation), characteristic
//! curve (tone), grain and print. Tone + print are pure per-pixel functions so
//! they can be baked into 3D LUTs for the GPU preview.

pub mod grain;
pub mod halation;
pub mod infrared;

use crate::color::{kelvin_gains, luma, mat_mul, saturate, Mat3};
use crate::preset::{FilmKind, FilmPreset, PrintSpec, ToneSpec};
use crate::recipe::UserParams;

/// Log2 exposure range covered by the preview LUT shaper, relative to 0.18.
pub const LOG_MIN: f32 = -10.0;
pub const LOG_MAX: f32 = 6.0;
const MID_GREY: f32 = 0.18;
/// Neutral contrast: makes contrast=1.0 roughly match sRGB slope at mid grey.
const SLOPE_UNIT: f32 = 0.55;
/// logistic(B_OFFSET) ≈ 0.46 so mid grey lands near sRGB(0.18).
const B_OFFSET: f32 = -0.16;

/// Precomputed tone transform (per-pixel, LUT-able).
#[derive(Clone, Debug)]
pub struct ToneXform {
    input: Mat3,
    gains: [f32; 3],
    crosstalk: Mat3,
    contrast: [f32; 3],
    pivot: [f32; 3],
    black: [f32; 3],
    white: [f32; 3],
    mono: Option<[f32; 3]>,
    saturation: f32,
    print: PrintSpec,
    print_fade: f32,
    print_sat: f32,
    pub is_mono: bool,
}

fn normalize_rows(m: &Mat3) -> Mat3 {
    // Keep neutrals neutral: each row sums to 1.
    let mut o = *m;
    for r in o.iter_mut() {
        let s: f32 = r.iter().sum();
        if s.abs() > 1e-6 {
            for v in r.iter_mut() {
                *v /= s;
            }
        }
    }
    o
}

impl ToneXform {
    pub fn new(film: &FilmPreset, p: &UserParams) -> Self {
        let t: &ToneSpec = &film.tone;
        let mut gains = if t.balance_kelvin > 0.0 {
            let k = kelvin_gains(t.balance_kelvin);
            [1.0 / k[0], 1.0 / k[1], 1.0 / k[2]]
        } else {
            [1.0; 3]
        };
        // Character pad warmth.
        let w = p.warmth.clamp(-1.0, 1.0);
        gains[0] *= 1.0 + 0.10 * w;
        gains[2] *= 1.0 - 0.10 * w;
        let punch = p.punch.clamp(-1.0, 1.0);
        let cmul = 1.0 + 0.22 * punch;
        let mono_sum: f32 = t.mono.iter().sum();
        let is_mono = mono_sum > 0.0 || matches!(film.kind, FilmKind::BwNegative | FilmKind::BwInfrared);
        let mono = if mono_sum > 0.0 {
            Some([t.mono[0] / mono_sum, t.mono[1] / mono_sum, t.mono[2] / mono_sum])
        } else if is_mono {
            Some([0.2126, 0.7152, 0.0722])
        } else {
            None
        };
        ToneXform {
            input: t.input_matrix,
            gains,
            crosstalk: normalize_rows(&t.crosstalk),
            contrast: t.contrast.map(|c| c * cmul),
            pivot: t.pivot,
            black: t.black,
            white: t.white,
            mono,
            saturation: t.saturation * (1.0 + 0.15 * punch),
            print: film.print.clone(),
            print_fade: (film.print.fade + (-punch).max(0.0) * 0.06).clamp(0.0, 0.4),
            print_sat: film.print.saturation.unwrap_or(1.0),
            is_mono,
        }
    }

    /// Linear scene light → display-encoded film image (before grain).
    #[inline]
    pub fn develop(&self, lin: [f32; 3]) -> [f32; 3] {
        let mut e = mat_mul(&self.input, lin);
        for c in 0..3 {
            e[c] = (e[c] * self.gains[c]).max(1e-8);
        }
        if let Some(m) = self.mono {
            let y = m[0] * e[0] + m[1] * e[1] + m[2] * e[2];
            e = [y; 3];
        }
        let le = [
            (e[0] / MID_GREY).log2(),
            (e[1] / MID_GREY).log2(),
            (e[2] / MID_GREY).log2(),
        ];
        let le = mat_mul(&self.crosstalk, le);
        let mut y = [0.0; 3];
        for c in 0..3 {
            let x = self.contrast[c] * SLOPE_UNIT * (le[c] - self.pivot[c]) + B_OFFSET;
            let s = 1.0 / (1.0 + (-x).exp());
            y[c] = self.black[c] + (self.white[c] - self.black[c]) * s;
        }
        if !self.is_mono && (self.saturation - 1.0).abs() > 1e-4 {
            y = saturate(y, self.saturation);
        }
        y.map(|v| v.clamp(0.0, 1.0))
    }

    /// Print / finishing stage (display space, after grain).
    #[inline]
    pub fn print(&self, y: [f32; 3]) -> [f32; 3] {
        let f = self.print_fade;
        let mut o = y.map(|v| f + (1.0 - f) * v);
        let l = luma(o).clamp(0.0, 1.0);
        let sh = (1.0 - l) * (1.0 - l);
        let hl = l * l;
        for c in 0..3 {
            o[c] += self.print.shadow_tint[c] * sh + self.print.highlight_tint[c] * hl;
        }
        if (self.print_sat - 1.0).abs() > 1e-4 {
            o = saturate(o, self.print_sat);
        }
        o.map(|v| v.clamp(0.0, 1.0))
    }
}

/// Map linear value to LUT coordinate (0..1) via log2 shaper.
#[inline]
pub fn shaper(lin: f32) -> f32 {
    let l = (lin.max(1e-8) / MID_GREY).log2();
    ((l - LOG_MIN) / (LOG_MAX - LOG_MIN)).clamp(0.0, 1.0)
}

#[inline]
pub fn unshaper(u: f32) -> f32 {
    MID_GREY * (LOG_MIN + u * (LOG_MAX - LOG_MIN)).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    #[test]
    fn tone_is_monotonic_for_all_films() {
        let cat = Catalog::builtin();
        for f in cat.films.values() {
            let t = ToneXform::new(f, &UserParams::default());
            let mut prev = -1.0;
            for i in 0..200 {
                let v = unshaper(i as f32 / 199.0);
                let y = luma(t.develop([v, v, v]));
                assert!(y >= prev - 1e-4, "{} not monotonic at {i}: {y} < {prev}", f.id);
                prev = y;
            }
        }
    }

    #[test]
    fn shaper_roundtrip() {
        for i in 1..100 {
            let u = i as f32 / 100.0;
            assert!((shaper(unshaper(u)) - u).abs() < 1e-5);
        }
    }
}
