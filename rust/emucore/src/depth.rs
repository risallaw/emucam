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
    let mut m = inverse_depth(raw, w, h, kind, img_w, img_h)?;
    normalize(&mut m.data)?;
    m.data.shrink_to_fit();
    Some(m)
}

/// Raw output → un-normalised inverse depth in the photo's orientation, or
/// None for a malformed map (wrong size, NaN / inf).
fn inverse_depth(
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
    let inv = |v: f32| match kind {
        DepthKind::Depth => 1.0 / v.max(1e-3),
        DepthKind::InverseDepth => v,
    };
    Some(if rotates(img_w, img_h, w, h) {
        // undo the 90° cw rotation
        let mut m = MaskF32::new(h, w);
        for py in 0..h {
            for px in 0..w {
                // portrait (px, py) came from photo (py, w-1-px)
                let (x, y) = (py, w - 1 - px);
                m.data[y * h + x] = inv(raw[py * w + px]);
            }
        }
        m
    } else {
        MaskF32 {
            w,
            h,
            data: raw.iter().map(|&v| inv(v)).collect(),
        }
    })
}

/// Map values to 0..1 between their 1st and 99th percentile, in place.
/// None when that range is empty (a flat map carries no depth information).
fn normalize(data: &mut [f32]) -> Option<()> {
    // Two order statistics: selection is O(n) vs a full sort (same values).
    let mut sorted = data.to_vec();
    let n = sorted.len();
    let hi = *sorted
        .select_nth_unstable_by(n * 99 / 100, f32::total_cmp)
        .1;
    let lo = *sorted[..n * 99 / 100 + 1]
        .select_nth_unstable_by(n / 100, f32::total_cmp)
        .1;
    if hi - lo <= 1e-6 || hi.is_nan() || lo.is_nan() {
        return None;
    }
    data.iter_mut()
        .for_each(|v| *v = ((*v - lo) / (hi - lo)).clamp(0.0, 1.0));
    Some(())
}

/// Detail tiles per axis for boosted (tiled) depth inference.
pub const TILE_GRID: usize = 2;
/// Tile side as a fraction of the photo side. 2×2 tiles of 0.6 give the
/// network ~1.7× the pixels per object, enough to resolve bottle caps,
/// handles and thin poles the whole-frame pass loses; 3×3 tiles of 0.45
/// (desktop-tested) start to lack context and invent depth blobs.
pub const TILE_SIZE: f32 = 0.6;
/// Tile detail is taken at scales below ~1/DETAIL_SCALE of the long side
/// (box radius of the low-pass split).
const DETAIL_SCALE: usize = 48;

/// Normalised photo-space rects `[x0, y0, x1, y1]` of the detail tiles, in
/// the order their network outputs are passed to [`nearness_boosted`].
pub fn detail_tiles() -> Vec<[f32; 4]> {
    let mut v = Vec::with_capacity(TILE_GRID * TILE_GRID);
    for j in 0..TILE_GRID {
        for i in 0..TILE_GRID {
            let step = |k: usize| (1.0 - TILE_SIZE) * k as f32 / (TILE_GRID - 1).max(1) as f32;
            let (x0, y0) = (step(i), step(j));
            v.push([x0, y0, x0 + TILE_SIZE, y0 + TILE_SIZE]);
        }
    }
    v
}

/// Integer pixel bounds of a normalised rect in a `w`×`h` image (non-empty).
fn rect_px(r: [f32; 4], w: usize, h: usize) -> (usize, usize, usize, usize) {
    let x0 = ((r[0] * w as f32) as usize).min(w - 1);
    let y0 = ((r[1] * h as f32) as usize).min(h - 1);
    let x1 = ((r[2] * w as f32).round() as usize).clamp(x0 + 1, w);
    let y1 = ((r[3] * h as f32).round() as usize).clamp(y0 + 1, h);
    (x0, y0, x1, y1)
}

/// Network input for one detail tile (see [`detail_tiles`]): the photo
/// cropped to `rect`, then prepared like [`model_input`].
pub fn model_input_region(img: &ImageF32, layout: ModelLayout, rect: [f32; 4]) -> Vec<f32> {
    if img.w == 0 || img.h == 0 {
        return Vec::new();
    }
    let (x0, y0, x1, y1) = rect_px(rect, img.w, img.h);
    let mut crop = ImageF32::new(x1 - x0, y1 - y0);
    for y in y0..y1 {
        let src = &img.data[(y * img.w + x0) * 3..(y * img.w + x1) * 3];
        let dst = (y - y0) * crop.w * 3;
        crop.data[dst..dst + src.len()].copy_from_slice(src);
    }
    model_input(&crop, layout)
}

