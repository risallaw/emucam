//! OMNI-style creative filter: a crystal or wand held in front of *any* lens,
//! at a chosen spot around the frame edge. The element is a few cm from the
//! lens, so it is far out of focus: it smears a refracted / mirrored copy of
//! the scene over a soft region next to the edge it sits on.
//!
//! Wands: "rods" (crystal rods poking in from the edge → radial streaks),
//! "cylinder" (crystal cylinder along the edge → mirrored band), "facet"
//! (faceted crystal → fragmented duplicates), "swirl" (twisted refraction),
//! "rainbow" (dichroic wand → spectral band).
//!
//! Mirrored in `final.frag` (`omni`); keep both identical. Positions are in
//! pixels with lengths in half-diagonal units so preview and capture agree.

use crate::color::smoothstep;
use crate::image::ImageF32;
use crate::preset::{EffectSpec, Slot};

use super::{Effect, FxCtx};

pub const TAPS: usize = 8;
/// Softness of the refracted copy (Gaussian σ, half-diagonal units): the
/// wand is far out of focus. The preview reads a mip level of matching size.
pub const SOFT: f32 = 0.012;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Omni {
    /// 0 rods, 1 cylinder, 2 facet, 3 swirl, 4 rainbow.
    pub wand: u32,
    /// Where around the frame, degrees: 0 right, 90 bottom, 180 left, 270 top.
    pub position: f32,
    /// How far into the frame (half-diagonal units, 0.1..1).
    pub reach: f32,
    pub strength: f32,
}

pub fn wand_index(s: &str) -> u32 {
    match s {
        "cylinder" => 1,
        "facet" => 2,
        "swirl" => 3,
        "rainbow" => 4,
        _ => 0,
    }
}

impl Omni {
    pub fn from_spec(s: &EffectSpec) -> Self {
        Omni {
            wand: wand_index(s.s("wand", "rods")),
            position: s.f("position", 0.0),
            reach: s.f("reach", 0.45).clamp(0.1, 1.0),
            strength: s.strength.clamp(0.0, 1.5),
        }
    }

    /// Half-width of the affected band along the edge (half-diagonal units).
    fn width(&self) -> f32 {
        match self.wand {
            1 => 0.75,
            4 => 0.55,
            2 => 0.35,
            3 => 0.4,
            _ => 0.3,
        }
    }

    /// Anchor on the frame border, inward normal, tangent.
    fn frame(&self, w: f32, h: f32) -> ([f32; 2], [f32; 2], [f32; 2]) {
        let (anchor, out) = super::edge_point(self.position, w, h);
        let n = [-out[0], -out[1]];
        let tan = [-n[1], n[0]];
        (anchor, n, tan)
    }

    /// Blend weight at pixel p (0 = untouched).
    pub fn weight(&self, p: [f32; 2], w: f32, h: f32) -> f32 {
        let hd = 0.5 * (w * w + h * h).sqrt();
        let (a, n, t) = self.frame(w, h);
        let d = [p[0] - a[0], p[1] - a[1]];
        let along = (d[0] * n[0] + d[1] * n[1]) / hd;
        let across = (d[0] * t[0] + d[1] * t[1]) / hd / self.width();
        let m = (1.0 - smoothstep(0.35 * self.reach, self.reach, along))
            * (-across * across * 2.0).exp();
        (m * self.strength * 0.9).min(1.0)
    }

    /// Refracted colour at pixel p, from `sample(px, py)` of the scene.
    pub fn refract(
        &self,
        p: [f32; 2],
        w: f32,
        h: f32,
        sample: impl Fn(f32, f32) -> [f32; 3],
    ) -> [f32; 3] {
        let hd = 0.5 * (w * w + h * h).sqrt();
        let (a, n, t) = self.frame(w, h);
        let d = [p[0] - a[0], p[1] - a[1]];
        let along = (d[0] * n[0] + d[1] * n[1]) / hd;
        let across = (d[0] * t[0] + d[1] * t[1]) / hd;
        // mirror the strip just inside the wand back onto it (compressed)
        let r = self.reach;
        let qa = 0.5 * r + 0.6 * (0.5 * r - along);
        let at = |al: f32, ac: f32| {
            [
                a[0] + (n[0] * al + t[0] * ac) * hd,
                a[1] + (n[1] * al + t[1] * ac) * hd,
            ]
        };
        let mut acc = [0.0f32; 3];
        for i in 0..TAPS {
            // s in -0.5..0.5; per-wand tap position + dispersion axis (R one way, B the other)
            let s = (i as f32 + 0.5) / TAPS as f32 - 0.5;
            let (q, disp) = match self.wand {
                1 => (at(qa, across + s * 0.16), [n[0], n[1]]),
                2 => {
                    let k = (i % 4) as f32 * std::f32::consts::FRAC_PI_2 + 0.6;
                    let o = 0.05 + 0.02 * (i / 4) as f32;
                    (at(qa + k.sin() * o, across + k.cos() * o), [t[0], t[1]])
                }
                3 => {
                    let ang = 2.2 * (1.0 - along / r).clamp(0.0, 1.0) + s * 0.35;
                    let (sn, cs) = ang.sin_cos();
                    (
                        at(qa * cs - across * sn, qa * sn + across * cs),
                        [t[0], t[1]],
                    )
                }
                4 => (at(qa, across + s * 0.12), [n[0], n[1]]),
                _ => (at(qa + s * 0.12, across), [t[0], t[1]]),
            };
            let dsp = 0.006 * hd;
            let rr = sample(q[0] + disp[0] * dsp, q[1] + disp[1] * dsp)[0];
            let gg = sample(q[0], q[1])[1];
            let bb = sample(q[0] - disp[0] * dsp, q[1] - disp[1] * dsp)[2];
            acc[0] += rr;
            acc[1] += gg;
            acc[2] += bb;
        }
        let mut c = acc.map(|v| v / TAPS as f32);
        if self.wand == 4 {
            let tint = spectrum(0.5 + across / (2.0 * self.width()));
            for k in 0..3 {
                c[k] *= 0.25 + 1.3 * tint[k];
            }
        }
        // light scattered inside the crystal: a little brighter and hazier
        let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        c.map(|v| v * 1.08 + 0.12 * y + 0.01)
    }
}

