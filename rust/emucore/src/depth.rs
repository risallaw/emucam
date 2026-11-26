//! Depth maps for realistic defocus. Phones rarely expose a depth sensor to
//! apps, so the app runs a monocular depth network and hands its raw output
//! here. This module owns everything model-independent:
//!
//! * building the network input from the decoded photo ([`model_input`]),
//!   rotating landscape shots to portrait because the networks are trained
//!   for portrait aspect;
//! * converting the raw output (depth or inverse depth) to *nearness*
//!   (inverse depth normalised to 0 = far … 1 = near) — thin-lens defocus is
//!   linear in inverse depth, so blur radius ∝ |nearness − focus nearness|;
//! * edge-aware upsampling to photo resolution with a guided filter so depth
//!   edges snap to object edges instead of bleeding blur across them.

use crate::color::linear_to_srgb;
use crate::image::{ImageF32, MaskF32};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// What the raw network output means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DepthKind {
    /// Larger = farther (e.g. Depth Anything 3).
    #[default]
    Depth,
    /// Larger = nearer, disparity-like (e.g. MiDaS).
    InverseDepth,
}

/// Tensor layout the network expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelLayout {
    /// Input width / height in the network's portrait orientation.
    pub w: usize,
    pub h: usize,
    /// true = NCHW planar, false = NHWC interleaved.
    pub nchw: bool,
}

/// Largest accepted network input side. Depth networks run at ≤ ~1 k px; a
/// bogus layout (e.g. from a JNI caller) would otherwise allocate w·h·3
/// floats and abort the process on OOM, which `catch_unwind` can't stop.
pub const MAX_LAYOUT_SIDE: usize = 4096;

impl ModelLayout {
    /// Reject empty or absurd layouts (zero would divide by zero in
    /// [`model_input`]).
    pub fn validate(&self) -> Result<(), String> {
        let ok = |v: usize| (1..=MAX_LAYOUT_SIDE).contains(&v);
        if ok(self.w) && ok(self.h) {
            Ok(())
        } else {
            Err(format!(
                "bad depth layout {}x{} (1..={MAX_LAYOUT_SIDE} per side)",
                self.w, self.h
            ))
        }
    }
}

/// Landscape photos are rotated 90° clockwise to portrait before inference
/// (and the result rotated back), so the network sees its training aspect.
/// Only portrait layouts rotate: a square or landscape layout (e.g. a
/// segmentation network at 1024×1024) gets the upright photo, stretched.
pub fn rotates(img_w: usize, img_h: usize, layout_w: usize, layout_h: usize) -> bool {
    img_w > img_h && layout_h > layout_w
}

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Build an ImageNet-normalised input tensor (sRGB, stretched to the layout).
/// Returns an empty tensor for an invalid layout (see [`ModelLayout::validate`]).
pub fn model_input(img: &ImageF32, layout: ModelLayout) -> Vec<f32> {
    if layout.validate().is_err() || img.w == 0 || img.h == 0 {
        return Vec::new();
    }
    let rot = rotates(img.w, img.h, layout.w, layout.h);
    // portrait-oriented source size
    let (pw, ph) = if rot { (img.h, img.w) } else { (img.w, img.h) };
    // box-downsample first so the bilinear resize doesn't alias
    let factor = (pw / layout.w).min(ph / layout.h).max(1);
    let small = img.downsample(factor);
    let (sw, sh) = (small.w as f32, small.h as f32);
    let (lw, lh) = (layout.w, layout.h);
    let plane = lw * lh;
    let mut out = vec![0.0f32; plane * 3];
    for y in 0..lh {
        for x in 0..lw {
            // portrait (x, y) → source coords; rotated: portrait = source turned 90° cw
            let (u, v) = ((x as f32 + 0.5) / lw as f32, (y as f32 + 0.5) / lh as f32);
            let (sx, sy) = if rot {
                (v * sw, (1.0 - u) * sh)
            } else {
                (u * sw, v * sh)
            };
            let c = small.sample(sx, sy);
            for ch in 0..3 {
                let n = (linear_to_srgb(c[ch]).clamp(0.0, 1.0) - MEAN[ch]) / STD[ch];
                let i = y * lw + x;
                if layout.nchw {
                    out[ch * plane + i] = n;
                } else {
                    out[i * 3 + ch] = n;
                }
            }
        }
    }
    out
}