/// "Boosted" monocular depth (after Miangoleh et al., CVPR 2021): the
/// whole-frame pass gets the scene layout right but loses thin and small
/// objects; tiles see them but each has its own unknown scale/shift and
/// little context. Each tile is fitted to the whole-frame map (least squares,
/// inverse depth), the tiles are feather-blended, and only their fine detail
/// is kept on top of the whole-frame map's coarse structure.
///
/// `global` and every entry of `tiles` (in [`detail_tiles`] order) are raw
/// outputs with the same `w`×`h` layout. Unusable tiles are skipped; with no
/// usable tile this is exactly [`nearness`].
pub fn nearness_boosted(
    global: &[f32],
    tiles: &[&[f32]],
    w: usize,
    h: usize,
    kind: DepthKind,
    img_w: usize,
    img_h: usize,
) -> Option<MaskF32> {
    let g0 = inverse_depth(global, w, h, kind, img_w, img_h)?;
    let rects = detail_tiles();
    if tiles.len() != rects.len() || img_w == 0 || img_h == 0 {
        return nearness(global, w, h, kind, img_w, img_h);
    }
    // Merge grid: tile resolution over the whole frame, photo aspect.
    let long = (g0.w.max(g0.h) as f32 / TILE_SIZE).round() as usize;
    let (ow, oh) = if img_w >= img_h {
        (long, (long * img_h / img_w).max(1))
    } else {
        ((long * img_w / img_h).max(1), long)
    };
    let resample = |m: &MaskF32, ow: usize, oh: usize| -> Vec<f32> {
        let (sx, sy) = (m.w as f32 / ow as f32, m.h as f32 / oh as f32);
        (0..ow * oh)
            .map(|i| {
                m.sample(
                    (i % ow) as f32 * sx + 0.5 * sx,
                    (i / ow) as f32 * sy + 0.5 * sy,
                )
            })
            .collect()
    };
    let g = resample(&g0, ow, oh);
    let mut acc = vec![0.0f64; ow * oh];
    let mut wac = vec![0.0f64; ow * oh];
    let mut used = 0;
    for (raw, &rect) in tiles.iter().zip(&rects) {
        let (x0, y0, x1, y1) = rect_px(rect, ow, oh);
        let (tw, th) = (x1 - x0, y1 - y0);
        // the tile was cropped from the photo with the same aspect, so it
        // rotates exactly like the whole frame
        let Some(t0) = inverse_depth(raw, w, h, kind, img_w, img_h) else {
            continue;
        };
        let t = resample(&t0, tw, th);
        // least-squares g ≈ a·t + b over the tile
        let (mut st, mut sg, mut stt, mut stg) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for y in 0..th {
            for x in 0..tw {
                let tv = t[y * tw + x] as f64;
                let gv = g[(y0 + y) * ow + x0 + x] as f64;
                st += tv;
                sg += gv;
                stt += tv * tv;
                stg += tv * gv;
            }
        }
        let n = (tw * th) as f64;
        let var = stt - st * st / n;
        if var <= 1e-12 {
            continue;
        }
        let a = (stg - st * sg / n) / var;
        let b = (sg - a * st) / n;
        if a.is_nan() || a <= 0.0 || !b.is_finite() {
            continue; // inverted or degenerate fit: the tile is unreliable
        }
        used += 1;
        for y in 0..th {
            // tent weight: 1 in the middle, → 0 at the tile border
            let fy = (((y + 1).min(th - y)) as f64 / (0.5 * th as f64)).min(1.0);
            for x in 0..tw {
                let fx = (((x + 1).min(tw - x)) as f64 / (0.5 * tw as f64)).min(1.0);
                let wt = fx * fy + 1e-4;
                let k = (y0 + y) * ow + x0 + x;
                acc[k] += (a * t[y * tw + x] as f64 + b) * wt;
                wac[k] += wt;
            }
        }
    }
    if used == 0 {
        return nearness(global, w, h, kind, img_w, img_h);
    }
    let d: Vec<f32> = (0..ow * oh)
        .map(|k| {
            if wac[k] > 0.0 {
                (acc[k] / wac[k]) as f32
            } else {
                g[k]
            }
        })
        .collect();
    // Coarse structure from the whole frame; from the tiles only *small*
    // structures that come *forward* (caps, poles, handles in front of what
    // is behind them). Tiles lack context, so on flat or ambiguous surfaces
    // they invent depth steps — a laptop screen's content pushed half the
    // screen back and blurred it (device check); larger-scale or receding
    // tile detail is therefore ignored.
    let r = (ow.max(oh) / DETAIL_SCALE).max(1);
    let lp = |v: &[f32]| box_mean(&box_mean(v, ow, oh, r), ow, oh, r);
    let (gl, dl) = (lp(&g), lp(&d));
    let mut data: Vec<f32> = (0..ow * oh)
        .map(|k| {
            let fine_g = g[k] - gl[k];
            let fine_d = d[k] - dl[k];
            gl[k] + fine_g.max(fine_d)
        })
        .collect();
    normalize(&mut data)?;
    Some(MaskF32 { w: ow, h: oh, data })
}

