//! OMNI-style creative filter: a crystal or wand held in front of *any* lens,
//! at a chosen spot around the frame edge. The element is a few cm from the
//! lens, so it is far out of focus: it smears a refracted / mirrored copy of
//! the scene over a soft region next to the edge it sits on. A recipe can
//! hold several (the UI offers a left and a right wand).
//!
//! A wand is a refraction *mode* (how the copy is folded) plus a tint. The
//! built-in wands are [`WANDS`]; any spec can override `width`, `tint`,
//! `tint2` (gradient across the band) and `spectral` in its params, so new
//! wands need no code.
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

/// Refraction modes (`final.frag` uses the same numbers).
pub const MODE_RODS: u32 = 0;
pub const MODE_CYLINDER: u32 = 1;
pub const MODE_FACET: u32 = 2;
pub const MODE_SWIRL: u32 = 3;
pub const MODE_BAND: u32 = 4;
pub const MODE_RIPPLE: u32 = 5;
pub const MODE_REPEAT: u32 = 6;
pub const MODE_STRETCH: u32 = 7;
pub const MODE_BULGE: u32 = 8;
pub const MODE_PERFORATED: u32 = 9;
pub const MODE_DIAMOND: u32 = 10;
pub const MODE_FILM: u32 = 11;
pub const MODE_TRIANGLE: u32 = 12;
pub const MODE_WINDOW: u32 = 13;

/// A built-in wand.
pub struct Wand {
    pub id: &'static str,
    pub label: &'static str,
    /// UI group: "crystal", "mirror" or "film".
    pub family: &'static str,
    pub mode: u32,
    /// Half-width of the affected band along the edge (half-diagonal units).
    pub width: f32,
    /// Tint at one side of the band and at the other (gradient across).
    pub tint: [f32; 3],
    pub tint2: [f32; 3],
    /// Rainbow tint across the band instead of `tint`..`tint2`.
    pub spectral: bool,
    /// Dispersion of bright light ("fire"): light sources the wand catches
    /// fan out into rainbow streaks; plain scene stays neutral.
    pub fire: f32,
}

const W: [f32; 3] = [1.0, 1.0, 1.0];

const fn wand(
    id: &'static str,
    label: &'static str,
    family: &'static str,
    mode: u32,
    width: f32,
    fire: f32,
    tint: [f32; 3],
    tint2: [f32; 3],
) -> Wand {
    Wand {
        id,
        label,
        family,
        mode,
        width,
        tint,
        tint2,
        spectral: false,
        fire,
    }
}

const fn spectral(id: &'static str, label: &'static str, mode: u32, width: f32, fire: f32) -> Wand {
    Wand {
        spectral: true,
        ..wand(id, label, "crystal", mode, width, fire, W, W)
    }
}

const fn crystal(id: &'static str, label: &'static str, mode: u32, width: f32, fire: f32) -> Wand {
    wand(id, label, "crystal", mode, width, fire, W, W)
}

const fn mirror(
    id: &'static str,
    label: &'static str,
    mode: u32,
    width: f32,
    a: [f32; 3],
    b: [f32; 3],
) -> Wand {
    // metal reflects, barely disperses
    wand(id, label, "mirror", mode, width, 0.25, a, b)
}

const fn diamond(id: &'static str, label: &'static str, a: [f32; 3], b: [f32; 3]) -> Wand {
    wand(id, label, "film", MODE_DIAMOND, 0.6, 0.6, a, b)
}

/// Translucent colour gel: an in-place colour cast, no refraction.
const fn gel(id: &'static str, label: &'static str, c: [f32; 3]) -> Wand {
    wand(id, label, "film", MODE_FILM, 0.7, 0.0, c, c)
}

const GOLD: [f32; 3] = [1.35, 1.05, 0.55];
const GOLD2: [f32; 3] = [1.25, 0.9, 0.5];
const SILVER: [f32; 3] = [1.08, 1.12, 1.2];
const SILVER2: [f32; 3] = [1.0, 1.04, 1.12];

