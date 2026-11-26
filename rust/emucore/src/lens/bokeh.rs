//! Bokeh rendering. Scatter-as-gather: every output pixel sums contributions
//! of neighbours whose (position-dependent) kernel covers it, so each source
//! pixel spreads light in *its own* aperture shape — this is what makes swirl,
//! cat's-eye and creative shapes work without depth.
//!
//! Multi-level for speed: small radii are gathered at full resolution, large
//! radii on 1/4 and 1/16 downsampled copies (≤16 px kernel radius per level),
//! then levels are cross-faded by each pixel's own CoC.

use super::{aperture, coc, Geometry};
use crate::color::smoothstep;
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
        let u = if al > 1e-3 { [ax / al, ay / al] } else { [1.0, 0.0] };
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
    let sg = SrcGeo { e, u, inv_r: 1.0, inv_t: 1.0 };
    let sg = if k.swirl > 0.0 {
        let s = k.swirl * e.powf(1.7);
        SrcGeo { inv_r: 1.0 / (1.0 - 0.3 * s).max(0.35), inv_t: 1.0 / (1.0 + s), ..sg }
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

struct Level {
    factor: usize,
    /// Max kernel radius in level pixels.
    rmax: f32,
    /// Only pixels whose own full-res CoC lies in [lo, hi] need output here.
    lo: f32,
    hi: f32,
    img: ImageF32,
    /// CoC in level px, clamped to rmax (used for kernels).
    coc: MaskF32,
    /// Unclamped CoC in full-res px (used to decide which pixels need output).
    own: MaskF32,
}

/// Gather one level. CoC in level pixels, clamped to `rmax`.
fn gather(level: &Level, kp: &KernelParams, g: &Geometry, full_w: usize, full_h: usize) -> ImageF32 {
    let (w, h) = (level.img.w, level.img.h);

    let has_ca = kp.ca_long > 0.0;
    let sx = full_w as f32 / w as f32;
    let sy = full_h as f32 / h as f32;
    let simple = !has_ca && kp.swirl <= 0.0 && kp.cats_eye <= 0.0 && kp.bubble == 0.0 && kp.onion <= 0.0;

    // Precompute per-source geometry.
    let geo: Vec<SrcGeo> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            SrcGeo::new(kp, g, (x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy)
        })
        .collect();

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
    // dilate by one tile (rmax ≤ T)
    let mut treach = vec![0.0f32; tw * th];
    for ty in 0..th {
        for tx in 0..tw {
            let mut m = 0.0f32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (tx as i32 + dx, ty as i32 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < tw && (ny as usize) < th {
                        m = m.max(tmax[ny as usize * tw + nx as usize]);
                    }
                }
            }
            treach[ty * tw + tx] = m * (1.0 + kp.ca_long) + 0.5;
        }
    }

    let mut out = ImageF32::new(w, h);
    out.data.par_chunks_exact_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let own = level.own.data[y * w + x];
            if own < level.lo || own > level.hi {
                continue;
            }
            let reach_f = treach[(y / T) * tw + x / T].min(level.rmax * (1.0 + kp.ca_long) + 0.5);
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
                    let c = &level.img.data[si * 3..si * 3 + 3];
                    let center = dx == 0 && dy == 0;
                    // offset from source to destination (scatter direction)
                    let o = [-(dx as f32), -(dy as f32)];
                    if has_ca {
                        let rs = [rr * (1.0 + kp.ca_long), rr, rr * (1.0 - kp.ca_long)];
                        for ch in 0..3 {
                            let inv = 1.0 / rs[ch];
                            let norm = 1.0 / (rs[ch] * rs[ch] * kp.area).max(1.0);
                            let wgt = if center && rs[ch] <= 0.75 { norm } else { kernel_g(kp, [o[0] * inv, o[1] * inv], &geo[si]) * norm };
                            acc[ch] += c[ch] * wgt;
                            wsum[ch] += wgt;
                        }
                    } else {
                        let inv = 1.0 / rr;
                        let norm = 1.0 / (rr * rr * kp.area).max(1.0);
                        let wgt = if center && rr <= 0.75 {
                            1.0
                        } else if simple {
                            aperture::weight(&kp.aperture, [o[0] * inv, o[1] * inv]) * norm
                        } else {
                            kernel_g(kp, [o[0] * inv, o[1] * inv], &geo[si]) * norm
                        };
                        if wgt > 0.0 {
                            acc[0] += c[0] * wgt;
                            acc[1] += c[1] * wgt;
                            acc[2] += c[2] * wgt;
                            wsum[0] += wgt;
                            wsum[1] += wgt;
                            wsum[2] += wgt;
                        }
                    }
                }
            }
            let i = y * w + x;
            for ch in 0..3 {
                row[x * 3 + ch] = if wsum[ch] > 1e-8 { acc[ch] / wsum[ch] } else { level.img.data[i * 3 + ch] };
            }
        }
    });
    out
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