/// Network output → nearness map (0 far … 1 near) in the photo's orientation.
/// Percentile normalisation (1 %–99 %) makes it robust to the unknown scale
/// and shift of relative depth networks and to a few outlier pixels.
pub fn nearness(
    raw: &[f32],
    w: usize,
    h: usize,
    kind: DepthKind,
    img_w: usize,
    img_h: usize,
) -> Option<MaskF32> {
    // checked: a huge w·h must not wrap around to raw.len() (release builds)
    if w == 0 || h == 0 || w.checked_mul(h) != Some(raw.len()) || raw.iter().any(|v| !v.is_finite())
    {
        return None;
    }
    let inv: Vec<f32> = match kind {
        DepthKind::Depth => raw.iter().map(|d| 1.0 / d.max(1e-3)).collect(),
        DepthKind::InverseDepth => raw.to_vec(),
    };
    // Two order statistics: selection is O(n) vs a full sort (same values).
    let mut sorted = inv.clone();
    let n = sorted.len();
    let hi = *sorted
        .select_nth_unstable_by(n * 99 / 100, f32::total_cmp)
        .1;
    let lo = *sorted[..n * 99 / 100 + 1]
        .select_nth_unstable_by(n / 100, f32::total_cmp)
        .1;
    if hi - lo <= 1e-6 {
        return None; // flat output carries no depth information
    }
    let norm = |v: f32| ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    let mut m = if rotates(img_w, img_h, w, h) {
        // undo the 90° cw rotation
        let mut m = MaskF32::new(h, w);
        for py in 0..h {
            for px in 0..w {
                // portrait (px, py) came from photo (py, w-1-px)
                let (x, y) = (py, w - 1 - px);
                m.data[y * h + x] = norm(inv[py * w + px]);
            }
        }
        m
    } else {
        MaskF32 {
            w,
            h,
            data: inv.into_iter().map(norm).collect(),
        }
    };
    m.data.shrink_to_fit();
    Some(m)
}

/// Mean filter with an integral image (radius in pixels, clamped borders).
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut sat = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, orow)| {
            let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
            for (x, o) in orow.iter_mut().enumerate() {
                let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
                let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
                    + sat[y0 * (w + 1) + x0];
                *o = (s / ((x1 - x0) * (y1 - y0)) as f64) as f32;
            }
        });
    out
}

fn guide_luma(c: [f32; 3]) -> f32 {
    linear_to_srgb(0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
}

/// Fast guided filter upsampling (He & Sun 2015): fit `near ≈ a·I + b` locally
/// at depth resolution using the photo's luma `I` as guide, then evaluate the
/// smoothly upsampled `a, b` against the full-resolution luma. Depth edges
/// follow image edges (hair, leaves) instead of the network's blurry ones.
pub fn upsample_guided(near: &MaskF32, img: &ImageF32) -> MaskF32 {
    let (dw, dh) = (near.w, near.h);
    let factor = (img.w / dw).min(img.h / dh).max(1);
    let guide_small = img.downsample(factor).resize(dw, dh);
    let i_lo: Vec<f32> = guide_small
        .data
        .chunks_exact(3)
        .map(|c| guide_luma([c[0], c[1], c[2]]))
        .collect();
    let p = &near.data;
    // radius ≈ 5 px at 896 px; eps tuned on phone photos: smaller copies
    // texture into depth (sharp specks in the blur), larger loses edges
    let r = (dw.max(dh) / 180).max(2);
    let eps = 0.01;
    let mi = box_mean(&i_lo, dw, dh, r);
    let mp = box_mean(p, dw, dh, r);
    let ii: Vec<f32> = i_lo.iter().map(|v| v * v).collect();
    let ip: Vec<f32> = i_lo.iter().zip(p).map(|(a, b)| a * b).collect();
    let mii = box_mean(&ii, dw, dh, r);
    let mip = box_mean(&ip, dw, dh, r);
    let mut a = vec![0.0f32; dw * dh];
    let mut b = vec![0.0f32; dw * dh];
    for k in 0..dw * dh {
        let var = mii[k] - mi[k] * mi[k];
        let cov = mip[k] - mi[k] * mp[k];
        a[k] = cov / (var + eps);
        b[k] = mp[k] - a[k] * mi[k];
    }
    let a = box_mean(&a, dw, dh, r);
    let b = box_mean(&b, dw, dh, r);
    // Bilinear taps of the coefficient maps: the same per column / per row
    // for the whole image, so compute them once (bit-identical to
    // `MaskF32::sample_uv`, minus 2 floor/clamp sets per pixel).
    let (w, h) = (img.w, img.h);
    let taps = |n: usize, src_n: usize| -> Vec<(usize, usize, f32)> {
        (0..n)
            .map(|i| {
                let f = (i as f32 + 0.5) / n as f32 * src_n as f32 - 0.5;
                let f0 = f.floor();
                let i0 = f0 as isize;
                let clamp = |v: isize| v.clamp(0, src_n as isize - 1) as usize;
                (clamp(i0), clamp(i0 + 1), f - f0)
            })
            .collect()
    };
    let (cols, rows) = (taps(w, dw), taps(h, dh));
    let lerp2 =
        |m: &[f32], (y0, y1, ty): (usize, usize, f32), (x0, x1, tx): (usize, usize, f32)| {
            let top = m[y0 * dw + x0] + (m[y0 * dw + x1] - m[y0 * dw + x0]) * tx;
            let bot = m[y1 * dw + x0] + (m[y1 * dw + x1] - m[y1 * dw + x0]) * tx;
            top + (bot - top) * ty
        };
    let mut out = MaskF32::new(w, h);
    out.data
        .par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            let ry = rows[y];
            let src = &img.data[y * w * 3..(y + 1) * w * 3];
            for ((o, c), &cx) in row.iter_mut().zip(src.chunks_exact(3)).zip(&cols) {
                let i = guide_luma([c[0], c[1], c[2]]);
                *o = (lerp2(&a, ry, cx) * i + lerp2(&b, ry, cx)).clamp(0.0, 1.0);
            }
        });
    out
}

