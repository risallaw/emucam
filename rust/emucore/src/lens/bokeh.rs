//! Bokeh rendering. Scatter-as-gather: every output pixel sums contributions
//! of neighbours whose (position-dependent) kernel covers it, so each source
//! pixel spreads light in *its own* aperture shape — this is what makes swirl,
//! cat's-eye and creative shapes work without depth.
//!
//! Multi-level for speed: small radii are gathered at full resolution, large
//! radii on 1/4 and 1/16 downsampled copies (≤16 px kernel radius per level),
//! then levels are cross-faded by each pixel's own CoC.

use super::{aperture, Defocus, Geometry};
use crate::color::smoothstep;
use crate::depth::DepthField;
use crate::image::{ImageF32, MaskF32};
use crate::preset::{Aperture, LensOptics};
use rayon::prelude::*;

/// Kernel-shaping parameters that vary with image position.
#[derive(Clone, Debug)]
pub struct KernelParams {
    pub aperture: Aperture,
    pub area: f32,
    pub swirl: f32,
    pub cats_eye: f32,
    pub bubble: f32,
    pub onion: f32,
    pub ca_long: f32,
}

impl KernelParams {
    pub fn from_optics(o: &LensOptics) -> Self {
        Self {
            aperture: o.aperture.clone(),
            area: aperture::area(&o.aperture),
            swirl: o.swirl,
            cats_eye: o.cats_eye,
            bubble: o.bubble,
            onion: o.onion,
            ca_long: o.ca_long,
        }
    }
}

/// Per-source-pixel kernel geometry (precomputed once, reused for every tap).
#[derive(Clone, Copy, Default)]
pub struct SrcGeo {
    /// Radial position 0 (axis) … ~1 (corner).
    pub e: f32,
    /// Radial unit vector.
    pub u: [f32; 2],
    /// 1/radial and 1/tangential scale from swirl.
    pub inv_r: f32,
    pub inv_t: f32,
}

impl SrcGeo {
    #[inline]
    pub fn new(k: &KernelParams, g: &Geometry, px: f32, py: f32) -> Self {
        let ax = px - g.axis[0];
        let ay = py - g.axis[1];
        let al = (ax * ax + ay * ay).sqrt();
        let e = (al / g.half_diag).min(1.2);
        let u = if al > 1e-3 {
            [ax / al, ay / al]
        } else {
            [1.0, 0.0]
        };
        let (inv_r, inv_t) = if k.swirl > 0.0 {
            // Field curvature/astigmatism: stretch tangentially, squeeze radially.
            let s = k.swirl * e.powf(1.7);
            (1.0 / (1.0 - 0.3 * s).max(0.35), 1.0 / (1.0 + s))
        } else {
            (1.0, 1.0)
        };
        SrcGeo { e, u, inv_r, inv_t }
    }
}

/// Kernel weight for unit offset `o` (|o|≤1 ≈ inside), at radial position
/// `e` (0 center … 1 corner) with radial unit vector `u`.
#[inline]
pub fn kernel(k: &KernelParams, o: [f32; 2], e: f32, u: [f32; 2]) -> f32 {
    let sg = SrcGeo {
        e,
        u,
        inv_r: 1.0,
        inv_t: 1.0,
    };
    let sg = if k.swirl > 0.0 {
        let s = k.swirl * e.powf(1.7);
        SrcGeo {
            inv_r: 1.0 / (1.0 - 0.3 * s).max(0.35),
            inv_t: 1.0 / (1.0 + s),
            ..sg
        }
    } else {
        sg
    };
    kernel_g(k, o, &sg)
}

#[inline(always)]
pub fn kernel_g(k: &KernelParams, o: [f32; 2], sg: &SrcGeo) -> f32 {
    let u = sg.u;
    let e = sg.e;
    let t = [-u[1], u[0]];
    let mut q = o;
    if k.swirl > 0.0 {
        let r2 = (o[0] * u[0] + o[1] * u[1]) * sg.inv_r;
        let t2 = (o[0] * t[0] + o[1] * t[1]) * sg.inv_t;
        q = [r2 * u[0] + t2 * t[0], r2 * u[1] + t2 * t[1]];
    }
    let mut w = aperture::weight(&k.aperture, q);
    if w <= 0.0 {
        return 0.0;
    }
    if k.cats_eye > 0.0 {
        // Mechanical vignetting: intersect with a second disc shifted radially.
        let d = 0.5 * k.cats_eye * e;
        let qr = q[0] * u[0] + q[1] * u[1];
        let qt = q[0] * t[0] + q[1] * t[1];
        let a = ((qr + d) * (qr + d) + qt * qt).sqrt();
        let b = ((qr - d) * (qr - d) + qt * qt).sqrt();
        w *= (1.0 - smoothstep(0.92, 1.0, a)) * (1.0 - smoothstep(0.92, 1.0, b));
    }
    let rho = (q[0] * q[0] + q[1] * q[1]).sqrt().min(1.0);
    if k.bubble > 0.0 {
        // Over-corrected SA: bright rim, dim center (soap bubble).
        let r4 = rho * rho * rho * rho;
        w *= 1.0 + k.bubble * (0.2 + 2.2 * r4 - 1.0);
    } else if k.bubble < 0.0 {
        // Under-corrected: soft-edged, smooth bokeh.
        w *= 1.0 + (-k.bubble) * ((1.0 - rho * rho) - 1.0);
    }
    if k.onion > 0.0 {
        w *= 1.0 + k.onion * 0.18 * (40.0 * rho).sin();
    }
    w.max(0.0)
}