/// Mean filter with an integral image (radius in pixels, clamped borders).
pub(crate) fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
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

    /// Raw (depth-kind) map in the photo's own orientation (square, so no
    /// rotation): far background 4, a near square 1, plus an optional thin
    /// near pole the whole-frame pass "misses".
    fn scene(n: usize, pole: bool) -> Vec<f32> {
        (0..n * n)
            .map(|i| {
                let (x, y) = (i % n, i / n);
                let sq = (n / 4..n / 2).contains(&x) && (n / 4..n / 2).contains(&y);
                let p = pole && (3 * n / 4..3 * n / 4 + n / 60).contains(&x) && y > n / 8;
                if sq || p {
                    1.0
                } else {
                    4.0
                }
            })
            .collect()
    }

    #[test]
    fn boosted_without_usable_tiles_is_plain_nearness() {
        let g = scene(40, false);
        let plain = nearness(&g, 40, 40, DepthKind::Depth, 40, 40).unwrap();
        let bad = vec![f32::NAN; 1600];
        let tiles: Vec<&[f32]> = vec![&bad; TILE_GRID * TILE_GRID];
        let b = nearness_boosted(&g, &tiles, 40, 40, DepthKind::Depth, 40, 40).unwrap();
        assert_eq!(b.data, plain.data);
        let few: Vec<&[f32]> = vec![&g];
        let b = nearness_boosted(&g, &few, 40, 40, DepthKind::Depth, 40, 40).unwrap();
        assert_eq!(b.data, plain.data, "wrong tile count falls back");
    }

    #[test]
    fn boosted_recovers_thin_detail_seen_only_by_tiles() {
        // whole-frame pass misses a thin near pole; tiles (cropped from a
        // scene that has it, rendered at the same layout) see it, with their
        // own arbitrary scale — the merge must keep the pole and the layout
        let n = 360;
        let full = scene(n, true);
        let g = scene(n, false);
        let tiles: Vec<Vec<f32>> = detail_tiles()
            .iter()
            .enumerate()
            .map(|(k, r)| {
                let (x0, y0, x1, y1) = rect_px(*r, n, n);
                (0..n * n)
                    .map(|i| {
                        let (x, y) = (i % n, i / n);
                        let sx = x0 + x * (x1 - x0) / n;
                        let sy = y0 + y * (y1 - y0) / n;
                        full[sy * n + sx] * (1.0 + k as f32) // per-tile scale
                    })
                    .collect()
            })
            .collect();
        let t: Vec<&[f32]> = tiles.iter().map(|v| v.as_slice()).collect();
        let b = nearness_boosted(&g, &t, n, n, DepthKind::Depth, n, n).unwrap();
        let at = |fx: f32, fy: f32| b.sample(fx * b.w as f32, fy * b.h as f32);
        let plain = nearness(&g, n, n, DepthKind::Depth, n, n).unwrap();
        let pole_plain = plain.sample(0.75 * n as f32 + 3.0, 0.6 * n as f32);
        let pole = at(0.75 + 3.0 / n as f32, 0.6);
        assert!(
            pole > pole_plain + 0.3,
            "pole {pole} vs whole-frame {pole_plain}"
        );
        assert!(at(0.37, 0.37) > 0.8, "near square kept: {}", at(0.37, 0.37));
        assert!(at(0.1, 0.9) < 0.2, "background kept far: {}", at(0.1, 0.9));
    }

    #[test]
    fn detail_tiles_cover_the_frame() {
        let t = detail_tiles();
        assert_eq!(t.len(), TILE_GRID * TILE_GRID);
        for (x, y) in [
            (0.0, 0.0),
            (0.999, 0.0),
            (0.5, 0.5),
            (0.0, 0.999),
            (0.999, 0.999),
        ] {
            assert!(t
                .iter()
                .any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]));
        }
    }

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
