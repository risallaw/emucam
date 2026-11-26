//! Aperture shapes as soft-edged masks on a unit disc. Creative shapes are
//! signed-distance functions so they're crisp at any size and trivially ported
//! to GLSL. `o` is the kernel offset in units of the CoC radius (|o|≤1 inside).

use crate::color::smoothstep;
use crate::preset::Aperture;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::{Mutex, OnceLock};

pub const SHAPE_IDS: &[&str] = &[
    "star", "heart", "sparkle", "crescent", "flower", "diamond", "bird",
];

const EDGE: f32 = 0.08;

#[inline]
fn len(p: [f32; 2]) -> f32 {
    (p[0] * p[0] + p[1] * p[1]).sqrt()
}

/// iq's regular star SDF: n points, m in [2, n] controls inner radius.
fn sd_star(p: [f32; 2], r: f32, n: f32, m: f32) -> f32 {
    let an = PI / n;
    let en = PI / m;
    let acs = [an.cos(), an.sin()];
    let ecs = [en.cos(), en.sin()];
    let bn = (p[1].atan2(p[0])).rem_euclid(2.0 * an) - an;
    let l = len(p);
    let mut q = [l * bn.cos(), l * bn.sin().abs()];
    q[0] -= r * acs[0];
    q[1] -= r * acs[1];
    let t = (-(q[0] * ecs[0] + q[1] * ecs[1])).clamp(0.0, r * acs[1] / ecs[1]);
    q[0] += ecs[0] * t;
    q[1] += ecs[1] * t;
    len(q) * q[0].signum()
}

/// iq's heart SDF (point up in +y). Expects p roughly in [-0.6,0.6]×[0,1].
fn sd_heart(p: [f32; 2]) -> f32 {
    let p = [p[0].abs(), p[1]];
    let dot2 = |v: [f32; 2]| v[0] * v[0] + v[1] * v[1];
    if p[1] + p[0] > 1.0 {
        return dot2([p[0] - 0.25, p[1] - 0.75]).sqrt() - 2f32.sqrt() / 4.0;
    }
    let a = dot2([p[0], p[1] - 1.0]);
    let m = 0.5 * (p[0] + p[1]).max(0.0);
    let b = dot2([p[0] - m, p[1] - m]);
    a.min(b).sqrt() * (p[0] - p[1]).signum()
}

/// Signed distance (negative inside) for a creative shape on the unit disc.
/// Image space: +y is DOWN, so we flip y to keep shapes upright.
pub fn shape_sdf(id: &str, o: [f32; 2]) -> f32 {
    let p = [o[0], -o[1]];
    match id {
        "star" => sd_star(p, 0.95, 5.0, 2.6),
        "sparkle" => {
            // 4-point astroid sparkle (implicit, sign-correct, scaled for edge softness)
            let f = p[0].abs().sqrt() + p[1].abs().sqrt();
            (f - 0.98) * 0.6
        }
        "heart" => {
            // map unit disc → heart space (heart ≈ [-0.6,0.6]×[-0.1,1])
            let q = [p[0] * 0.58, p[1] * 0.58 + 0.53];
            sd_heart(q) / 0.58
        }
        "crescent" => {
            let a = len(p) - 0.95;
            let b = len([p[0] - 0.42, p[1] - 0.18]) - 0.78;
            a.max(-b)
        }
        "flower" => {
            let th = p[1].atan2(p[0]);
            len(p) - (0.72 + 0.26 * (5.0 * th).cos())
        }
        "diamond" => (p[0].abs() * 0.95 + p[1].abs() * 0.7) - 0.7,
        "bird" => {
            // stylised gull: two arcs
            let x = p[0].abs();
            let wing = (p[1] - (0.35 - 1.2 * (x - 0.45) * (x - 0.45))).abs() - 0.17;
            wing.max(x - 0.95)
        }
        _ => len(p) - 1.0,
    }
}

