//! Effect registry. Each kind implements `Effect` and declares its physical
//! pipeline slot. Presets (film, lens, body) and users add effects by `kind`
//! name; unknown kinds are skipped (forward compatible). To add a new kind:
//! implement `Effect`, add a `build()` arm and its name to `KINDS`.

pub mod omni;

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
    "omni",
];

#[derive(Default)]
pub struct FxCtx {
    pub seed: u64,
    /// [`crate::recipe::Recipe::capture_rotation`]: viewfinder → photo frame.
    pub rotation: u8,
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
        "omni" => Box::new(omni::OmniFx(s)),
        _ => return None,
    };
    Some(b)
}

/// Where a placed effect sits: the point on the border of a w×h frame in
/// direction `deg` from the centre (0 right, 90 bottom, 180 left, 270 top,
/// y down), and the outward unit direction. Shared by every effect with a
/// `position` param and mirrored in `final.frag`.
pub fn edge_point(deg: f32, w: f32, h: f32) -> ([f32; 2], [f32; 2]) {
    let (s, c) = deg.to_radians().sin_cos();
    let tx = if c.abs() > 1e-6 {
        0.5 * w / c.abs()
    } else {
        f32::MAX
    };
    let ty = if s.abs() > 1e-6 {
        0.5 * h / s.abs()
    } else {
        f32::MAX
    };
    let t = tx.min(ty);
    ([0.5 * w + c * t, 0.5 * h + s * t], [c, s])
}

/// The effect's `position` param (degrees), if the user placed it.
fn position(s: &EffectSpec) -> Option<f32> {
    s.params.get("position").map(|_| s.f("position", 0.0))
}

// ------------------------------------------------------------ exposure slot

/// One light-leak blob. `at` is the centre in image uv (y down), or for a
/// placed leak the offset from the edge anchor: [along the edge, outward],
/// in frame-height units (the viewfinder's: `LightLeak` rescales them for a
/// turned shot; resolved by [`leak_center`] once the aspect is known, so
/// preview and capture agree at any frame size).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeakBlob {
    pub at: [f32; 2],
    pub r: f32,
    pub inten: f32,
    pub col: [f32; 3],
}

/// Blobs beyond this many are dropped: a huge `count` would otherwise
/// allocate and loop without bound. Also the preview's uniform slot count.
pub const LEAK_MAX_BLOBS: usize = 4;

/// The leak's blobs for effect seed `seed` and its placement (degrees) if
/// placed. Shared by capture and `preview::params`, so the viewfinder shows
/// exactly the leak the shot will get.
pub fn leak_blobs(s: &EffectSpec, seed: u64) -> (Option<f32>, Vec<LeakBlob>) {
    let mut rng = Rng::new(seed);
    let color = s.rgb("color", [1.0, 0.35, 0.08]);
    let want = s.f("count", 2.0).max(1.0) as usize;
    if want > LEAK_MAX_BLOBS {
        eprintln!("light_leak: count {want} clamped to {LEAK_MAX_BLOBS}");
    }
    let size = s.f("size", 0.35);
    let edge = s.s("edge", "random");
    let placed = position(s);
    let blobs = (0..want.min(LEAK_MAX_BLOBS))
        .map(|_| {
            let at = if placed.is_some() {
                // spread along the chosen edge, centred just outside it
                [rng.range(-0.22, 0.22), rng.range(0.0, 0.12)]
            } else {
                let e = match edge {
                    "left" => 0,
                    "right" => 1,
                    "top" => 2,
                    "bottom" => 3,
                    _ => (rng.f() * 4.0) as u32,
                };
                let t = rng.range(0.1, 0.9);
                match e {
                    0 => [rng.range(-0.15, 0.02), t],
                    1 => [rng.range(0.98, 1.15), t],
                    2 => [t, rng.range(-0.15, 0.02)],
                    _ => [t, rng.range(0.98, 1.15)],
                }
            };
            let r = size * rng.range(0.6, 1.4);
            let inten = rng.range(0.5, 1.2);
            let hue = rng.range(-0.15, 0.15);
            let col = [
                color[0],
                (color[1] + hue).max(0.0),
                (color[2] + hue * 0.3).max(0.0),
            ];
            LeakBlob { at, r, inten, col }
        })
        .collect();
    (placed, blobs)
}

/// Viewfinder uv (y down) → photo uv for a shot taken at `rotation`
/// (0..3); same mapping as the app's `UserParams.forCaptureRotation`.
pub fn rotate_uv([x, y]: [f32; 2], rotation: u8) -> [f32; 2] {
    match rotation % 4 {
        1 => [y, 1.0 - x],
        2 => [1.0 - x, 1.0 - y],
        3 => [1.0 - y, x],
        _ => [x, y],
    }
}

/// Degrees added to a viewfinder-space direction for a shot taken at `rotation`.
pub fn rotate_deg(deg: f32, rotation: u8) -> f32 {
    deg + [0.0, -90.0, 180.0, 90.0][(rotation % 4) as usize]
}

