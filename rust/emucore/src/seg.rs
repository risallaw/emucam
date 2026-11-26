//! Semantic segmentation maps. The app runs a segmentation network (PIDNet-S
//! Cityscapes) on the photo and hands its raw logits here. Infrared films use
//! the *vegetation* class to tell real foliage from green towels, plastic and
//! paint, which colour alone cannot separate. The network input is built with
//! [`crate::depth::model_input`] on a square layout.

use crate::image::MaskF32;

/// Cityscapes class index of vegetation. Terrain (class 9) is deliberately
/// not used: it pulls bare soil into the foliage mask.
pub const VEG_CLASS: usize = 8;

/// Raw logits (`classes` planes of `w`×`h`, NCHW: `raw[c*w*h + y*w + x]`) →
/// per-pixel probability of [`VEG_CLASS`] as a `w`×`h` mask (0..1), via a
/// numerically stable softmax over all classes. Returns None for too few
/// classes, empty or overflowing sizes, a length mismatch, or any non-finite
/// value, so callers fall back to colour-only detection.
pub fn vegetation(raw: &[f32], classes: usize, w: usize, h: usize) -> Option<MaskF32> {
    if classes <= VEG_CLASS || w == 0 || h == 0 {
        return None;
    }
    let plane = w.checked_mul(h)?;
    if plane.checked_mul(classes) != Some(raw.len()) || raw.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let mut m = MaskF32::new(w, h);
    for (i, o) in m.data.iter_mut().enumerate() {
        let max = (0..classes)
            .map(|c| raw[c * plane + i])
            .fold(f32::NEG_INFINITY, f32::max);
        let mut sum = 0.0f32;
        for c in 0..classes {
            sum += (raw[c * plane + i] - max).exp();
        }
        *o = (raw[VEG_CLASS * plane + i] - max).exp() / sum;
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 9 classes on a 2x1 image; pixel 0 votes veg = 0 vs others 0 (p = 1/9),
    /// pixel 1 has veg logit ln(8) vs eight zeros (p = 0.5).
    fn tiny() -> Vec<f32> {
        let mut raw = vec![0.0f32; 9 * 2];
        raw[VEG_CLASS * 2 + 1] = 8.0f32.ln();
        raw
    }

    #[test]
    fn softmax_matches_hand_value() {
        let m = vegetation(&tiny(), 9, 2, 1).unwrap();
        assert!((m.data[0] - 1.0 / 9.0).abs() < 1e-6, "{:?}", m.data);
        assert!((m.data[1] - 0.5).abs() < 1e-6, "{:?}", m.data);
    }

    #[test]
    fn softmax_is_stable_for_huge_logits() {
        let mut raw = vec![0.0f32; 9];
        raw[VEG_CLASS] = 1000.0;
        raw[0] = 1000.0;
        let m = vegetation(&raw, 9, 1, 1).unwrap();
        assert!((m.data[0] - 0.5).abs() < 1e-6, "{:?}", m.data);
    }

    #[test]
    fn rejects_bad_input() {
        let raw = tiny();
        assert!(vegetation(&raw[..17], 9, 2, 1).is_none(), "length");
        let mut nan = raw.clone();
        nan[3] = f32::NAN;
        assert!(vegetation(&nan, 9, 2, 1).is_none(), "nan");
        let mut inf = raw.clone();
        inf[3] = f32::INFINITY;
        assert!(vegetation(&inf, 9, 2, 1).is_none(), "inf");
        assert!(vegetation(&raw[..16], 8, 2, 1).is_none(), "classes <= 8");
        assert!(vegetation(&raw, 9, 0, 1).is_none(), "zero size");
        let big = 1usize << (usize::BITS / 2);
        assert!(vegetation(&[], 9, big, big).is_none(), "overflow");
        assert!(
            vegetation(&[], 19, 1 << 40, 1 << 30).is_none(),
            "overflow 2"
        );
    }
}