/// Built-in wands, in UI order (grouped by family). The first is the
/// fallback for unknown ids. Modelled on the kinds of element sold for
/// OMNI-style systems: clear and coated crystals, metal mirrors, colour films.
pub const WANDS: &[Wand] = &[
    // clear crystal: shape only, strongest fire on light sources
    crystal("rods", "Crystal rods", MODE_RODS, 0.3, 1.0),
    crystal("cylinder", "Cylinder", MODE_CYLINDER, 0.75, 1.0),
    crystal("stretch", "Stretched glass", MODE_STRETCH, 0.55, 0.8),
    crystal("swirl", "Scalloped S", MODE_SWIRL, 0.4, 1.0),
    crystal("window", "Scalloped window", MODE_WINDOW, 0.6, 1.5),
    crystal("facet", "Faceted prism", MODE_FACET, 0.35, 1.6),
    crystal("triangle", "Triangle prism", MODE_TRIANGLE, 0.45, 1.4),
    crystal("repeater", "Repeater", MODE_REPEAT, 0.6, 0.8),
    crystal("ripple", "Ripple", MODE_RIPPLE, 0.5, 1.0),
    // coated crystal: spectral colour
    spectral("rainbow", "Rainbow", MODE_BAND, 0.55, 0.6),
    spectral("wedge", "Spectral wedge", MODE_SWIRL, 0.45, 1.2),
    spectral("regal", "Bold facets", MODE_FACET, 0.5, 1.0),
    wand(
        "dichro",
        "Dichroic",
        "crystal",
        MODE_CYLINDER,
        0.5,
        0.8,
        [0.55, 1.15, 1.3],
        [1.3, 0.6, 1.15],
    ),
    // metal mirrors: curved reflected borders, a bulge, a mirror with holes
    mirror("gold", "Gold concave", MODE_CYLINDER, 0.7, GOLD, GOLD2),
    mirror(
        "silver",
        "Silver concave",
        MODE_CYLINDER,
        0.7,
        SILVER,
        SILVER2,
    ),
    mirror("convex", "Silver convex", MODE_BULGE, 0.5, SILVER, SILVER2),
    mirror(
        "perf_gold",
        "Gold perforated",
        MODE_PERFORATED,
        0.6,
        GOLD,
        GOLD2,
    ),
    mirror(
        "perf_silver",
        "Silver perforated",
        MODE_PERFORATED,
        0.6,
        SILVER,
        SILVER2,
    ),
    // films: textured reflective diamond films, translucent colour gels
    diamond(
        "diamond_blue",
        "Blue diamond",
        [0.55, 0.85, 1.45],
        [0.75, 1.0, 1.35],
    ),
    diamond(
        "diamond_gold",
        "Gold diamond",
        [1.45, 1.1, 0.5],
        [1.35, 0.95, 0.45],
    ),
    diamond(
        "diamond_pink",
        "Pink diamond",
        [1.45, 0.55, 0.95],
        [1.3, 0.5, 1.1],
    ),
    gel("gel_red", "Red gel", [1.0, 0.3, 0.25]),
    gel("gel_orange", "Orange gel", [1.0, 0.6, 0.25]),
    gel("gel_yellow", "Yellow gel", [1.0, 0.95, 0.3]),
    gel("gel_green", "Green gel", [0.35, 1.0, 0.4]),
    gel("gel_blue", "Blue gel", [0.3, 0.5, 1.0]),
];

/// x − floor(x) (GLSL `fract`; Rust's `fract` keeps the sign).
fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Diamond-film texture 0..1 at (along, across). Mirrored in `final.frag`.
pub fn diamond_tex(al: f32, ac: f32) -> f32 {
    let tri = |x: f32| (2.0 * fract(x) - 1.0).abs();
    0.5 * (tri((al + ac) / 0.05) + tri((al - ac) / 0.05))
}

