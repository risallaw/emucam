//! Lens emulation: circle-of-confusion field (sweet spot / slice / uniform),
//! position-dependent bokeh kernel (aperture shape, swirl, cat's-eye, soap
//! bubble, onion rings, longitudinal CA), lateral CA, glow, veil, vignette.

pub mod aperture;
pub mod bokeh;
pub mod projection;

use crate::blur;
use crate::color::{luma, smoothstep};
use crate::depth::{self, DepthField};
use crate::image::ImageF32;
use crate::preset::{BlurField, LensOptics};

/// Geometry shared by CoC and kernel evaluation.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub w: f32,
    pub h: f32,
    pub half_diag: f32,
    /// Focus center (px).
    pub center: [f32; 2],
    /// Optical axis for swirl / cat's eye (px).
    pub axis: [f32; 2],
    pub angle: f32,
}

impl Geometry {
    pub fn new(
        w: usize,
        h: usize,
        focus_center: [f32; 2],
        angle_deg: f32,
        field: BlurField,
    ) -> Self {
        let (w, h) = (w as f32, h as f32);
        let center = [focus_center[0] * w, focus_center[1] * h];
        let axis = if field == BlurField::SweetSpot {
            center
        } else {
            [w * 0.5, h * 0.5]
        };
        Geometry {
            w,
            h,
            half_diag: 0.5 * (w * w + h * h).sqrt(),
            center,
            axis,
            angle: angle_deg.to_radians(),
        }
    }
}

/// Blur radius in px at pixel position (x, y) (pixel centers).
#[inline]
pub fn coc(o: &LensOptics, g: &Geometry, x: f32, y: f32) -> f32 {
    let rmax = o.rmax * g.w;
    if rmax <= 0.0 {
        return 0.0;
    }
    match o.field {
        BlurField::None => 0.0,
        BlurField::Uniform => rmax,
        BlurField::SweetSpot => {
            let dx = x - g.center[0];
            let dy = y - g.center[1];
            let d = (dx * dx + dy * dy).sqrt() / g.half_diag;
            rmax * smoothstep(o.s0, o.s0 + o.falloff, d)
        }
        BlurField::Slice => {
            let (s, c) = g.angle.sin_cos();
            let n = [-s, c];
            let d = ((x - g.center[0]) * n[0] + (y - g.center[1]) * n[1]).abs() / g.half_diag;
            rmax * smoothstep(o.s0, o.s0 + o.falloff, d)
        }
    }
}

/// Per-pixel blur radius from the lens field and (optionally) a depth map.
pub struct Defocus<'a> {
    pub optics: &'a LensOptics,
    pub g: &'a Geometry,
    pub depth: Option<&'a DepthField>,
}

impl Defocus<'_> {
    /// Blur radius in full-res px at pixel position (x, y).
    #[inline]
    pub fn at(&self, x: f32, y: f32) -> f32 {
        let o = self.optics;
        let Some(d) = self.depth else {
            return coc(o, self.g, x, y);
        };
        let rmax = o.rmax * self.g.w;
        if rmax <= 0.0 || o.field == BlurField::None {
            return 0.0;
        }
        let dz = depth::defocus(d.near.sample(x, y), d.focus, o.depth_scale);
        if o.field_optical {
            // optical field blur and focus blur add like independent spreads
            let f = coc(o, self.g, x, y) / rmax;
            rmax * (f * f + dz * dz).sqrt().min(1.0)
        } else {
            rmax * dz
        }
    }

    /// Nearness at (x, y) when a depth map is present.
    #[inline]
    pub fn near(&self, x: f32, y: f32) -> Option<f32> {
        self.depth.map(|d| d.near.sample(x, y))
    }
}

/// Lateral chromatic aberration: radial scale of R and B around the axis.
pub fn lateral_ca(img: &ImageF32, k: f32, g: &Geometry) -> ImageF32 {
    if k.abs() < 1e-5 {
        return img.clone();
    }
    let mut out = img.clone();
    out.par_map_pixels(|x, y, p| {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        let dx = px - g.axis[0];
        let dy = py - g.axis[1];
        let rho2 = (dx * dx + dy * dy) / (g.half_diag * g.half_diag);
        let sr = 1.0 + k * rho2;
        let sb = 1.0 - k * rho2;
        p[0] = img.sample(g.axis[0] + dx * sr, g.axis[1] + dy * sr)[0];
        p[2] = img.sample(g.axis[0] + dx * sb, g.axis[1] + dy * sb)[2];
    });
    out
}

/// Spherical-aberration glow (Velvet / soft focus): soft halo + highlight bloom.
pub fn glow(img: &mut ImageF32, strength: f32, radius_frac: f32) {
    if strength <= 0.0 {
        return;
    }
    let g = strength.clamp(0.0, 1.0);
    let wide = blur::gaussian(img, radius_frac * img.w as f32);
    let tight = blur::gaussian(img, radius_frac * 0.35 * img.w as f32);
    img.par_map_pixels(|x, y, p| {
        let i = (y * wide.w + x) * 3;
        let wc = [wide.data[i], wide.data[i + 1], wide.data[i + 2]];
        let tc = [tight.data[i], tight.data[i + 1], tight.data[i + 2]];
        let bloom = smoothstep(0.35, 1.0, luma(wc));
        for c in 0..3 {
            let soft = 0.6 * tc[c] + 0.4 * wc[c];
            p[c] = p[c] * (1.0 - 0.55 * g) + soft * 0.55 * g + wc[c] * bloom * g * 0.6;
        }
    });
}

/// Veiling flare (uncoated / plastic lenses): lifts shadows with average light.
pub fn veil(img: &mut ImageF32, strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let small = img.downsample((img.w / 64).max(1));
    let n = (small.w * small.h) as f32;
    let mut avg = [0.0f32; 3];
    for p in small.data.chunks_exact(3) {
        avg[0] += p[0] / n;
        avg[1] += p[1] / n;
        avg[2] += p[2] / n;
    }
    let v = strength.clamp(0.0, 1.0);
    img.par_map_pixels(|_, _, p| {
        for c in 0..3 {
            p[c] = p[c] * (1.0 - 0.5 * v) + avg[c] * v * 0.6;
        }
    });
}

/// Natural + mechanical vignetting (applied in linear light).
#[inline]
pub fn vignette_factor(strength: f32, g: &Geometry, x: f32, y: f32) -> f32 {
    if strength <= 0.0 {
        return 1.0;
    }
    let dx = (x - g.w * 0.5) / g.half_diag;
    let dy = (y - g.h * 0.5) / g.half_diag;
    let r2 = dx * dx + dy * dy;
    let f = 1.0 / (1.0 + 1.6 * strength * r2);
    f * f
}

pub fn vignette(img: &mut ImageF32, strength: f32, g: &Geometry) {
    if strength <= 0.0 {
        return;
    }
    img.par_map_pixels(|x, y, p| {
        let f = vignette_factor(strength, g, x as f32 + 0.5, y as f32 + 0.5);
        p[0] *= f;
        p[1] *= f;
        p[2] *= f;
    });
}