/// Render lens blur. Returns the input unchanged when the lens has no blur.
pub fn render(img: &ImageF32, optics: &LensOptics, g: &Geometry) -> ImageF32 {
    let rmax_px = optics.rmax * img.w as f32;
    if rmax_px < 0.6 || optics.field == crate::preset::BlurField::None {
        return img.clone();
    }
    let kp = KernelParams::from_optics(optics);
    let k = highlight_k(optics.highlight_gain);
    let (w, h) = (img.w, img.h);

    // Full-res CoC map.
    let mut coc_full = MaskF32::new(w, h);
    coc_full.data.par_chunks_exact_mut(w).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = coc(optics, g, x as f32 + 0.5, y as f32 + 0.5);
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
    let levels: Vec<(Level, ImageF32)> = specs
        .iter()
        .enumerate()
        .map(|(i, &factor)| {
            let limg = hdr.downsample(factor);
            let mut lcoc = MaskF32::new(limg.w, limg.h);
            let mut lown = MaskF32::new(limg.w, limg.h);
            let sx = w as f32 / limg.w as f32;
            let sy = h as f32 / limg.h as f32;
            lcoc.data
                .par_chunks_exact_mut(limg.w)
                .zip(lown.data.par_chunks_exact_mut(limg.w))
                .enumerate()
                .for_each(|(y, (row, orow))| {
                    for x in 0..row.len() {
                        let r = coc(optics, g, (x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
                        orow[x] = r;
                        row[x] = (r / factor as f32).min(LRMAX[i]);
                    }
                });
            let f = factor as f32;
            let lo = if i == 0 { 0.5 } else { 0.70 * LRMAX[i - 1] * f / 2.0 };
            let hi = if i + 1 == specs.len() { f32::INFINITY } else { LRMAX[i] * f + 1.0 };
            let level = Level { factor, rmax: LRMAX[i], lo, hi, img: limg, coc: lcoc, own: lown };
            let t0 = std::time::Instant::now();
            let out = gather(&level, &kp, g, w, h);
            if std::env::var_os("EMUCAM_PROF").is_some() {
                let n = level.own.data.iter().filter(|o| **o >= level.lo && **o <= level.hi).count();
                eprintln!("  bokeh level x{factor} {}x{}: {:.2?}  active px {n} lo {:.1} hi {:.1}", level.img.w, level.img.h, t0.elapsed(), level.lo, level.hi);
            }
            (level, out)
        })
        .collect();

    // Composite per pixel by its own CoC.
    let mut out = img.clone();
    out.par_map_pixels(|x, y, p| {
        let r = coc_full.data[y * w + x];
        if r < 0.5 {
            return;
        }
        // Cross-fade levels: level i covers r ≤ rmax·factor_i (full-res px).
        let mut col = [0.0f32; 3];
        let mut remaining = 1.0f32;
        for (i, (lvl, lout)) in levels.iter().enumerate() {
            let cover = lvl.rmax * lvl.factor as f32;
            let is_last = i + 1 == levels.len();
            let wgt = if is_last { 1.0 } else { 1.0 - smoothstep(cover * 0.75, cover, r) };
            let take = remaining * wgt;
            if take > 0.0 {
                let c = if lvl.factor == 1 {
                    let j = (y * w + x) * 3;
                    [lout.data[j], lout.data[j + 1], lout.data[j + 2]]
                } else {
                    lout.sample_uv((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
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
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::BlurField;

    fn optics(field: BlurField, rmax: f32) -> LensOptics {
        LensOptics { field, rmax, s0: 0.1, falloff: 0.4, highlight_gain: 6.0, ..Default::default() }
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
