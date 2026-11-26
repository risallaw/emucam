//! Effect registry. Each kind implements `Effect` and declares its physical
//! pipeline slot. Presets (film, lens, body) and users add effects by `kind`
//! name; unknown kinds are skipped (forward compatible). To add a new kind:
//! implement `Effect`, add a `build()` arm and its name to `KINDS`.

use crate::blur;
use crate::color::{luma, mix, mix3, smoothstep};
use crate::image::ImageF32;
use crate::noise::Rng;
use crate::preset::{EffectSpec, Slot};

pub const KINDS: &[&str] = &[
    "light_leak",
    "fog",
    "vignette",
    "haze",
    "flare",
    "prism",
    "dust",
    "scratches",
    "frame",
    "date_stamp",
    "toning",
    "color_shift",
];

pub struct FxCtx {
    pub seed: u64,
    /// Film is a negative (dust prints white) vs slide/digital (dark).
    pub negative: bool,
    pub mono: bool,
}

pub trait Effect: Send + Sync {
    fn slot(&self) -> Slot;
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx);
}

pub fn build(spec: &EffectSpec) -> Option<Box<dyn Effect>> {
    let s = spec.clone();
    let b: Box<dyn Effect> = match spec.kind.as_str() {
        "light_leak" => Box::new(LightLeak(s)),
        "fog" => Box::new(Fog(s)),
        "vignette" => Box::new(Vignette(s)),
        "haze" => Box::new(Haze(s)),
        "flare" => Box::new(Flare(s)),
        "prism" => Box::new(Prism(s)),
        "dust" => Box::new(Dust(s)),
        "scratches" => Box::new(Scratches(s)),
        "frame" => Box::new(Frame(s)),
        "date_stamp" => Box::new(DateStamp(s)),
        "toning" => Box::new(Toning(s)),
        "color_shift" => Box::new(ColorShift(s)),
        _ => return None,
    };
    Some(b)
}

// ------------------------------------------------------------ exposure slot

/// Light leaking through the camera back: warm additive blobs entering from an
/// edge. Added in exposure space so they bloom, halate and saturate like light.
struct LightLeak(EffectSpec);
impl Effect for LightLeak {
    fn slot(&self) -> Slot {
        Slot::Exposure
    }
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx) {
        let s = &self.0;
        let mut rng = Rng::new(ctx.seed);
        let color = s.rgb("color", [1.0, 0.35, 0.08]);
        let count = s.f("count", 2.0).max(1.0) as usize;
        let size = s.f("size", 0.35);
        let edge = s.s("edge", "random").to_string();
        let blobs: Vec<(f32, f32, f32, f32, [f32; 3])> = (0..count)
            .map(|_| {
                let e = match edge.as_str() {
                    "left" => 0,
                    "right" => 1,
                    "top" => 2,
                    "bottom" => 3,
                    _ => (rng.f() * 4.0) as u32,
                };
                let t = rng.range(0.1, 0.9);
                let (cx, cy) = match e {
                    0 => (rng.range(-0.15, 0.02), t),
                    1 => (rng.range(0.98, 1.15), t),
                    2 => (t, rng.range(-0.15, 0.02)),
                    _ => (t, rng.range(0.98, 1.15)),
                };
                let r = size * rng.range(0.6, 1.4);
                let inten = rng.range(0.5, 1.2);
                let hue = rng.range(-0.15, 0.15);
                let col = [
                    color[0],
                    (color[1] + hue).max(0.0),
                    (color[2] + hue * 0.3).max(0.0),
                ];
                (cx, cy, r, inten, col)
            })
            .collect();
        let st = s.strength;
        let aspect = img.w as f32 / img.h as f32;
        let (w, h) = (img.w, img.h);
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            for (cx, cy, r, inten, col) in &blobs {
                let dx = (u - cx) * aspect;
                let dy = v - cy;
                let d = (dx * dx + dy * dy).sqrt() / r;
                let f = (-d * d * 2.5).exp() * inten * st;
                for c in 0..3 {
                    p[c] += col[c] * f;
                }
            }
        });
    }
}