/// Nearness at the focus point: median over a small window, so tapping a
/// thin edge or a noisy pixel still focuses on the dominant surface.
pub fn focus_nearness(near: &MaskF32, center: [f32; 2]) -> f32 {
    // ≈1.7 % of the long edge: big enough to outvote an edge you tapped on
    let r = (near.w.max(near.h) / 60).max(1) as isize;
    let cx = (center[0].clamp(0.0, 1.0) * near.w as f32) as isize;
    let cy = (center[1].clamp(0.0, 1.0) * near.h as f32) as isize;
    let mut v: Vec<f32> = Vec::with_capacity(((2 * r + 1) * (2 * r + 1)) as usize);
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            v.push(near.get_clamped(x, y));
        }
    }
    v.sort_unstable_by(f32::total_cmp);
    v[v.len() / 2]
}

/// Depth-driven defocus for one photo.
#[derive(Clone, Debug)]
pub struct DepthField {
    /// Nearness at photo resolution.
    pub near: MaskF32,
    /// Nearness of the focused surface.
    pub focus: f32,
}

/// Nearness difference that reaches the lens's maximum blur (scaled per lens
/// by `depth_scale`). Half the scene's depth range ≈ background fully soft.
pub const FULL_BLUR_DELTA: f32 = 0.45;
/// Depth of field around the focus plane that stays perfectly sharp. Network
/// depth varies by up to ±0.1 across one solid object (a scooter's front on
/// phone shots), so a smaller band blurs the very thing you focused on.
pub const SHARP_DELTA: f32 = 0.08;