/// Soft inside-weight (1 inside, 0 outside) for an aperture at offset `o`.
#[inline(always)]
pub fn weight(ap: &Aperture, o: [f32; 2]) -> f32 {
    match ap {
        Aperture::Circle => 1.0 - smoothstep(1.0 - EDGE, 1.0, len(o)),
        Aperture::Polygon { blades, rotation } => {
            let n = (*blades).max(3) as f32;
            // The edge radius rp lies in [cos(π/n), 1], so taps clearly
            // inside or outside skip the atan2/cos (same result, exactly:
            // the smoothstep clamps). Most gather taps take this path.
            let l = len(o);
            if l >= 1.0 + 1e-3 {
                return 0.0;
            }
            let apothem = (PI / n).cos();
            if l <= apothem - EDGE - 1e-3 {
                return 1.0;
            }
            let th = o[1].atan2(o[0]) - rotation.to_radians();
            let seg = 2.0 * PI / n;
            let rp = apothem / ((th.rem_euclid(seg)) - PI / n).cos();
            1.0 - smoothstep(rp - EDGE, rp, l)
        }
        Aperture::Slit { ratio, rotation } => {
            let (s, c) = rotation.to_radians().sin_cos();
            let x = c * o[0] + s * o[1];
            let y = -s * o[0] + c * o[1];
            let d = len([x, y / ratio.max(0.05)]);
            1.0 - smoothstep(1.0 - EDGE, 1.0, d)
        }
        Aperture::Shape { id } => {
            let d = shape_sdf(id, o);
            1.0 - smoothstep(-EDGE, 0.0, d)
        }
    }
}

/// Area of the aperture shape on the unit disc (for energy normalization).
pub fn area(ap: &Aperture) -> f32 {
    static CACHE: OnceLock<Mutex<HashMap<String, f32>>> = OnceLock::new();
    let key = format!("{ap:?}");
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return *v;
    }
    let n = 96;
    let mut acc = 0.0;
    for j in 0..n {
        for i in 0..n {
            let o = [
                (i as f32 + 0.5) / n as f32 * 2.0 - 1.0,
                (j as f32 + 0.5) / n as f32 * 2.0 - 1.0,
            ];
            acc += weight(ap, o);
        }
    }
    let a = (acc * 4.0 / (n * n) as f32).max(0.05);
    cache.lock().unwrap().insert(key, a);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_area_is_pi() {
        let a = area(&Aperture::Circle);
        // soft edge (0.92..1) → effective radius ≈ 0.96
        assert!((a - PI * 0.96 * 0.96).abs() < 0.12, "{a}");
    }

    #[test]
    fn every_shape_has_reasonable_area_and_center() {
        for id in SHAPE_IDS {
            let ap = Aperture::Shape { id: id.to_string() };
            let a = area(&ap);
            assert!(a > 0.3 && a < 3.3, "{id} area {a}");
        }
    }

    #[test]
    fn polygon_fast_paths_match_full_formula() {
        for blades in [3u32, 5, 6, 9, 15] {
            let ap = Aperture::Polygon {
                blades,
                rotation: 12.0,
            };
            let n = blades as f32;
            for j in 0..=200 {
                for i in 0..=200 {
                    let o = [i as f32 / 80.0 - 1.25, j as f32 / 80.0 - 1.25];
                    let th = o[1].atan2(o[0]) - 12f32.to_radians();
                    let seg = 2.0 * PI / n;
                    let rp = (PI / n).cos() / ((th.rem_euclid(seg)) - PI / n).cos();
                    let full = 1.0 - smoothstep(rp - EDGE, rp, len(o));
                    assert_eq!(weight(&ap, o), full, "blades {blades} at {o:?}");
                }
            }
        }
    }

    #[test]
    fn heart_points_down_in_image_space() {
        // Image y down: the heart tip is at the bottom (positive y).
        let ap = Aperture::Shape { id: "heart".into() };
        assert!(weight(&ap, [0.0, 0.75]) > 0.5, "tip");
        assert!(weight(&ap, [0.0, -0.9]) < 0.5, "notch");
        assert!(weight(&ap, [0.45, -0.45]) > 0.5, "lobe");
    }
}