/// Edge fog: uniform light fog creeping from frame edges (film loading).
struct Fog(EffectSpec);
impl Effect for Fog {
    fn slot(&self) -> Slot {
        Slot::Exposure
    }
    fn apply(&self, img: &mut ImageF32, _ctx: &FxCtx) {
        let col = self.0.rgb("color", [1.0, 0.55, 0.3]);
        let width = self.0.f("width", 0.12);
        let st = self.0.strength * 0.6;
        let (w, h) = (img.w, img.h);
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let d = u.min(1.0 - u).min(v.min(1.0 - v));
            let f = (1.0 - smoothstep(0.0, width, d)) * st;
            for c in 0..3 {
                p[c] += col[c] * f;
            }
        });
    }
}

/// Extra (toy-camera) vignetting, in exposure space.
struct Vignette(EffectSpec);
impl Effect for Vignette {
    fn slot(&self) -> Slot {
        Slot::Exposure
    }
    fn apply(&self, img: &mut ImageF32, _ctx: &FxCtx) {
        let st = self.0.strength;
        let roundness = self.0.f("roundness", 1.0);
        let (w, h) = (img.w as f32, img.h as f32);
        let a = mix(1.0, w / h, roundness);
        img.par_map_pixels(|x, y, p| {
            let dx = ((x as f32 + 0.5) / w - 0.5) * 2.0 * a;
            let dy = ((y as f32 + 0.5) / h - 0.5) * 2.0;
            let r2 = (dx * dx + dy * dy) / (a * a + 1.0);
            let f = 1.0 / (1.0 + 2.5 * st * r2 * r2);
            p[0] *= f;
            p[1] *= f;
            p[2] *= f;
        });
    }
}

// ------------------------------------------------------------- optical slot

/// Veiling haze (old/uncoated glass, shooting into light).
struct Haze(EffectSpec);
impl Effect for Haze {
    fn slot(&self) -> Slot {
        Slot::Optical
    }
    fn apply(&self, img: &mut ImageF32, _ctx: &FxCtx) {
        let col = self.0.rgb("color", [1.0, 0.95, 0.88]);
        let st = self.0.strength * 0.25;
        let wide = blur::gaussian(img, img.w as f32 * 0.08);
        img.par_map_pixels(|x, y, p| {
            let i = (y * wide.w + x) * 3;
            let l = luma([wide.data[i], wide.data[i + 1], wide.data[i + 2]]);
            for c in 0..3 {
                p[c] = p[c] * (1.0 - st * 0.5) + col[c] * l * st;
            }
        });
    }
}

/// Lens flare ghosts: bright areas reflected through the optical center at a
/// few scales, blurred and tinted (cheap single-coated lens look).
struct Flare(EffectSpec);
impl Effect for Flare {
    fn slot(&self) -> Slot {
        Slot::Optical
    }
    fn apply(&self, img: &mut ImageF32, _ctx: &FxCtx) {
        let st = self.0.strength;
        let thr = self.0.f("threshold", 0.85);
        let factor = (img.w / 256).max(1);
        let mut src = img.downsample(factor);
        src.data.chunks_exact_mut(3).for_each(|p| {
            let l = luma([p[0], p[1], p[2]]);
            let k = smoothstep(thr, 1.0, l);
            p.iter_mut().for_each(|v| *v *= k);
        });
        blur::gaussian_in_place(&mut src, 1.5);
        let ghosts: [(f32, [f32; 3]); 4] = [
            (-0.6, [0.3, 0.6, 1.0]),
            (-1.2, [1.0, 0.5, 0.2]),
            (0.45, [0.6, 1.0, 0.5]),
            (-0.25, [1.0, 0.3, 0.8]),
        ];
        let halo = blur::gaussian(&src, src.w as f32 * 0.03);
        let (w, h) = (img.w, img.h);
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            for (scale, tint) in &ghosts {
                let gu = 0.5 + (u - 0.5) * scale;
                let gv = 0.5 + (v - 0.5) * scale;
                if (0.0..=1.0).contains(&gu) && (0.0..=1.0).contains(&gv) {
                    let c = halo.sample_uv(gu, gv);
                    for ch in 0..3 {
                        p[ch] += c[ch] * tint[ch] * st * 0.35;
                    }
                }
            }
        });
    }
}