/// Normalised defocus (0 sharp … 1 max blur) for a nearness value. Eased in
/// (smoothstep) so surfaces just beyond the focus band soften gently while the
/// background still gets most of the blur. Mirrored in common.glsl.
#[inline]
pub fn defocus(near: f32, focus: f32, scale: f32) -> f32 {
    if scale <= 0.0 {
        return 0.0; // e.g. pinhole: no focus plane
    }
    let d = (near - focus).abs();
    let full = (FULL_BLUR_DELTA / scale.max(0.05)).max(SHARP_DELTA * 2.0);
    crate::color::smoothstep(SHARP_DELTA, full, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearness_inverts_depth_and_normalises() {
        // left column far (depth 4), right column near (depth 1)
        let raw = [4.0, 1.0, 4.0, 1.0];
        let m = nearness(&raw, 2, 2, DepthKind::Depth, 2, 2).unwrap();
        assert!(m.data[0] < 0.01 && m.data[1] > 0.99, "{:?}", m.data);
        let inv = nearness(&[0.0, 9.0, 0.0, 9.0], 2, 2, DepthKind::InverseDepth, 2, 2).unwrap();
        assert_eq!(m.data, inv.data);
    }

    #[test]
    fn nearness_rejects_bad_output() {
        assert!(
            nearness(&[1.0; 4], 2, 2, DepthKind::Depth, 2, 2).is_none(),
            "flat"
        );
        assert!(
            nearness(&[1.0, f32::NAN, 2.0, 3.0], 2, 2, DepthKind::Depth, 2, 2).is_none(),
            "nan"
        );
        assert!(
            nearness(&[1.0; 3], 2, 2, DepthKind::Depth, 2, 2).is_none(),
            "size"
        );
    }

    #[test]
    fn nearness_rejects_overflowing_size() {
        // 2^32 · 2^32 wraps to 0 = raw.len() in release builds
        let big = 1usize << (usize::BITS / 2);
        assert!(nearness(&[], big, big, DepthKind::Depth, 2, 2).is_none());
    }

    #[test]
    fn nearness_percentiles_match_sorted() {
        let raw: Vec<f32> = (0..1000)
            .map(|i| ((i * 7919) % 1000) as f32 + 1.0)
            .collect();
        let m = nearness(&raw, 25, 40, DepthKind::InverseDepth, 25, 40).unwrap();
        let mut s = raw.clone();
        s.sort_by(f32::total_cmp);
        let (lo, hi) = (s[10], s[990]);
        for (v, r) in m.data.iter().zip(&raw) {
            assert_eq!(*v, ((r - lo) / (hi - lo)).clamp(0.0, 1.0));
        }
    }

    #[test]
    fn model_input_rejects_bad_layouts() {
        let img = ImageF32::filled(8, 6, [0.5; 3]);
        for (w, h) in [(0, 4), (4, 0), (MAX_LAYOUT_SIDE + 1, 4)] {
            let l = ModelLayout { w, h, nchw: true };
            assert!(l.validate().is_err());
            assert!(model_input(&img, l).is_empty(), "{w}x{h}");
        }
        let ok = ModelLayout {
            w: 4,
            h: 4,
            nchw: false,
        };
        assert_eq!(model_input(&img, ok).len(), 48);
    }

    #[test]
    fn square_layout_keeps_landscape_upright() {
        // landscape 4x2 photo with a marked top-left pixel; a square network
        // input (segmentation) must see it top-left, not rotated to top-right
        let mut img = ImageF32::filled(4, 2, [0.0; 3]);
        img.set(0, 0, [1.0; 3]);
        let layout = ModelLayout {
            w: 4,
            h: 4,
            nchw: true,
        };
        assert!(!rotates(4, 2, 4, 4));
        assert!(rotates(4, 2, 2, 4), "portrait layouts still rotate");
        assert!(!rotates(2, 4, 2, 4));
        let t = model_input(&img, layout);
        let brightest = t[..16]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(brightest, 0, "marked pixel moved: {:?}", &t[..16]);
    }

    #[test]
    fn landscape_rotation_round_trips() {
        // landscape 4x2 photo: left half bright (marks a position)
        let mut img = ImageF32::filled(4, 2, [0.0; 3]);
        img.set(0, 0, [1.0; 3]);
        let layout = ModelLayout {
            w: 2,
            h: 4,
            nchw: true,
        };
        let t = model_input(&img, layout);
        // the network "predicts" nearness = its input brightness (channel 0)
        let raw: Vec<f32> = t[..8].to_vec();
        let m = nearness(&raw, 2, 4, DepthKind::InverseDepth, 4, 2).unwrap();
        assert_eq!((m.w, m.h), (4, 2));
        let brightest = m
            .data
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(
            brightest, 0,
            "photo top-left must stay top-left: {:?}",
            m.data
        );
    }

    #[test]
    fn guided_upsample_snaps_depth_to_image_edges() {
        // photo: dark left half, bright right half (edge at x = 64 of 128)
        let mut img = ImageF32::filled(128, 128, [0.05; 3]);
        for y in 0..128 {
            for x in 64..128 {
                img.set(x, y, [0.8; 3]);
            }
        }
        // blurry low-res depth: ramp across the edge (what a network outputs)
        let mut near = MaskF32::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                near.data[y * 32 + x] = ((x as f32 - 12.0) / 8.0).clamp(0.0, 1.0);
            }
        }
        let up = upsample_guided(&near, &img);
        let (l, r) = (up.data[64 * 128 + 58], up.data[64 * 128 + 70]);
        // plain bilinear upsampling of the same map, for comparison
        let (bl, br) = (near.sample(58.5 / 4.0, 16.0), near.sample(70.5 / 4.0, 16.0));
        assert!(
            r - l > br - bl + 0.1,
            "guided {l}..{r} not sharper than bilinear {bl}..{br}"
        );
    }

    #[test]
    fn defocus_is_zero_at_focus_and_saturates() {
        assert_eq!(defocus(0.6, 0.6, 1.0), 0.0);
        assert_eq!(defocus(0.67, 0.6, 1.0), 0.0, "inside depth of field");
        assert_eq!(defocus(0.0, 0.6, 1.0), 1.0);
        assert!(
            defocus(0.5, 0.6, 1.0) < 0.02,
            "just past the focus band stays nearly sharp"
        );
        assert!(
            defocus(0.3, 0.6, 1.0) > 0.6,
            "background gets most of the blur"
        );
        assert!(
            defocus(0.4, 0.6, 2.0) > defocus(0.4, 0.6, 1.0),
            "scale = stronger falloff"
        );
        assert_eq!(defocus(0.0, 0.6, 0.0), 0.0, "scale 0 = no depth blur");
    }

    #[test]
    fn focus_uses_median() {
        let mut m = MaskF32::new(100, 100);
        m.data.iter_mut().for_each(|v| *v = 0.3);
        m.data[50 * 100 + 50] = 1.0; // one outlier at the tap point
        assert_eq!(focus_nearness(&m, [0.505, 0.505]), 0.3);
    }
}