/// Perforated mirror: 1 inside a hole (scene passes straight through); the
/// mirror is far out of focus, so the hole edge is soft.
pub fn hole(al: f32, ac: f32) -> f32 {
    let cell = 0.09;
    let u = fract(al / cell) - 0.5;
    let v = fract(ac / cell) - 0.5;
    1.0 - smoothstep(0.006, 0.042, (u * u + v * v).sqrt() * cell)
}

pub fn wand_def(id: &str) -> &'static Wand {
    WANDS.iter().find(|w| w.id == id).unwrap_or(&WANDS[0])
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Omni {
    /// One of the `MODE_*` constants.
    pub mode: u32,
    /// Where around the frame, degrees: 0 right, 90 bottom, 180 left, 270 top.
    pub position: f32,
    /// How far into the frame (half-diagonal units, 0.1..1).
    pub reach: f32,
    pub strength: f32,
    pub width: f32,
    pub tint: [f32; 3],
    pub tint2: [f32; 3],
    pub spectral: bool,
    pub fire: f32,
}

fn finite_tint(c: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    if c.iter().all(|v| v.is_finite()) {
        c.map(|v| v.clamp(0.0, 3.0))
    } else {
        d
    }
}

impl Omni {
    pub fn from_spec(s: &EffectSpec) -> Self {
        let w = wand_def(s.s("wand", "rods"));
        let tint = finite_tint(s.rgb("tint", w.tint), w.tint);
        Omni {
            mode: w.mode,
            position: s.f("position", 0.0),
            reach: s.f("reach", 0.45).clamp(0.1, 1.0),
            strength: s.strength.clamp(0.0, 1.5),
            width: s.f("width", w.width).clamp(0.05, 1.5),
            tint,
            // a custom single `tint` colours the whole band
            tint2: finite_tint(
                s.rgb(
                    "tint2",
                    if s.params.contains_key("tint") {
                        tint
                    } else {
                        w.tint2
                    },
                ),
                tint,
            ),
            spectral: match s.params.get("spectral") {
                Some(toml::Value::Boolean(b)) => *b,
                _ => w.spectral,
            },
            fire: s.f("fire", w.fire).clamp(0.0, 3.0),
        }
    }

    fn width(&self) -> f32 {
        self.width
    }