/// One pyramid level. Buffers are borrowed so level 0 can reuse the
/// full-res HDR image, CoC and nearness maps instead of copying them
/// (saves ~250 MB and ~60 ms at 12 MP).
struct Level<'a> {
    /// Downsampling factor (full-res px per level px).
    factor: f32,
    /// Max kernel radius in level pixels.
    rmax: f32,
    /// Only pixels whose footprint holds a full-res CoC in [lo, hi] need output.
    lo: f32,
    hi: f32,
    img: &'a ImageF32,
    /// CoC in level px, clamped to rmax (used for kernels).
    coc: &'a MaskF32,
    /// Min / max full-res CoC over each level pixel's footprint, dilated by
    /// one level pixel (the composite samples levels bilinearly). A pixel is
    /// gathered when that range meets [lo, hi]. Using only the CoC at the
    /// pixel centre left pixels unrendered (black) wherever the CoC jumps
    /// inside a footprint — i.e. at every depth edge. Level 0 is read
    /// without interpolation, so there both are simply the CoC map.
    own_min: &'a MaskF32,
    own_max: &'a MaskF32,
    /// Nearness per level pixel (depth map present) for occlusion.
    near: Option<&'a MaskF32>,
    /// Near-field overlay: `Some(share)` where `share(r)` is this level's
    /// part of a source with full-res CoC `r` in the composite cross-fade.
    near_field: Option<&'a (dyn Fn(f32) -> f32 + Sync)>,
}

/// Gathered level: blurred image plus, for the near field, premultiplied
/// colour + coverage (RGBA per pixel) of nearer sources spreading over it.
struct Gathered {
    img: ImageF32,
    near: Option<Vec<f32>>,
}

/// Nearness margin beyond which a source counts as *behind* the destination.
const BEHIND: f32 = 0.03;

/// A blurred background can't spread light over a sharper object in front of
/// it (the object occludes it). Scale a behind-source's weight by how sharp
/// the destination is relative to the source. `behind_dst` is the
/// destination's nearness minus [`BEHIND`] (hoisted out of the tap loop).
#[inline]
fn occlusion(
    near: Option<&MaskF32>,
    behind_dst: f32,
    src: usize,
    coc_dst: f32,
    coc_src: f32,
) -> f32 {
    match near {
        Some(n) if n.data[src] < behind_dst => (coc_dst / coc_src.max(1e-3)).min(1.0),
        _ => 1.0,
    }
}

