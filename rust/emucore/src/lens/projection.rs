//! Lens projection: remaps the (rectilinear) camera frame into a fisheye
//! image. Equidistant model r = f·θ: an output point at normalized radius ρ
//! (0 centre … 1 frame corner, or rim of the image circle) looks along
//! θ = ρ·θmax, which a rectilinear source shows at radius tan θ / tan θmax.
//! The source can't see past its own field of view, so θmax is a look
//! parameter (curvature), not the true 180° of a real fisheye.
//!
//! Mirrored in `final.frag` (`fisheyeUv`); keep both identical.

use crate::image::{ImageF32, MaskF32};
use crate::preset::{LensOptics, Projection};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct Warp {
    circular: bool,
    theta_max: f32,
    tan_max: f32,
}

impl Warp {
    /// None for an ordinary (rectilinear) lens.
    pub fn new(o: &LensOptics) -> Option<Self> {
        let circular = match o.projection {
            Projection::Rectilinear => return None,
            Projection::Fisheye => false,
            Projection::Circular => true,
        };
        // NaN (e.g. `nan` in a TOML preset) would blank the frame
        let angle = if o.fisheye_angle.is_nan() {
            LensOptics::default().fisheye_angle
        } else {
            o.fisheye_angle
        };
        let theta_max = angle.clamp(5.0, 85.0).to_radians();
        Some(Warp {
            circular,
            theta_max,
            tan_max: theta_max.tan(),
        })
    }

    /// [kind (1 fisheye, 2 circular), θmax, tan θmax] for the shader.
    pub fn uniform(&self) -> [f32; 3] {
        [
            if self.circular { 2.0 } else { 1.0 },
            self.theta_max,
            self.tan_max,
        ]
    }

    /// Output radius unit in px: half-diagonal, or the image circle's radius.
    fn r_out(&self, w: f32, h: f32) -> f32 {
        if self.circular {
            0.5 * w.min(h)
        } else {
            0.5 * (w * w + h * h).sqrt()
        }
    }

    /// Source pixel position for output pixel position (x, y) in a w×h frame,
    /// and the coverage (1 inside the image circle, 0 outside, 1 px AA rim).
    #[inline]
    pub fn source(&self, x: f32, y: f32, w: f32, h: f32) -> ([f32; 2], f32) {
        let r_out = self.r_out(w, h);
        let (cx, cy) = (0.5 * w, 0.5 * h);
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        let rho = (r / r_out).min(1.0);
        let scale = if r > 1e-4 {
            (rho * self.theta_max).tan() / self.tan_max * r_out / r
        } else {
            self.theta_max / self.tan_max
        };
        let cover = if self.circular {
            // AA width scales with resolution so preview and capture agree
            let aa = (r_out / 400.0).max(1.0);
            ((r_out - r) / aa + 0.5).clamp(0.0, 1.0)
        } else {
            1.0
        };
        ([cx + dx * scale, cy + dy * scale], cover)
    }

    pub fn image(&self, src: &ImageF32) -> ImageF32 {
        let (w, h) = (src.w as f32, src.h as f32);
        let mut out = ImageF32::new(src.w, src.h);
        out.par_map_pixels(|x, y, p| {
            let ([sx, sy], cover) = self.source(x as f32 + 0.5, y as f32 + 0.5, w, h);
            if cover <= 0.0 {
                return;
            }
            let c = src.sample_cubic(sx, sy);
            for k in 0..3 {
                p[k] = c[k].max(0.0) * cover;
            }
        });
        out
    }

    /// Black out what lies outside the image circle again after lens stages
    /// that spread light (veil, glow, bokeh): no light reaches there. Same
    /// point as `c *= cover` in `final.frag`. No-op for full-frame fisheye.
    pub fn clip_circle(&self, img: &mut ImageF32) {
        if !self.circular {
            return;
        }
        let (w, h) = (img.w as f32, img.h as f32);
        img.par_map_pixels(|x, y, p| {
            let (_, cover) = self.source(x as f32 + 0.5, y as f32 + 0.5, w, h);
            if cover < 1.0 {
                p.iter_mut().for_each(|v| *v *= cover);
            }
        });
    }