/// Smooth hue ramp 0..1 → rgb (rainbow wand tint).
pub fn spectrum(h: f32) -> [f32; 3] {
    let f = |o: f32| (((h + o).rem_euclid(1.0) * 6.0 - 3.0).abs() - 1.0).clamp(0.0, 1.0);
    [f(0.0), f(2.0 / 3.0), f(1.0 / 3.0)]
}

pub struct OmniFx(pub EffectSpec);
impl Effect for OmniFx {
    fn slot(&self) -> Slot {
        Slot::Optical
    }
    fn apply(&self, img: &mut ImageF32, _ctx: &FxCtx) {
        let o = Omni::from_spec(&self.0);
        if o.strength <= 0.0 {
            return;
        }
        let (w, h) = (img.w as f32, img.h as f32);
        let hd = 0.5 * (w * w + h * h).sqrt();
        let src = crate::blur::Blurred::new(img, SOFT * hd);
        img.par_map_pixels(|x, y, p| {
            let px = [x as f32 + 0.5, y as f32 + 0.5];
            let m = o.weight(px, w, h);
            if m <= 1e-3 {
                return;
            }
            let r = o.refract(px, w, h, |sx, sy| src.sample(sx, sy));
            for k in 0..3 {
                p[k] += (r[k] - p[k]) * m;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn omni(position: f32) -> Omni {
        Omni {
            wand: 0,
            position,
            reach: 0.45,
            strength: 1.0,
        }
    }

    #[test]
    fn acts_only_near_the_chosen_edge() {
        let (w, h) = (400.0, 300.0);
        let right = omni(0.0);
        assert!(right.weight([395.0, 150.0], w, h) > 0.5);
        assert!(
            right.weight([5.0, 150.0], w, h) < 1e-3,
            "opposite edge untouched"
        );
        assert!(
            right.weight([200.0, 150.0], w, h) < 0.05,
            "centre mostly untouched"
        );
        let top = omni(270.0);
        assert!(top.weight([200.0, 3.0], w, h) > 0.5);
        assert!(top.weight([395.0, 150.0], w, h) < 0.05);
    }

    #[test]
    fn effect_changes_only_the_wand_side() {
        let mut img = ImageF32::new(80, 60);
        img.par_map_pixels(|x, _, p| p.copy_from_slice(&[x as f32 / 80.0; 3]));
        let before = img.clone();
        let spec: EffectSpec =
            toml::from_str("kind = \"omni\"\n[params]\nwand = \"rods\"\nposition = 0.0").unwrap();
        OmniFx(spec).apply(
            &mut img,
            &FxCtx {
                rotation: 0,
                seed: 0,
                negative: false,
                mono: false,
            },
        );
        assert_eq!(img.get(2, 30), before.get(2, 30));
        assert_ne!(img.get(78, 30), before.get(78, 30));
    }

    #[test]
    fn every_wand_renders_finite() {
        let img = ImageF32::filled(40, 30, [0.4, 0.5, 0.6]);
        for wand in 0..5 {
            let o = Omni {
                wand,
                position: 45.0,
                reach: 0.6,
                strength: 1.0,
            };
            let c = o.refract([38.0, 28.0], 40.0, 30.0, |x, y| img.sample(x, y));
            assert!(
                c.iter().all(|v| v.is_finite() && *v >= 0.0),
                "wand {wand}: {c:?}"
            );
        }
    }
}