/// Prism / crystal held in front of the lens: a displaced, softened echo of
/// the scene fading in from one side (Omni-style creative filter).
struct Prism(EffectSpec);
impl Effect for Prism {
    fn slot(&self) -> Slot {
        Slot::Optical
    }
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx) {
        let mut rng = Rng::new(ctx.seed);
        let st = self.0.strength;
        let shift = self.0.f("shift", 0.18);
        let angle = self.0.f("angle", rng.range(0.0, 360.0)).to_radians();
        let soft = blur::gaussian(img, img.w as f32 * 0.004);
        let (dxn, dyn_) = (angle.cos(), angle.sin());
        let src = img.clone();
        let (w, h) = (img.w, img.h);
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            // mask: strongest along the side the prism sits on
            let side = (u - 0.5) * dxn + (v - 0.5) * dyn_;
            let m = smoothstep(0.0, 0.5, side) * st;
            if m <= 0.0 {
                return;
            }
            let su = u - dxn * shift;
            let sv = v - dyn_ * shift;
            let e = soft.sample_uv(su.clamp(0.0, 1.0), sv.clamp(0.0, 1.0));
            let o = src.get(x, y);
            for c in 0..3 {
                // rainbow fringe: offset channels slightly
                p[c] = mix(o[c], 0.5 * o[c] + 0.6 * e[c], m);
            }
        });
    }
}

// ------------------------------------------------------------ emulsion slot

/// Dust & hair on the negative/scan.
struct Dust(EffectSpec);
impl Effect for Dust {
    fn slot(&self) -> Slot {
        Slot::Emulsion
    }
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx) {
        let mut rng = Rng::new(ctx.seed);
        let n = (self.0.f("count", 40.0) * self.0.strength) as usize;
        let tone = if ctx.negative { 1.0 } else { 0.03 };
        let scale = img.w as f32 / 1000.0;
        for _ in 0..n {
            let cx = rng.f() * img.w as f32;
            let cy = rng.f() * img.h as f32;
            let hair = rng.f() < 0.2;
            let r = rng.range(0.6, 2.2) * scale;
            let len = if hair {
                rng.range(10.0, 40.0) * scale
            } else {
                0.0
            };
            let ang = rng.f() * std::f32::consts::TAU;
            let op = rng.range(0.4, 0.9);
            let steps = (len / (r * 0.5)).max(1.0) as usize;
            for s in 0..steps {
                let t = s as f32 / steps as f32;
                let bend = (t * 3.0).sin() * len * 0.1;
                let px = cx + ang.cos() * len * t - ang.sin() * bend;
                let py = cy + ang.sin() * len * t + ang.cos() * bend;
                let ri = r.ceil() as isize + 1;
                for oy in -ri..=ri {
                    for ox in -ri..=ri {
                        let x = px as isize + ox;
                        let y = py as isize + oy;
                        if x < 0 || y < 0 || x >= img.w as isize || y >= img.h as isize {
                            continue;
                        }
                        let d = ((ox * ox + oy * oy) as f32).sqrt();
                        let a = (1.0 - smoothstep(r * 0.6, r, d)) * op;
                        if a > 0.0 {
                            let c = img.get(x as usize, y as usize);
                            img.set(x as usize, y as usize, mix3(c, [tone; 3], a));
                        }
                    }
                }
            }
        }
    }
}

/// Vertical base scratches (cine film / rough handling).
struct Scratches(EffectSpec);
impl Effect for Scratches {
    fn slot(&self) -> Slot {
        Slot::Emulsion
    }
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx) {
        let mut rng = Rng::new(ctx.seed ^ 0x5c);
        let n = (self.0.f("count", 4.0) * self.0.strength).round() as usize;
        let lines: Vec<(f32, f32, f32, f32)> = (0..n)
            .map(|_| {
                (
                    rng.f(),
                    rng.range(0.3, 1.0),
                    rng.range(0.2, 0.7),
                    rng.range(-0.02, 0.02),
                )
            })
            .collect();
        let tone = if ctx.negative { 1.0 } else { 0.0 };
        let (w, h) = (img.w, img.h);
        let px = 1.0 / w as f32;
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            for (lx, len, op, drift) in &lines {
                if v > *len {
                    continue;
                }
                let cx = lx + drift * v;
                let d = ((u - cx) / px).abs();
                let a = (1.0 - smoothstep(0.3, 1.2, d)) * op;
                if a > 0.0 {
                    for v in p.iter_mut() {
                        *v = mix(*v, tone, a);
                    }
                }
            }
        });
    }
}