    /// Remap an auxiliary map (depth, segmentation) of a `w`×`h` frame. Aux
    /// maps cover the frame in normalized coordinates at any resolution (the
    /// models see a stretched square), so the warp is evaluated in frame pixels.
    pub fn mask(&self, m: &MaskF32, w: usize, h: usize) -> MaskF32 {
        let (w, h) = (w as f32, h as f32);
        let (kx, ky) = (w / m.w as f32, h / m.h as f32);
        let mut out = MaskF32::new(m.w, m.h);
        if m.w == 0 {
            return out;
        }
        out.data
            .par_chunks_exact_mut(m.w)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    let ([sx, sy], _) =
                        self.source((x as f32 + 0.5) * kx, (y as f32 + 0.5) * ky, w, h);
                    *o = m.sample(sx / kx, sy / ky);
                }
            });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warp(p: Projection, angle: f32) -> Warp {
        let o = LensOptics {
            projection: p,
            fisheye_angle: angle,
            ..Default::default()
        };
        Warp::new(&o).unwrap()
    }

    #[test]
    fn rectilinear_is_identity() {
        assert!(Warp::new(&LensOptics::default()).is_none());
    }

    #[test]
    fn fisheye_keeps_corners_and_centre_and_bulges_the_middle() {
        let wp = warp(Projection::Fisheye, 70.0);
        let (w, h) = (400.0, 300.0);
        let (c, _) = wp.source(200.0, 150.0, w, h);
        assert!((c[0] - 200.0).abs() < 1e-3 && (c[1] - 150.0).abs() < 1e-3);
        let (k, cover) = wp.source(0.0, 0.0, w, h);
        assert!(k[0].abs() < 1e-2 && k[1].abs() < 1e-2 && cover == 1.0);
        // halfway out, the output shows content from nearer the centre (magnified)
        let (m, _) = wp.source(300.0, 150.0, w, h);
        assert!(m[0] > 200.0 && m[0] < 300.0, "{m:?}");
        // straight edge midpoints are pulled in → straight lines bow outward
        let (e, _) = wp.source(400.0, 150.0, w, h);
        assert!(e[0] < 400.0);
    }

    #[test]
    fn circular_blacks_out_beyond_the_image_circle() {
        let wp = warp(Projection::Circular, 80.0);
        let src = ImageF32::filled(64, 48, [0.5, 0.5, 0.5]);
        let out = wp.image(&src);
        assert_eq!(
            out.get(0, 0),
            [0.0, 0.0, 0.0],
            "corner is outside the circle"
        );
        assert!((out.get(32, 24)[0] - 0.5).abs() < 1e-4);
        // the rim of the circle reaches the short-side edge midpoints
        let (s, _) = wp.source(32.0, 0.0, 64.0, 48.0);
        assert!(s[1].abs() < 1e-3);
    }

    /// A square depth map of a 4:3 photo must land where the image does.
    #[test]
    fn mask_follows_the_frame_not_its_own_aspect() {
        let wp = warp(Projection::Fisheye, 70.0);
        let (w, h) = (400usize, 300usize);
        let mut m = MaskF32::new(64, 64);
        // value = normalized x, so the warped map must equal source x / w
        for y in 0..64 {
            for x in 0..64 {
                m.data[y * 64 + x] = (x as f32 + 0.5) / 64.0;
            }
        }
        let out = wp.mask(&m, w, h);
        for (mx, my) in [(10usize, 10usize), (50, 20), (8, 56), (32, 5)] {
            let (px, py) = (
                (mx as f32 + 0.5) * 400.0 / 64.0,
                (my as f32 + 0.5) * 300.0 / 64.0,
            );
            let ([sx, _], _) = wp.source(px, py, w as f32, h as f32);
            let want = (sx / w as f32).clamp(0.5 / 64.0, 63.5 / 64.0);
            let got = out.data[my * 64 + mx];
            assert!((got - want).abs() < 2e-3, "({mx},{my}): {got} vs {want}");
        }
    }

    #[test]
    fn non_finite_angle_still_renders() {
        for a in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let wp = warp(Projection::Fisheye, a);
            let (p, _) = wp.source(10.0, 20.0, 400.0, 300.0);
            assert!(p.iter().all(|v| v.is_finite()), "{a}: {p:?}");
        }
    }

    #[test]
    fn clip_circle_blacks_the_border_only() {
        let mut img = ImageF32::filled(64, 48, [0.2, 0.2, 0.2]);
        warp(Projection::Circular, 64.0).clip_circle(&mut img);
        assert_eq!(img.get(0, 0), [0.0; 3]);
        assert_eq!(img.get(32, 24), [0.2; 3]);
        let mut full = ImageF32::filled(64, 48, [0.2, 0.2, 0.2]);
        warp(Projection::Fisheye, 64.0).clip_circle(&mut full);
        assert_eq!(full.get(0, 0), [0.2; 3]);
    }
}
