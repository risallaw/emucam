//! Color helpers: sRGB transfer, luma, HSV-ish hue rotation, small math.

#[inline]
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Rec.709 luma on linear values.
#[inline]
pub fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e0 == e1 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
pub fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
}

pub type Mat3 = [[f32; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[inline]
pub fn mat_mul(m: &Mat3, c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}

/// Saturation adjust around luma (works on linear or display values).
#[inline]
pub fn saturate(c: [f32; 3], s: f32) -> [f32; 3] {
    let y = luma(c);
    [y + (c[0] - y) * s, y + (c[1] - y) * s, y + (c[2] - y) * s]
}

/// Rotate hue around the gray axis by `deg` degrees (Rodrigues rotation in RGB).
pub fn hue_rotate(c: [f32; 3], deg: f32) -> [f32; 3] {
    let a = deg.to_radians();
    let (s, co) = a.sin_cos();
    let k = 1.0 / 3.0f32.sqrt();
    let one_c = 1.0 - co;
    // rotation matrix around (k,k,k)
    let m = [
        [co + k * k * one_c, k * k * one_c - k * s, k * k * one_c + k * s],
        [k * k * one_c + k * s, co + k * k * one_c, k * k * one_c - k * s],
        [k * k * one_c - k * s, k * k * one_c + k * s, co + k * k * one_c],
    ];
    mat_mul(&m, c)
}

/// Approximate white-balance shift as RGB multipliers for a color temperature
/// offset relative to 6500K (e.g. tungsten-balanced film under daylight).
pub fn kelvin_gains(kelvin: f32) -> [f32; 3] {
    // Tanner Helland approximation of blackbody RGB, normalized at 6500K.
    fn bb(k: f32) -> [f32; 3] {
        let t = k / 100.0;
        let r = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
        let g = if t <= 66.0 {
            99.470_8 * t.ln() - 161.119_57
        } else {
            288.122_17 * (t - 60.0).powf(-0.075_514_85)
        };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.517_73 * (t - 10.0).ln() - 305.044_8
        };
        [
            srgb_to_linear((r / 255.0).clamp(0.0, 1.0)),
            srgb_to_linear((g / 255.0).clamp(0.0, 1.0)),
            srgb_to_linear((b / 255.0).clamp(0.0, 1.0)),
        ]
    }
    let w = bb(6500.0);
    let c = bb(kelvin);
    let g = [c[0] / w[0], c[1] / w[1], c[2] / w[2]];
    let y = luma(g).max(1e-4);
    [g[0] / y, g[1] / y, g[2] / y]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=255 {
            let v = i as f32 / 255.0;
            let r = linear_to_srgb(srgb_to_linear(v));
            assert!((r - v).abs() < 1e-5, "{v} -> {r}");
        }
    }

    #[test]
    fn hue_rotate_preserves_gray() {
        let g = hue_rotate([0.4, 0.4, 0.4], 123.0);
        for c in g {
            assert!((c - 0.4).abs() < 1e-5);
        }
    }

    #[test]
    fn kelvin_neutral_at_6500() {
        let g = kelvin_gains(6500.0);
        for c in g {
            assert!((c - 1.0).abs() < 1e-3, "{g:?}");
        }
    }
}

/// Apply hue-selective bands in an opponent space (mean, a, b) where the hue
/// angle is 0° red, 120° green, 240° blue. Neutral colors pass unchanged.
pub fn hue_bands(c: [f32; 3], bands: &[crate::preset::HueBand]) -> [f32; 3] {
    if bands.is_empty() {
        return c;
    }
    const S3: f32 = 0.866_025_4; // sqrt(3)/2
    let m = (c[0] + c[1] + c[2]) / 3.0;
    let a = c[0] - 0.5 * (c[1] + c[2]);
    let b = S3 * (c[1] - c[2]);
    let chroma = (a * a + b * b).sqrt();
    if chroma < 1e-5 {
        return c;
    }
    let hue = b.atan2(a).to_degrees();
    let neutral_guard = smoothstep(0.0, 0.15, chroma);
    let (mut rot, mut sat, mut lum) = (0.0f32, 1.0f32, 0.0f32);
    for band in bands {
        let mut d = (hue - band.hue).rem_euclid(360.0);
        if d > 180.0 {
            d -= 360.0;
        }
        let wdt = band.width.max(1.0);
        if d.abs() >= wdt {
            continue;
        }
        let w = 0.5 * (1.0 + (std::f32::consts::PI * d / wdt).cos()) * neutral_guard;
        rot += band.shift * w;
        sat *= 1.0 + (band.sat - 1.0) * w;
        lum += band.lum * w;
    }
    let (s, co) = rot.to_radians().sin_cos();
    let sat = sat.max(0.0);
    let (a2, b2) = ((a * co - b * s) * sat, (a * s + b * co) * sat);
    let k = 1.0 + lum;
    let m2 = m * k;
    let (a2, b2) = (a2 * k, b2 * k);
    [
        m2 + a2 * (2.0 / 3.0),
        m2 - a2 / 3.0 + b2 / (2.0 * S3) ,
        m2 - a2 / 3.0 - b2 / (2.0 * S3),
    ]
}