// --------------------------------------------------------------- print slot

/// Frame / border: "black" (filed carrier), "white" (print border), "rebate".
struct Frame(EffectSpec);
impl Effect for Frame {
    fn slot(&self) -> Slot {
        Slot::Print
    }
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx) {
        let style = self.0.s("style", "black").to_string();
        let t = self.0.f("thickness", 0.035) * self.0.strength;
        let mut rng = Rng::new(ctx.seed ^ 0xf4);
        let jag: Vec<f32> = (0..64).map(|_| rng.range(0.0, 1.0)).collect();
        let col = match style.as_str() {
            "white" => [0.95, 0.94, 0.91],
            "rebate" => [0.85, 0.45, 0.12],
            _ => [0.02, 0.02, 0.02],
        };
        let (w, h) = (img.w, img.h);
        let aspect = w as f32 / h as f32;
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let ju = jag[((v * 63.0) as usize).min(63)] * t * 0.25;
            let jv = jag[((u * 63.0) as usize).min(63)] * t * 0.25;
            let du = u.min(1.0 - u) * aspect;
            let dv = v.min(1.0 - v);
            let inside_u = smoothstep(t + ju, t + ju + 0.002, du);
            let inside_v = smoothstep(t + jv, t + jv + 0.002, dv);
            let m = 1.0 - inside_u * inside_v;
            if m > 0.0 {
                for c in 0..3 {
                    p[c] = mix(p[c], col[c], m);
                }
            }
        });
    }
}

/// Orange 7-segment date imprint (glows: drawn in exposure-like brightness).
struct DateStamp(EffectSpec);
const SEGS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];
impl Effect for DateStamp {
    fn slot(&self) -> Slot {
        Slot::Print
    }
    fn apply(&self, img: &mut ImageF32, _ctx: &FxCtx) {
        let text = self.0.s("text", "'26 11 26").to_string();
        let col = self.0.rgb("color", [1.0, 0.48, 0.1]);
        let st = self.0.strength;
        let ch_h = img.h as f32 * 0.035;
        let ch_w = ch_h * 0.55;
        let gap = ch_w * 0.45;
        let thick = ch_h * 0.12;
        let total = text.chars().count() as f32 * (ch_w + gap);
        let x0 = img.w as f32 * 0.93 - total;
        let y0 = img.h as f32 * 0.92 - ch_h;
        let mut glyph = ImageF32::new(img.w, img.h);
        // Segment rectangles in glyph-local unit coords (x0,y0,x1,y1).
        let seg_rect = |s: usize| -> (f32, f32, f32, f32) {
            match s {
                0 => (0.1, 0.0, 0.9, 0.12),
                1 => (0.85, 0.05, 1.0, 0.5),
                2 => (0.85, 0.5, 1.0, 0.95),
                3 => (0.1, 0.88, 0.9, 1.0),
                4 => (0.0, 0.5, 0.15, 0.95),
                5 => (0.0, 0.05, 0.15, 0.5),
                _ => (0.1, 0.44, 0.9, 0.56),
            }
        };
        for (i, c) in text.chars().enumerate() {
            let gx = x0 + i as f32 * (ch_w + gap);
            let rects: Vec<(f32, f32, f32, f32)> = match c {
                '0'..='9' => {
                    let m = SEGS[c as usize - '0' as usize];
                    (0..7).filter(|s| m & (1 << s) != 0).map(seg_rect).collect()
                }
                '\'' => vec![(0.4, 0.0, 0.6, 0.25)],
                '-' => vec![seg_rect(6)],
                _ => vec![],
            };
            for (a, b, cc, d) in rects {
                let rx0 = (gx + a * ch_w) as isize;
                let rx1 = (gx + cc * ch_w).max(gx + a * ch_w + thick) as isize;
                let ry0 = (y0 + b * ch_h) as isize;
                let ry1 = (y0 + d * ch_h) as isize;
                for y in ry0.max(0)..ry1.min(img.h as isize) {
                    for x in rx0.max(0)..rx1.min(img.w as isize) {
                        glyph.set(x as usize, y as usize, col);
                    }
                }
            }
        }
        let bloom = blur::gaussian(&glyph, ch_h * 0.15);
        let core = blur::gaussian(&glyph, (ch_h * 0.02).max(0.5));
        img.par_map_pixels(|x, y, p| {
            let i = (y * glyph.w + x) * 3;
            for (c, v) in p.iter_mut().enumerate() {
                let g = core.data[i + c] + bloom.data[i + c] * 0.8;
                // screen blend
                *v = 1.0 - (1.0 - *v) * (1.0 - (g * st).min(1.0));
            }
        });
    }
}