/// Blob centre in image uv (y down) for a frame of this aspect (w / h).
/// Mirrored in `final.frag` (FX_LEAK).
pub fn leak_center(b: &LeakBlob, placed: Option<f32>, aspect: f32) -> [f32; 2] {
    match placed {
        Some(deg) => {
            let (a, o) = edge_point(deg, aspect, 1.0);
            let [along, out] = b.at;
            [
                (a[0] - o[1] * along + o[0] * out) / aspect,
                a[1] + o[0] * along + o[1] * out,
            ]
        }
        None => b.at,
    }
}

/// Light leaking through the camera back: warm additive blobs entering from an
/// edge. Added in exposure space so they bloom, halate and saturate like light.
struct LightLeak(EffectSpec);
impl Effect for LightLeak {
    fn slot(&self) -> Slot {
        Slot::Exposure
    }
    fn apply(&self, img: &mut ImageF32, ctx: &FxCtx) {
        let aspect = img.w as f32 / img.h as f32;
        let (placed, blobs) = leak_blobs(&self.0, ctx.seed);
        // sizes are in viewfinder-height units: a shot turned a quarter has
        // the viewfinder's height across its width
        let unit = if ctx.rotation % 2 == 1 { aspect } else { 1.0 };
        let blobs: Vec<([f32; 2], LeakBlob)> = blobs
            .into_iter()
            // placed positions were already rotated by the app; random ones
            // were drawn in the viewfinder's frame
            .map(|mut b| {
                b.r *= unit;
                if placed.is_some() {
                    b.at = b.at.map(|v| v * unit);
                }
                let c = leak_center(&b, placed, aspect);
                (
                    if placed.is_some() {
                        c
                    } else {
                        rotate_uv(c, ctx.rotation)
                    },
                    b,
                )
            })
            .collect();
        let st = self.0.strength;
        let (w, h) = (img.w, img.h);
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            for ([cx, cy], b) in &blobs {
                let dx = (u - cx) * aspect;
                let dy = v - cy;
                let d = (dx * dx + dy * dy).sqrt() / b.r;
                let f = (-d * d * 2.5).exp() * b.inten * st;
                for (v, c) in p.iter_mut().zip(b.col) {
                    *v += c * f;
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
        let aspect = w as f32 / h as f32;
        let side = position(&self.0).map(|d| edge_point(d, aspect, 1.0).1);
        img.par_map_pixels(|x, y, p| {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let d = u.min(1.0 - u).min(v.min(1.0 - v));
            // placed: only the edges facing that direction fog (light got in
            // there), reaching further in; unplaced: every edge, as before
            let (dir, reach) = side.map_or((1.0, width), |o| {
                let (dx, dy) = ((u - 0.5) * aspect, v - 0.5);
                let l = (dx * dx + dy * dy).sqrt().max(1e-6);
                let dir = smoothstep(0.1, 0.75, (dx * o[0] + dy * o[1]) / l);
                (dir, width * (1.0 + dir))
            });
            let f = (1.0 - smoothstep(0.0, reach, d)) * st * dir;
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
        let wide = blur::Blurred::new(img, img.w as f32 * 0.08);
        img.par_map_pixels(|x, y, p| {
            let l = luma(wide.sample(x as f32 + 0.5, y as f32 + 0.5));
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
        let random = rng.range(0.0, 360.0);
        let angle = position(&self.0)
            .unwrap_or_else(|| rotate_deg(self.0.f("angle", random), ctx.rotation))
            .to_radians();
        let soft = blur::Blurred::new(img, img.w as f32 * 0.004);
        let (dxn, dyn_) = (angle.cos(), angle.sin());
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
            // each pixel only reads itself from `img`, so no source copy is needed
            for c in 0..3 {
                // rainbow fringe: offset channels slightly
                p[c] = mix(p[c], 0.5 * p[c] + 0.6 * e[c], m);
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
                    rotation: 0,
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

#[cfg(test)]
mod placement_tests {
    use super::*;

    fn spec(kind: &str, pos: Option<f32>) -> EffectSpec {
        let p = pos.map_or(String::new(), |d| format!("[params]\nposition = {d:.1}\n"));
        toml::from_str(&format!("kind = \"{kind}\"\n{p}")).unwrap()
    }

    fn run(kind: &str, pos: Option<f32>) -> ImageF32 {
        let mut img = ImageF32::filled(80, 60, [0.2, 0.2, 0.2]);
        build(&spec(kind, pos)).unwrap().apply(
            &mut img,
            &FxCtx {
                rotation: 0,
                seed: 7,
                negative: false,
                mono: false,
            },
        );
        img
    }

    fn lift(img: &ImageF32, x: usize, y: usize) -> f32 {
        img.get(x, y).iter().sum::<f32>() - 0.6
    }

    #[test]
    fn edge_point_lands_on_the_border() {
        let (a, o) = edge_point(0.0, 4.0, 3.0);
        assert!((a[0] - 4.0).abs() < 1e-5 && (a[1] - 1.5).abs() < 1e-5 && o == [1.0, 0.0]);
        let (a, _) = edge_point(270.0, 4.0, 3.0);
        assert!(a[1].abs() < 1e-4 && (a[0] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn placed_light_leak_comes_from_that_side() {
        let left = run("light_leak", Some(180.0));
        assert!(lift(&left, 1, 30) > 0.05);
        assert!(lift(&left, 78, 30) < 0.01);
        let top = run("light_leak", Some(270.0));
        assert!(lift(&top, 40, 1) > lift(&top, 40, 58) + 0.05);
    }

    #[test]
    fn placed_fog_only_fogs_that_edge() {
        let bottom = run("fog", Some(90.0));
        assert!(lift(&bottom, 40, 59) > 0.05);
        assert!(lift(&bottom, 40, 0) < 1e-3);
        let all = run("fog", None);
        // unplaced fog keeps its original reach: nothing past `width` (0.12)
        assert!(lift(&all, 40, 8).abs() < 1e-6, "{}", lift(&all, 40, 8));
        assert!(
            lift(&all, 40, 0) > 0.05 && lift(&all, 40, 59) > 0.05,
            "unplaced fog keeps every edge"
        );
    }

    #[test]
    fn non_finite_params_fall_back_to_defaults() {
        let spec: EffectSpec = toml::from_str(
            "kind = \"omni\"\n[params]\nreach = nan\nposition = inf\nwand = \"swirl\"",
        )
        .unwrap();
        let mut img = ImageF32::filled(40, 30, [0.3, 0.3, 0.3]);
        build(&spec).unwrap().apply(
            &mut img,
            &FxCtx {
                rotation: 0,
                seed: 1,
                negative: false,
                mono: false,
            },
        );
        assert!(img.data.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn leak_count_is_bounded() {
        let spec: EffectSpec =
            toml::from_str("kind = \"light_leak\"\n[params]\ncount = 1e15").unwrap();
        assert_eq!(leak_blobs(&spec, 3).1.len(), LEAK_MAX_BLOBS);
    }

    #[test]
    fn random_leak_follows_the_capture_rotation() {
        // a leak from the viewfinder's left edge lands on the bottom of a
        // shot taken at ROTATION_90 (viewfinder (x, y) → photo (y, 1 - x))
        let spec: EffectSpec =
            toml::from_str("kind = \"light_leak\"\n[params]\nedge = \"left\"\ncount = 1").unwrap();
        let run = |rotation| {
            let mut img = ImageF32::filled(60, 60, [0.2; 3]);
            build(&spec).unwrap().apply(
                &mut img,
                &FxCtx {
                    seed: 3,
                    rotation,
                    ..Default::default()
                },
            );
            img
        };
        let upright = run(0);
        assert!(lift(&upright, 0, 30) > 0.05 && lift(&upright, 30, 59) < 0.05);
        let turned = run(1);
        assert!(
            lift(&turned, 30, 59) > 0.05,
            "bottom: {}",
            lift(&turned, 30, 59)
        );
        assert!(lift(&turned, 0, 30) < lift(&turned, 30, 59) * 0.5);
    }

    /// A shot taken at ROTATION_90 is the portrait viewfinder turned
    /// landscape: its leaks are the viewfinder's, turned, at the same size.
    #[test]
    fn turned_shot_is_the_turned_viewfinder_leak() {
        for params in ["count = 2", "count = 2\nposition = 200.0"] {
            let spec: EffectSpec =
                toml::from_str(&format!("kind = \"light_leak\"\n[params]\n{params}")).unwrap();
            let placed = position(&spec);
            let run = |w, h, rotation| {
                let mut img = ImageF32::filled(w, h, [0.0; 3]);
                let mut s = spec.clone();
                if let Some(d) = placed {
                    // what the app saves for the shot (Recipe.forCaptureRotation)
                    s.params.insert(
                        "position".into(),
                        toml::Value::Float(rotate_deg(d, rotation) as f64),
                    );
                }
                build(&s).unwrap().apply(
                    &mut img,
                    &FxCtx {
                        seed: 11,
                        rotation,
                        ..Default::default()
                    },
                );
                img
            };
            let (vf, shot) = (run(60, 80, 0), run(80, 60, 1));
            let mut peak = 0.0f32;
            for py in 0..60 {
                for px in 0..80 {
                    // photo (u, v) = (v_vf, 1 - u_vf)
                    let (a, b) = (shot.get(px, py), vf.get(59 - py, px));
                    for k in 0..3 {
                        assert!(
                            (a[k] - b[k]).abs() < 1e-4,
                            "{params} ({px},{py}): {a:?} vs {b:?}"
                        );
                    }
                    peak = peak.max(a[0]);
                }
            }
            assert!(peak > 0.05, "{params}: leak visible");
        }
    }

    #[test]
    fn rotation_helpers_match_the_app_mapping() {
        assert_eq!(rotate_uv([0.2, 0.3], 0), [0.2, 0.3]);
        assert_eq!(rotate_uv([0.2, 0.3], 1), [0.3, 0.8]);
        assert_eq!(rotate_uv([0.2, 0.25], 2), [0.8, 0.75]);
        assert_eq!(rotate_uv([0.2, 0.3], 3), [0.7, 0.2]);
        assert_eq!(rotate_deg(10.0, 1), -80.0);
        assert_eq!(rotate_deg(10.0, 3), 100.0);
    }
}
