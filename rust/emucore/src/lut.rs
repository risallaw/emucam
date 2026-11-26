//! LUT baking for the GPU preview. The preview shader does:
//!   lin → shaper(log2) → LUT_DEVELOP (3D) → grain → LUT_PRINT (3D)
//! Both LUTs are baked from the exact same `ToneXform` the CPU render uses.

use crate::film::{shaper, unshaper, ToneXform};
use crate::recipe::Resolved;
use half::f16;

pub const PREVIEW_LUT_SIZE: usize = 48;

/// RGB float LUT, red fastest: index = (b*N + g)*N + r.
pub struct Lut3 {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

impl Lut3 {
    fn bake(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Self {
        let mut data = Vec::with_capacity(size * size * size);
        let n = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(f([r as f32 / n, g as f32 / n, b as f32 / n]));
                }
            }
        }
        Lut3 { size, data }
    }

    /// Trilinear lookup (inputs 0..1).
    pub fn lookup(&self, c: [f32; 3]) -> [f32; 3] {
        let n = (self.size - 1) as f32;
        let p = c.map(|v| v.clamp(0.0, 1.0) * n);
        let i = p.map(|v| (v.floor() as usize).min(self.size - 2));
        let t = [p[0] - i[0] as f32, p[1] - i[1] as f32, p[2] - i[2] as f32];
        let at = |r: usize, g: usize, b: usize| self.data[(b * self.size + g) * self.size + r];
        // Fetch the 8 cell corners once, then interpolate all channels.
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        };
        let (r, g, b) = (i[0], i[1], i[2]);
        let x00 = lerp(at(r, g, b), at(r + 1, g, b), t[0]);
        let x10 = lerp(at(r, g + 1, b), at(r + 1, g + 1, b), t[0]);
        let x01 = lerp(at(r, g, b + 1), at(r + 1, g, b + 1), t[0]);
        let x11 = lerp(at(r, g + 1, b + 1), at(r + 1, g + 1, b + 1), t[0]);
        lerp(lerp(x00, x10, t[1]), lerp(x01, x11, t[1]), t[2])
    }

    /// RGBA16F bytes (little endian) for `glTexImage3D(GL_RGBA16F, GL_HALF_FLOAT)`.
    pub fn to_rgba_f16_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 8);
        for c in &self.data {
            for v in [c[0], c[1], c[2], 1.0] {
                out.extend_from_slice(&f16::from_f32(v).to_le_bytes());
            }
        }
        out
    }

    /// Adobe .cube text (for inspection in other tools).
    pub fn to_cube(&self, title: &str) -> String {
        let mut s = format!("TITLE \"{title}\"\nLUT_3D_SIZE {}\n", self.size);
        for c in &self.data {
            s.push_str(&format!("{:.6} {:.6} {:.6}\n", c[0], c[1], c[2]));
        }
        s
    }
}

pub struct FilmLuts {
    pub develop: Lut3,
    pub print: Lut3,
}

/// Bake develop + print LUTs. Without a film, develop = sRGB encode, print = identity.
pub fn bake(res: &Resolved, size: usize) -> FilmLuts {
    match &res.film {
        Some(f) => {
            let t = ToneXform::new(f, &res.params);
            FilmLuts {
                develop: Lut3::bake(size, |s| t.develop(s.map(unshaper))),
                print: Lut3::bake(size, |y| t.print(y)),
            }
        }
        None => FilmLuts {
            develop: Lut3::bake(size, |s| {
                s.map(|v| crate::color::linear_to_srgb(unshaper(v)).clamp(0.0, 1.0))
            }),
            print: Lut3::bake(size, |y| y),
        },
    }
}

/// Max abs error of LUT vs analytic develop over a probe grid.
pub fn develop_error(res: &Resolved, luts: &FilmLuts) -> f32 {
    let Some(f) = &res.film else { return 0.0 };
    let t = ToneXform::new(f, &res.params);
    let mut worst = 0.0f32;
    let n = 23;
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let lin = [r, g, b].map(|i| 0.002 + (i as f32 / (n - 1) as f32).powf(2.2) * 1.5);
                let exact = t.develop(lin);
                let approx = luts.develop.lookup(lin.map(shaper));
                for k in 0..3 {
                    worst = worst.max((exact[k] - approx[k]).abs());
                }
            }
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::recipe::{resolve, Recipe};

    #[test]
    fn preview_lut_matches_analytic_within_tolerance() {
        let cat = Catalog::builtin();
        for id in cat.films.keys() {
            let r = Recipe {
                film: Some(id.clone()),
                ..Default::default()
            };
            let res = resolve(&r, cat).unwrap();
            let luts = bake(&res, PREVIEW_LUT_SIZE);
            let e = develop_error(&res, &luts);
            assert!(e < 0.02, "{id}: LUT error {e}");
        }
    }
}