/// B&W toning: sepia, selenium, cyanotype, split.
struct Toning(EffectSpec);
impl Effect for Toning {
    fn slot(&self) -> Slot {
        Slot::Print
    }
    fn apply(&self, img: &mut ImageF32, _ctx: &FxCtx) {
        let (sh, hl) = match self.0.s("tone", "sepia") {
            "selenium" => ([0.32, 0.25, 0.32], [1.0, 0.98, 0.95]),
            "cyanotype" => ([0.05, 0.18, 0.38], [0.9, 0.97, 1.0]),
            "split" => ([0.15, 0.22, 0.32], [1.0, 0.92, 0.78]),
            _ => ([0.28, 0.17, 0.08], [1.0, 0.95, 0.85]),
        };
        let st = self.0.strength;
        img.par_map_pixels(|_, _, p| {
            let l = luma([p[0], p[1], p[2]]).clamp(0.0, 1.0);
            let toned = [
                mix(sh[0], hl[0], l) * l.powf(0.8),
                mix(sh[1], hl[1], l) * l.powf(0.8),
                mix(sh[2], hl[2], l) * l.powf(0.8),
            ];
            for c in 0..3 {
                p[c] = mix(p[c], toned[c].max(0.0) * 1.15, st.min(1.0));
            }
        });
    }
}

/// Expired / heat-damaged stock: color cast, lifted blacks, lower contrast.
struct ColorShift(EffectSpec);
impl Effect for ColorShift {
    fn slot(&self) -> Slot {
        Slot::Print
    }
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx) {
        let mut rng = Rng::new(ctx.seed ^ 0xc5);
        let default = [
            rng.range(-0.06, 0.08),
            rng.range(-0.05, 0.06),
            rng.range(-0.08, 0.05),
        ];
        let cast = self.0.rgb("cast", default);
        let st = self.0.strength;
        let lift = self.0.f("lift", 0.06);
        img.par_map_pixels(|_, _, p| {
            for c in 0..3 {
                let v = lift * st + p[c] * (1.0 - lift * st * 1.5);
                p[c] = v + cast[c] * st;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_builds_and_runs() {
        for k in KINDS {
            let spec = EffectSpec {
                kind: k.to_string(),
                id: String::new(),
                strength: 1.0,
                seed_mode: Default::default(),
                params: Default::default(),
            };
            let fx = build(&spec).expect(k);
            let mut img = ImageF32::filled(96, 64, [0.4, 0.4, 0.4]);
            img.set(48, 32, [5.0; 3]);
            fx.apply(
                &mut img,
                &FxCtx {
                    seed: 1,
                    negative: true,
                    mono: false,
                },
            );
            assert!(img.data.iter().all(|v| v.is_finite()), "{k} produced NaN");
        }
    }

    #[test]
    fn unknown_kind_is_skipped() {
        let spec = EffectSpec {
            kind: "from_the_future".into(),
            id: String::new(),
            strength: 1.0,
            seed_mode: Default::default(),
            params: Default::default(),
        };
        assert!(build(&spec).is_none());
    }
}