    /// Colour multiplier at `across` (half-diagonal units from the anchor).
    fn tint_at(&self, across: f32) -> [f32; 3] {
        let h = 0.5 + across / (2.0 * self.width);
        if self.spectral {
            spectrum(h).map(|v| 0.25 + 1.3 * v)
        } else {
            crate::color::mix3(self.tint, self.tint2, h.clamp(0.0, 1.0))
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
        let mut m = (1.0 - smoothstep(0.35 * self.reach, self.reach, along))
            * (-across * across * 2.0).exp();
        if self.mode == MODE_PERFORATED {
            m *= 1.0 - hole(along, across * self.width());
        } else if self.mode == MODE_WINDOW {
            // clear centre, flaring scalloped rim
            m *= smoothstep(0.2, 0.45, across.abs());
        }
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
        let mut fire = [0.0f32; 3];
        for i in 0..TAPS {
            // s in -0.5..0.5; per-wand tap position + dispersion axis (R one way, B the other)
            let s = (i as f32 + 0.5) / TAPS as f32 - 0.5;
            let (q, disp) = match self.mode {
                MODE_REPEAT => {
                    let seg = 0.1;
                    let cell = (across / seg).floor();
                    (
                        at(along + s * 0.02, cell * seg * 0.35 + (across - cell * seg)),
                        [t[0], t[1]],
                    )
                }
                MODE_STRETCH => (at(along * 0.9, across * 0.4 + s * 0.25), [t[0], t[1]]),
                MODE_BULGE => {
                    let cr = 0.45 * r;
                    let dl = along - cr;
                    let rr = (dl * dl + across * across).sqrt() / self.width;
                    let k = 0.35 + 0.5 * rr * rr + s * 0.03;
                    (at(cr - dl * k, across * k), [n[0], n[1]])
                }
                MODE_DIAMOND => {
                    let tx = diamond_tex(along, across);
                    (
                        at(along + 0.02 * tx + s * 0.01, across + 0.02 * (1.0 - tx)),
                        [t[0], t[1]],
                    )
                }
                MODE_FILM => (at(along, across + s * 0.01), [t[0], t[1]]),
                MODE_TRIANGLE => {
                    // two faces, two reflections at different depths
                    let (f, o) = if i % 2 == 0 {
                        (1.0, 0.08)
                    } else {
                        (0.6, -0.08)
                    };
                    (at(qa * f + s * 0.02, across + o), [n[0], n[1]])
                }
                MODE_RIPPLE => (
                    at(
                        qa + 0.05 * (across * 40.0 + s * 6.0).sin(),
                        across + s * 0.1,
                    ),
                    [n[0], n[1]],
                ),
                1 => (at(qa, across + s * 0.16), [n[0], n[1]]),
                MODE_FACET | MODE_WINDOW => {
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
                MODE_BAND | MODE_PERFORATED => (at(qa, across + s * 0.12), [n[0], n[1]]),
                _ => (at(qa + s * 0.12, across), [t[0], t[1]]),
            };
            let dsp = 0.006 * hd;
            let rr = sample(q[0] + disp[0] * dsp, q[1] + disp[1] * dsp)[0];
            let mid = sample(q[0], q[1]);
            let gg = mid[1];
            // bright light through the crystal splits by wavelength: each tap
            // gets its own hue, so a light source smears into a rainbow
            let y = 0.2126 * mid[0] + 0.7152 * mid[1] + 0.0722 * mid[2];
            let hl = smoothstep(0.6, 1.0, y) * y;
            let sp = spectrum((i as f32 + 0.5) / TAPS as f32);
            for k in 0..3 {
                fire[k] += sp[k] * hl;
            }
            let bb = sample(q[0] - disp[0] * dsp, q[1] - disp[1] * dsp)[2];
            acc[0] += rr;
            acc[1] += gg;
            acc[2] += bb;
        }
        let mut c = acc.map(|v| v / TAPS as f32);
        let tint = self.tint_at(across);
        // only the colour of the fire: an evenly bright area stays neutral
        let fm = (fire[0] + fire[1] + fire[2]) / 3.0;
        let fk = self.fire * 3.0 / TAPS as f32;
        // diamond film: reflective texture sparkles brighter / darker
        let tex = if self.mode == MODE_DIAMOND {
            0.85 + 0.3 * diamond_tex(along, across)
        } else {
            1.0
        };
        for k in 0..3 {
            c[k] = (c[k] * tint[k] * tex + fk * (fire[k] - fm)).max(0.0);
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

    fn spec(t: &str) -> EffectSpec {
        toml::from_str(&format!("kind = \"omni\"\n[params]\n{t}")).unwrap()
    }

    fn omni(position: f32) -> Omni {
        Omni {
            position,
            ..Omni::from_spec(&spec("wand = \"rods\""))
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
        for w in WANDS {
            let o = Omni {
                position: 45.0,
                reach: 0.6,
                ..Omni::from_spec(&spec(&format!("wand = \"{}\"", w.id)))
            };
            let c = o.refract([38.0, 28.0], 40.0, 30.0, |x, y| img.sample(x, y));
            assert!(
                c.iter().all(|v| v.is_finite() && *v >= 0.0),
                "wand {}: {c:?}",
                w.id
            );
        }
    }

    #[test]
    fn wand_ids_are_unique_and_unknown_falls_back() {
        let mut ids: Vec<_> = WANDS.iter().map(|w| w.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), WANDS.len());
        assert_eq!(Omni::from_spec(&spec("wand = \"nope\"")).mode, MODE_RODS);
    }

    #[test]
    fn colour_wands_tint_and_params_override() {
        let grey = |_: f32, _: f32| [0.4f32, 0.4, 0.4];
        let gold = Omni::from_spec(&spec("wand = \"gold\""));
        let c = gold.refract([380.0, 150.0], 400.0, 300.0, grey);
        assert!(c[0] > c[2] * 1.5, "gold is warm: {c:?}");
        let clear = Omni::from_spec(&spec("wand = \"cylinder\""));
        let c = clear.refract([380.0, 150.0], 400.0, 300.0, grey);
        assert!(
            (c[0] - c[2]).abs() < 1e-5,
            "clear crystal is neutral: {c:?}"
        );
        // a custom single tint colours the whole band; width overrides
        let custom = Omni::from_spec(&spec(
            "wand = \"cylinder\"\ntint = [0.5, 1.0, 1.5]\nwidth = 0.2",
        ));
        assert_eq!(custom.tint2, [0.5, 1.0, 1.5]);
        assert_eq!(custom.width, 0.2);
        let bad = Omni::from_spec(&spec(
            "wand = \"gold\"\ntint = [nan, 1.0, 1.0]\nwidth = nan",
        ));
        assert_eq!(bad.tint, wand_def("gold").tint);
        assert_eq!(bad.width, wand_def("gold").width);
    }

    #[test]
    fn crystal_fans_a_light_source_into_a_rainbow() {
        let (w, h) = (400.0, 300.0);
        let o = Omni::from_spec(&spec("wand = \"rods\""));
        // dark scene with one small bright light inside the wand's strip
        let lamp = |x: f32, y: f32| {
            let d = ((x - 330.0).powi(2) + (y - 150.0).powi(2)).sqrt();
            if d < 14.0 {
                [3.0f32; 3]
            } else {
                [0.05; 3]
            }
        };
        let flat = |_: f32, _: f32| [0.05f32; 3];
        let mut spread = 0.0f32;
        for py in (130..170).step_by(4) {
            for px in (360..398).step_by(4) {
                let c = o.refract([px as f32, py as f32], w, h, lamp);
                spread = spread.max(c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]));
                let f = o.refract([px as f32, py as f32], w, h, flat);
                assert!((f[0] - f[2]).abs() < 1e-5, "no light, no rainbow: {f:?}");
            }
        }
        assert!(
            spread > 0.2,
            "light source should split into colour: {spread}"
        );
        // metal barely disperses
        let gold = Omni {
            tint: W,
            tint2: W,
            ..Omni::from_spec(&spec("wand = \"gold\""))
        };
        assert!(gold.fire < o.fire);
    }

    #[test]
    fn families_and_new_shapes() {
        assert!(WANDS
            .iter()
            .all(|w| ["crystal", "mirror", "film"].contains(&w.family)));
        // a gel tints in place and does not disperse
        let red = Omni::from_spec(&spec("wand = \"gel_red\""));
        assert_eq!(red.fire, 0.0);
        let c = red.refract([390.0, 150.0], 400.0, 300.0, |_, _| [0.5f32; 3]);
        assert!(c[0] > 2.0 * c[1], "red gel: {c:?}");
        // perforated mirror lets the scene through its holes
        let perf = Omni::from_spec(&spec("wand = \"perf_silver\""));
        let solid = Omni {
            mode: MODE_BAND,
            ..perf
        };
        let (mut lo, mut hi) = (f32::MAX, 0.0f32);
        for x in 370..400 {
            for y in 120..180 {
                let p = [x as f32, y as f32];
                let w = perf.weight(p, 400.0, 300.0) / solid.weight(p, 400.0, 300.0).max(1e-6);
                lo = lo.min(w);
                hi = hi.max(w);
            }
        }
        assert!(lo < 0.2 && hi > 0.9, "holes: {lo}..{hi}");
        // scalloped window keeps its centre clear
        let win = Omni::from_spec(&spec("wand = \"window\""));
        assert!(win.weight([399.0, 150.0], 400.0, 300.0) < 0.05);
        assert!(win.weight([399.0, 60.0], 400.0, 300.0) > 0.2);
    }
}