/// Gather one level. CoC in level pixels, clamped to `rmax`.
fn gather(
    level: &Level,
    kp: &KernelParams,
    g: &Geometry,
    full_w: usize,
    full_h: usize,
) -> Gathered {
    let (w, h) = (level.img.w, level.img.h);

    let has_ca = kp.ca_long > 0.0;
    let sx = full_w as f32 / w as f32;
    let sy = full_h as f32 / h as f32;
    let simple =
        !has_ca && kp.swirl <= 0.0 && kp.cats_eye <= 0.0 && kp.bubble == 0.0 && kp.onion <= 0.0;

    // Tile max of CoC → per-pixel gather reach (no point looking further than
    // the largest kernel nearby).
    const T: usize = 8;
    let (tw, th) = (w.div_ceil(T), h.div_ceil(T));
    let mut tmax = vec![0.0f32; tw * th];
    for ty in 0..th {
        for tx in 0..tw {
            let mut m = 0.0f32;
            for y in ty * T..((ty + 1) * T).min(h) {
                for x in tx * T..((tx + 1) * T).min(w) {
                    m = m.max(level.coc.data[y * w + x]);
                }
            }
            tmax[ty * tw + tx] = m;
        }
    }
    // Near field: per tile, the max nearness of sources this level spreads
    // (share > 0); a destination can only receive near-field light if some
    // source within reach is nearer than it.
    let tnear: Vec<f32> = match (level.near, level.near_field) {
        (Some(n), Some(share)) => (0..tw * th)
            .map(|t| {
                let (tx, ty) = (t % tw, t / tw);
                let mut m = f32::NEG_INFINITY;
                for y in ty * T..((ty + 1) * T).min(h) {
                    for x in tx * T..((tx + 1) * T).min(w) {
                        let i = y * w + x;
                        if share(level.coc.data[i] * level.factor) > 0.0 {
                            m = m.max(n.data[i]);
                        }
                    }
                }
                m
            })
            .collect(),
        _ => Vec::new(),
    };
    // Dilate by the max reach in tiles. One tile was assumed (rmax ≤ T), but
    // with longitudinal CA the reach is up to rmax·(1+ca)+0.5 > T, which
    // silently dropped the outer fringe of big coarse-level bokeh.
    let max_reach = (level.rmax * (1.0 + kp.ca_long) + 0.5).ceil() as usize;
    let td = max_reach.div_ceil(T) as isize;
    let tile_dilate = |src: &[f32], init: f32| -> Vec<f32> {
        (0..tw * th)
            .map(|t| {
                let (tx, ty) = ((t % tw) as isize, (t / tw) as isize);
                let mut m = init;
                for ny in (ty - td).max(0)..=(ty + td).min(th as isize - 1) {
                    for nx in (tx - td).max(0)..=(tx + td).min(tw as isize - 1) {
                        m = m.max(src[ny as usize * tw + nx as usize]);
                    }
                }
                m
            })
            .collect()
    };
    let treach: Vec<f32> = tile_dilate(&tmax, 0.0)
        .into_iter()
        .map(|m| m * (1.0 + kp.ca_long) + 0.5)
        .collect();
    let tnear = if tnear.is_empty() {
        tnear
    } else {
        tile_dilate(&tnear, f32::NEG_INFINITY)
    };

    // Per-source kernel geometry (powf/sqrt per pixel), computed per band of
    // rows plus the gather reach above/below instead of for the whole level
    // up front: same values, but ~20 B/px (250 MB at 12 MP) never sits in
    // memory at once, and the simple-kernel path skips it entirely.
    let band = (h / 64).clamp(8, 32);
    let mut out = ImageF32::new(w, h);
    let mut near_out = (!tnear.is_empty()).then(|| vec![0.0f32; w * h * 4]);
    let band_fn = |b: usize, rows: &mut [f32], mut nrows: Option<&mut [f32]>| {
        let by0 = b * band;
        let gy0 = by0.saturating_sub(max_reach);
        let gy1 = (by0 + rows.len() / (w * 3) + max_reach).min(h);
        let gbase = gy0 * w;
        let geo: Vec<SrcGeo> = if simple {
            Vec::new()
        } else {
            (gbase..gy1 * w)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    SrcGeo::new(kp, g, (x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy)
                })
                .collect()
        };
        for (ry, row) in rows.chunks_exact_mut(w * 3).enumerate() {
            let y = by0 + ry;
            let mut nrows = nrows
                .as_deref_mut()
                .map(|n| &mut n[ry * w * 4..(ry + 1) * w * 4]);
            for x in 0..w {
                let di = y * w + x;
                let regular =
                    level.own_max.data[di] >= level.lo && level.own_min.data[di] <= level.hi;
                let near_dst = level.near.map_or(0.0, |n| n.data[di]);
                let in_front = near_dst + BEHIND;
                let near_on = !tnear.is_empty() && tnear[(y / T) * tw + x / T] > in_front;
                if !regular {
                    // not needed by the composite; keep the unblurred level pixel
                    row[x * 3..x * 3 + 3].copy_from_slice(&level.img.data[di * 3..di * 3 + 3]);
                    if !near_on {
                        continue;
                    }
                }
                let coc_dst = level.coc.data[di];
                let behind_dst = near_dst - BEHIND;
                let mut nacc = [0.0f32; 4];
                let reach_f =
                    treach[(y / T) * tw + x / T].min(level.rmax * (1.0 + kp.ca_long) + 0.5);
                let reach = reach_f.ceil() as isize;
                let mut acc = [0.0f32; 3];
                let mut wsum = [0.0f32; 3];
                let y0 = (y as isize - reach).max(0);
                let y1 = (y as isize + reach).min(h as isize - 1);
                let x0 = (x as isize - reach).max(0);
                let x1 = (x as isize + reach).min(w as isize - 1);
                for syi in y0..=y1 {
                    let dy = syi - y as isize;
                    let rowbase = syi as usize * w;
                    for sxi in x0..=x1 {
                        let dx = sxi - x as isize;
                        let si = rowbase + sxi as usize;
                        let rr = level.coc.data[si].max(0.5);
                        let dist2 = (dx * dx + dy * dy) as f32;
                        let rmax_ch = rr * (1.0 + kp.ca_long) + 0.5;
                        if dist2 > rmax_ch * rmax_ch {
                            continue;
                        }
                        // near field: a nearer source whose CoC covers
                        // this pixel spreads over it, however sharp it is
                        let nshare = match (near_on, level.near, level.near_field) {
                            (true, Some(n), Some(share)) if n.data[si] > in_front => {
                                share(rr * level.factor) * (1.0 - coc_dst / rr).clamp(0.0, 1.0)
                            }
                            _ => 0.0,
                        };
                        if !regular && nshare <= 0.0 {
                            continue;
                        }
                        let c = &level.img.data[si * 3..si * 3 + 3];
                        let center = dx == 0 && dy == 0;
                        let occ = if center {
                            1.0
                        } else {
                            occlusion(level.near, behind_dst, si, coc_dst, rr)
                        };
                        if occ <= 0.0 {
                            continue;
                        }
                        // offset from source to destination (scatter direction)
                        let o = [-(dx as f32), -(dy as f32)];
                        if has_ca {
                            let rs = [rr * (1.0 + kp.ca_long), rr, rr * (1.0 - kp.ca_long)];
                            for ch in 0..3 {
                                let inv = 1.0 / rs[ch];
                                let norm = 1.0 / (rs[ch] * rs[ch] * kp.area).max(1.0);
                                let wgt = if center && rs[ch] <= 0.75 {
                                    norm
                                } else {
                                    kernel_g(kp, [o[0] * inv, o[1] * inv], &geo[si - gbase]) * norm
                                } * occ;
                                acc[ch] += c[ch] * wgt;
                                wsum[ch] += wgt;
                                nacc[ch] += c[ch] * wgt * nshare;
                                if ch == 1 {
                                    nacc[3] += wgt * nshare;
                                }
                            }
                        } else {
                            let inv = 1.0 / rr;
                            let norm = 1.0 / (rr * rr * kp.area).max(1.0);
                            let wgt = if center && rr <= 0.75 {
                                1.0
                            } else if simple {
                                aperture::weight(&kp.aperture, [o[0] * inv, o[1] * inv]) * norm
                            } else {
                                kernel_g(kp, [o[0] * inv, o[1] * inv], &geo[si - gbase]) * norm
                            } * occ;
                            if wgt > 0.0 {
                                acc[0] += c[0] * wgt;
                                acc[1] += c[1] * wgt;
                                acc[2] += c[2] * wgt;
                                wsum[0] += wgt;
                                wsum[1] += wgt;
                                wsum[2] += wgt;
                                if nshare > 0.0 {
                                    let nw = wgt * nshare;
                                    nacc[0] += c[0] * nw;
                                    nacc[1] += c[1] * nw;
                                    nacc[2] += c[2] * nw;
                                    nacc[3] += nw;
                                }
                            }
                        }
                    }
                }
                if let Some(nr) = nrows.as_deref_mut() {
                    nr[x * 4..x * 4 + 4].copy_from_slice(&nacc);
                }
                if !regular {
                    continue;
                }
                let i = y * w + x;
                for ch in 0..3 {
                    row[x * 3 + ch] = if wsum[ch] > 1e-8 {
                        acc[ch] / wsum[ch]
                    } else {
                        level.img.data[i * 3 + ch]
                    };
                }
            }
        }
    };
    match near_out.as_mut() {
        Some(n) => out
            .data
            .par_chunks_mut(w * 3 * band)
            .zip(n.par_chunks_mut(w * 4 * band))
            .enumerate()
            .for_each(|(b, (rows, nrows))| band_fn(b, rows, Some(nrows))),
        None => out
            .data
            .par_chunks_mut(w * 3 * band)
            .enumerate()
            .for_each(|(b, rows)| band_fn(b, rows, None)),
    }
    Gathered {
        img: out,
        near: near_out,
    }
}

/// Map a lens `highlight_gain` G to the expansion curve constant so that a
/// fully clipped pixel expands to 1 + 4G.
pub fn highlight_k(gain: f32) -> f32 {
    let peak = 1.0 + 4.0 * gain.max(0.0);
    1.0 - (1.0 - HL_KNEE) / (peak - HL_KNEE).max(1.0 - HL_KNEE + 1e-3)
}

/// Highlight knee: values above this (linear ≈ sRGB 0.88) are treated as
/// possibly clipped and expanded.
pub const HL_KNEE: f32 = 0.75;

/// Expand near-clipped highlights (inverse Reinhard above the knee) so specular
/// points carry enough energy to show as bright bokeh shapes; `compress`
/// is the exact inverse, so uniform areas are unchanged.
#[inline]
pub fn expand(v: f32, k: f32) -> f32 {
    if v <= HL_KNEE {
        return v.max(0.0);
    }
    let x = ((v - HL_KNEE) / (1.0 - HL_KNEE)).min(0.999);
    HL_KNEE + (1.0 - HL_KNEE) * x / (1.0 - k * x)
}
#[inline]
pub fn compress(v: f32, k: f32) -> f32 {
    if v <= HL_KNEE {
        return v.max(0.0);
    }
    let y = (v - HL_KNEE) / (1.0 - HL_KNEE);
    HL_KNEE + (1.0 - HL_KNEE) * y / (1.0 + k * y)
}

/// Min and max of the full-res CoC over each level pixel's footprint
/// (the `factor`×`factor` block `ImageF32::downsample` averages, clipped at
/// the right/bottom edge), built from the previous level's block min / max
/// (`pmin`, `pmax`, half the factor): a block at factor 2f is exactly the 2×2
/// child blocks at factor f, and ceil(ceil(w/f)/2) = ceil(w/2f) keeps the
/// sizes in step. Cascading reads each level once instead of rescanning the
/// 12 MP map per level (~75 ms → ~10 ms on an M-series Mac).
fn block_range(pmin: &MaskF32, pmax: &MaskF32, lw: usize, lh: usize) -> (MaskF32, MaskF32) {
    let (pw, ph) = (pmin.w, pmin.h);
    let mut bmin = MaskF32::new(lw, lh);
    let mut bmax = MaskF32::new(lw, lh);
    bmin.data
        .par_chunks_exact_mut(lw)
        .zip(bmax.data.par_chunks_exact_mut(lw))
        .enumerate()
        .for_each(|(y, (rmin, rmax))| {
            for x in 0..lw {
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for py in 2 * y..(2 * y + 2).min(ph) {
                    for px in 2 * x..(2 * x + 2).min(pw) {
                        lo = lo.min(pmin.data[py * pw + px]);
                        hi = hi.max(pmax.data[py * pw + px]);
                    }
                }
                rmin[x] = lo;
                rmax[x] = hi;
            }
        });
    (bmin, bmax)
}

/// Grow a min (or max) map by one pixel: the composite samples coarse
/// levels bilinearly, so a level pixel feeds its neighbours too.
fn dilate(m: &MaskF32, pick: fn(f32, f32) -> f32, init: f32) -> MaskF32 {
    let (lw, lh) = (m.w, m.h);
    let mut o = MaskF32::new(lw, lh);
    o.data
        .par_chunks_exact_mut(lw)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let mut acc = init;
                for ny in y.saturating_sub(1)..(y + 2).min(lh) {
                    for nx in x.saturating_sub(1)..(x + 2).min(lw) {
                        acc = pick(acc, m.data[ny * lw + nx]);
                    }
                }
                *v = acc;
            }
        });
    o
}

/// Render lens blur. Returns the input unchanged when the lens has no blur.
pub fn render(img: &ImageF32, optics: &LensOptics, g: &Geometry) -> ImageF32 {
    render_with_depth(img, optics, g, None)
}

/// Render lens blur with optional depth-driven defocus (see [`Defocus`]).
pub fn render_with_depth(
    img: &ImageF32,
    optics: &LensOptics,
    g: &Geometry,
    depth: Option<&DepthField>,
) -> ImageF32 {
    let dfc = Defocus { optics, g, depth };
    let rmax_px = optics.rmax * img.w as f32;
    if rmax_px < 0.6 || optics.field == crate::preset::BlurField::None {
        return img.clone();
    }
    let kp = KernelParams::from_optics(optics);
    let k = highlight_k(optics.highlight_gain);
    let (w, h) = (img.w, img.h);

    // Full-res CoC map.
    let mut coc_full = MaskF32::new(w, h);
    coc_full
        .data
        .par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = dfc.at(x as f32 + 0.5, y as f32 + 0.5);
            }
        });

    // Expanded (HDR-ish) image for the blur path.
    // Hue-preserving: scale all channels by the expansion of the max channel.
    let mut hdr = img.clone();
    hdr.data.par_chunks_exact_mut(3).for_each(|p| {
        let m = p[0].max(p[1]).max(p[2]);
        if m > HL_KNEE {
            let f = expand(m, k) / m;
            p.iter_mut().for_each(|v| *v *= f);
        }
    });

    // Factor-2 pyramid. Level i (factor 2^i) gathers with a max kernel radius
    // of LRMAX[i] level px and covers full-res radii up to LRMAX[i]·2^i.
    // Fine levels use small kernels (many pixels, small blur — shape detail
    // invisible anyway); coarse levels use 8 px so big shaped bokeh stays crisp.
    const LRMAX: [f32; 8] = [3.0, 4.0, 6.0, 8.0, 8.0, 8.0, 8.0, 8.0];
    let mut specs: Vec<usize> = vec![1];
    while LRMAX[specs.len() - 1] * (*specs.last().unwrap() as f32) < rmax_px && specs.len() < 8 {
        specs.push(specs.last().unwrap() * 2);
    }
    // Gathered output per level with its (factor, rmax); level buffers are
    // dropped as soon as the level is gathered.
    let mut levels: Vec<(usize, f32, Gathered)> = Vec::with_capacity(specs.len());
    // Level i's share of a pixel with full-res CoC r in the composite
    // cross-fade below (kept in sync with it).
    let covers: Vec<f32> = specs
        .iter()
        .enumerate()
        .map(|(i, &f)| LRMAX[i] * f as f32)
        .collect();
    let share = |i: usize, r: f32| -> f32 {
        if r < 0.5 {
            return 0.0;
        }
        let mut remaining = 1.0f32;
        for (j, &cover) in covers.iter().enumerate().take(i + 1) {
            let wgt = if j + 1 == covers.len() {
                1.0
            } else {
                1.0 - smoothstep(cover * 0.75, cover, r)
            };
            let take = remaining * wgt;
            if j == i {
                return take;
            }
            remaining -= take;
            if remaining <= 1e-4 {
                return 0.0;
            }
        }
        0.0
    };
    // Undilated footprint min / max of the previous level (level 0: the CoC).
    let mut blocks: Option<(MaskF32, MaskF32)> = None;
    for (i, &factor) in specs.iter().enumerate() {
        let f = factor as f32;
        let lo = if i == 0 {
            0.5
        } else {
            0.70 * LRMAX[i - 1] * f / 2.0
        };
        let hi = if i + 1 == specs.len() {
            f32::INFINITY
        } else {
            LRMAX[i] * f + 1.0
        };
        let (limg_own, lcoc, lnear_own, own);
        // Near field on coarse levels only: level 0 spreads ≤ 3 px, which
        // its regular gather already covers for any blurred destination.
        let level_share = |r: f32| share(i, r);
        let near_field: Option<&(dyn Fn(f32) -> f32 + Sync)> =
            (depth.is_some() && i > 0).then_some(&level_share);
        let level = if factor == 1 {
            // Level 0 is the full-res image itself: reuse the buffers.
            lcoc = MaskF32 {
                w,
                h,
                data: coc_full.data.iter().map(|r| r.min(LRMAX[0])).collect(),
            };
            Level {
                factor: f,
                rmax: LRMAX[i],
                lo,
                hi,
                img: &hdr,
                coc: &lcoc,
                own_min: &coc_full,
                own_max: &coc_full,
                near: depth.map(|d| &d.near),
                near_field,
            }
        } else {
            limg_own = hdr.downsample(factor);
            let (lw, lh) = (limg_own.w, limg_own.h);
            let sx = w as f32 / lw as f32;
            let sy = h as f32 / lh as f32;
            let mut c = MaskF32::new(lw, lh);
            c.data
                .par_chunks_exact_mut(lw)
                .enumerate()
                .for_each(|(y, row)| {
                    for (x, v) in row.iter_mut().enumerate() {
                        let r = dfc.at((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
                        *v = (r / f).min(LRMAX[i]);
                    }
                });
            lcoc = c;
            lnear_own = depth.map(|d| {
                let mut n = MaskF32::new(lw, lh);
                n.data
                    .par_chunks_exact_mut(lw)
                    .enumerate()
                    .for_each(|(y, row)| {
                        for (x, v) in row.iter_mut().enumerate() {
                            *v = d.near.sample((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
                        }
                    });
                n
            });
            let (pmin, pmax) = blocks
                .as_ref()
                .map_or((&coc_full, &coc_full), |b| (&b.0, &b.1));
            let b = block_range(pmin, pmax, lw, lh);
            own = (
                dilate(&b.0, f32::min, f32::INFINITY),
                dilate(&b.1, f32::max, f32::NEG_INFINITY),
            );
            blocks = Some(b);
            Level {
                factor: f,
                rmax: LRMAX[i],
                lo,
                hi,
                img: &limg_own,
                coc: &lcoc,
                own_min: &own.0,
                own_max: &own.1,
                near: lnear_own.as_ref(),
                near_field,
            }
        };
        let t0 = std::time::Instant::now();
        let out = gather(&level, &kp, g, w, h);
        if std::env::var_os("EMUCAM_PROF").is_some() {
            let n = (0..level.own_min.data.len())
                .filter(|&k| level.own_max.data[k] >= level.lo && level.own_min.data[k] <= level.hi)
                .count();
            eprintln!(
                "  bokeh level x{factor} {}x{}: {:.2?}  active px {n} lo {:.1} hi {:.1}",
                level.img.w,
                level.img.h,
                t0.elapsed(),
                level.lo,
                level.hi
            );
        }
        levels.push((factor, LRMAX[i], out));
    }

    // Levels are gathered; free 12 B/px before the composite copy.
    drop(hdr);
    let composite = |p: &mut [f32], r: f32, u: f32, v: f32, x: usize, y: usize| {
        // Cross-fade levels: level i covers r ≤ rmax·factor_i (full-res px).
        let mut col = [0.0f32; 3];
        let mut remaining = 1.0f32;
        for (i, (factor, rmax, lout)) in levels.iter().enumerate() {
            let cover = rmax * *factor as f32;
            let is_last = i + 1 == levels.len();
            let wgt = if is_last {
                1.0
            } else {
                1.0 - smoothstep(cover * 0.75, cover, r)
            };
            let take = remaining * wgt;
            if take > 0.0 {
                let c = if *factor == 1 {
                    let j = (y * w + x) * 3;
                    [lout.img.data[j], lout.img.data[j + 1], lout.img.data[j + 2]]
                } else {
                    lout.img.sample_uv(u, v)
                };
                for ch in 0..3 {
                    col[ch] += c[ch] * take;
                }
                remaining -= take;
            }
            if remaining <= 1e-4 {
                break;
            }
        }
        let t = smoothstep(0.5, 1.5, r);
        let m = col[0].max(col[1]).max(col[2]);
        let f = if m > HL_KNEE { compress(m, k) / m } else { 1.0 };
        for ch in 0..3 {
            p[ch] = p[ch] + (col[ch] * f - p[ch]) * t;
        }
    };
    // Composite per pixel by its own CoC, then lay the near field over it.
    let mut out = img.clone();
    out.par_map_pixels(|x, y, p| {
        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        let r = coc_full.data[y * w + x];
        if r >= 0.5 {
            composite(p, r, u, v, x, y);
        }
        // Near field: nearer blurred content partly covers this pixel.
        let mut nf = [0.0f32; 4];
        for (factor, _, gl) in &levels {
            let Some(n) = gl.near.as_ref() else { continue };
            let lw = w.div_ceil(*factor);
            let lh = h.div_ceil(*factor);
            let px = u * lw as f32 - 0.5;
            let py = v * lh as f32 - 0.5;
            let (x0, y0) = (px.floor(), py.floor());
            let (tx, ty) = (px - x0, py - y0);
            let (x0, y0) = (x0 as isize, y0 as isize);
            let at = |xx: isize, yy: isize| {
                let xx = xx.clamp(0, lw as isize - 1) as usize;
                let yy = yy.clamp(0, lh as isize - 1) as usize;
                (yy * lw + xx) * 4
            };
            let (a, b, c, d) = (
                at(x0, y0),
                at(x0 + 1, y0),
                at(x0, y0 + 1),
                at(x0 + 1, y0 + 1),
            );
            if n[a + 3] + n[b + 3] + n[c + 3] + n[d + 3] <= 0.0 {
                continue;
            }
            for ch in 0..4 {
                let top = n[a + ch] + (n[b + ch] - n[a + ch]) * tx;
                let bot = n[c + ch] + (n[d + ch] - n[c + ch]) * tx;
                nf[ch] += top + (bot - top) * ty;
            }
        }
        if nf[3] > 1e-4 {
            // Right at a near object's silhouette only half its disc covers
            // the pixel (coverage ≈ 0.5) while the object side stays opaque;
            // doubling makes the spread start at full coverage there and
            // fade to 0 one CoC out — continuous, no hard outline.
            let cov = (2.0 * nf[3]).min(1.0);
            let col = [nf[0] / nf[3], nf[1] / nf[3], nf[2] / nf[3]];
            let m = col[0].max(col[1]).max(col[2]);
            let f = if m > HL_KNEE { compress(m, k) / m } else { 1.0 };
            for ch in 0..3 {
                p[ch] += (col[ch] * f - p[ch]) * cov;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::BlurField;

    fn optics(field: BlurField, rmax: f32) -> LensOptics {
        LensOptics {
            field,
            rmax,
            s0: 0.1,
            falloff: 0.4,
            highlight_gain: 6.0,
            ..Default::default()
        }
    }

    #[test]
    fn depth_edges_leave_no_unrendered_pixels() {
        // uniform grey scene, near half in focus, far half fully defocused:
        // the output must stay grey everywhere (no black, unrendered pixels)
        let (w, h) = (256, 192);
        let img = ImageF32::filled(w, h, [0.5; 3]);
        let mut o = optics(BlurField::SweetSpot, 0.05);
        o.depth_scale = 1.0;
        let g = Geometry::new(w, h, [0.25, 0.5], 0.0, o.field);
        let mut near = MaskF32::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // jagged edge so CoC jumps inside level footprints
                near.data[y * w + x] = if x + (y % 7) < w / 2 { 0.9 } else { 0.1 };
            }
        }
        let df = DepthField { near, focus: 0.9 };
        let out = render_with_depth(&img, &o, &g, Some(&df));
        let worst = out.data.iter().fold(0.0f32, |m, v| m.max((v - 0.5).abs()));
        assert!(worst < 0.02, "pixel off by {worst}");
    }

    #[test]
    fn near_blur_spreads_softly_over_sharp_background() {
        // bright near disc (defocused) in front of a dark, in-focus background
        let (w, h) = (256, 192);
        let (cx, cy, rad) = (128.0f32, 96.0f32, 40.0f32);
        let mut img = ImageF32::filled(w, h, [0.1; 3]);
        let mut near = MaskF32::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                if d < rad {
                    img.set(x, y, [0.6; 3]);
                    near.data[y * w + x] = 0.9;
                } else {
                    near.data[y * w + x] = 0.1;
                }
            }
        }
        let mut o = optics(BlurField::Uniform, 0.06); // ≈15 px blur
        o.depth_scale = 1.0;
        let g = Geometry::new(w, h, [0.5, 0.5], 0.0, o.field);
        let df = DepthField { near, focus: 0.1 };
        let out = render_with_depth(&img, &o, &g, Some(&df));
        let at = |dx: f32| out.get((cx + rad + dx) as usize, cy as usize)[0];
        let (just, mid, far) = (at(2.0), at(7.0), at(30.0));
        assert!(just > 0.2, "disc colour must spill over the edge: {just}");
        assert!(
            just > mid && mid > far + 0.01,
            "soft falloff: {just} {mid} {far}"
        );
        assert!(
            (far - 0.1).abs() < 0.01,
            "background far away stays sharp: {far}"
        );
    }

    #[test]
    fn ca_fringe_reaches_beyond_one_tile() {
        // coarse-level kernel (8 px) with strong CA: the red fringe reaches
        // 8·1.25+0.5 px, more than the one-tile dilation used to cover
        let (w, h) = (40, 8);
        let mut img = ImageF32::filled(w, h, [0.0; 3]);
        img.set(7, 4, [1.0; 3]);
        let mut coc = MaskF32::new(w, h);
        coc.data[4 * w + 7] = 8.0;
        let lo = MaskF32 {
            w,
            h,
            data: vec![1.0; w * h],
        };
        let kp = KernelParams {
            aperture: Aperture::Circle,
            area: aperture::area(&Aperture::Circle),
            swirl: 0.0,
            cats_eye: 0.0,
            bubble: 0.0,
            onion: 0.0,
            ca_long: 0.25,
        };
        let level = Level {
            factor: 8.0,
            rmax: 8.0,
            lo: 0.0,
            hi: f32::INFINITY,
            img: &img,
            coc: &coc,
            own_min: &lo,
            own_max: &lo,
            near: None,
            near_field: None,
        };
        let g = Geometry::new(w * 8, h * 8, [0.5, 0.5], 0.0, BlurField::Uniform);
        let out = gather(&level, &kp, &g, w * 8, h * 8).img;
        // x = 16 is 9 px from the source (inside the 10 px red disc only),
        // in tile 2 (source in tile 0)
        assert!(
            out.get(16, 4)[0] > 0.0,
            "red fringe cut off at the tile edge"
        );
    }

    #[test]
    fn cascaded_block_range_matches_direct_footprints() {
        // odd sizes: edge blocks are clipped like ImageF32::downsample
        let (w, h) = (53, 37);
        let coc = MaskF32 {
            w,
            h,
            data: (0..w * h).map(|i| ((i * 7919) % 97) as f32).collect(),
        };
        let (mut pmin, mut pmax) = (coc.clone(), coc.clone());
        for factor in [2usize, 4, 8, 16, 32, 64] {
            let (lw, lh) = (w.div_ceil(factor), h.div_ceil(factor));
            let (bmin, bmax) = block_range(&pmin, &pmax, lw, lh);
            for y in 0..lh {
                for x in 0..lw {
                    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                    for fy in y * factor..((y + 1) * factor).min(h) {
                        for fx in x * factor..((x + 1) * factor).min(w) {
                            lo = lo.min(coc.data[fy * w + fx]);
                            hi = hi.max(coc.data[fy * w + fx]);
                        }
                    }
                    assert_eq!((bmin.data[y * lw + x], bmax.data[y * lw + x]), (lo, hi));
                }
            }
            (pmin, pmax) = (bmin, bmax);
        }
    }

    #[test]
    fn uniform_region_stays_uniform() {
        let img = ImageF32::filled(128, 96, [0.3, 0.2, 0.1]);
        let o = optics(BlurField::Uniform, 0.05);
        let g = Geometry::new(128, 96, [0.5, 0.5], 0.0, o.field);
        let out = render(&img, &o, &g);
        for (a, b) in img.data.iter().zip(out.data.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn sweet_spot_center_stays_sharp() {
        let mut img = ImageF32::filled(200, 200, [0.1; 3]);
        for x in 0..200 {
            if x % 2 == 0 {
                for y in 0..200 {
                    img.set(x, y, [0.8; 3]);
                }
            }
        }
        let o = optics(BlurField::SweetSpot, 0.05);
        let g = Geometry::new(200, 200, [0.5, 0.5], 0.0, o.field);
        let out = render(&img, &o, &g);
        assert_eq!(out.get(100, 100), img.get(100, 100));
        // corner is blurred toward the mean
        let c = out.get(5, 5)[0];
        assert!((c - 0.45).abs() < 0.12, "corner {c}");
    }

    #[test]
    fn expand_compress_roundtrip() {
        for i in 0..100 {
            let v = i as f32 / 100.0;
            assert!((compress(expand(v, 0.9), 0.9) - v).abs() < 1e-4);
        }
    }

    #[test]
    fn point_light_becomes_shape() {
        // A single bright point in a uniform-blur lens spreads into a disc.
        let mut img = ImageF32::filled(160, 160, [0.01; 3]);
        img.set(80, 80, [1.0; 3]);
        let o = optics(BlurField::Uniform, 0.06); // ~9.6 px radius
        let g = Geometry::new(160, 160, [0.5, 0.5], 0.0, o.field);
        let out = render(&img, &o, &g);
        let inside = out.get(84, 80)[0];
        let outside = out.get(95, 80)[0];
        assert!(inside > outside * 3.0, "inside {inside} outside {outside}");
    }
}
