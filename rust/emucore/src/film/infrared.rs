//! Infrared film emulation from visible RGB. Phone sensors block IR, so we
//! estimate the near-IR record: vegetation (high NIR reflectance) is detected
//! with the ExG index on chromaticity, skies (low NIR) with a blue-dominance
//! mask and a vertical prior. Then the film's layer remap is applied.

use crate::blur::gaussian_mask;
use crate::color::{hue_rotate, luma, mix3, smoothstep};
use crate::image::{ImageF32, MaskF32};
use crate::preset::{IrSpec, IrVariant};
use rayon::prelude::*;

pub struct IrMasks {
    /// Regional masks (≈512 px wide, well blurred): "is this area foliage/sky?"
    pub veg: MaskF32,
    pub sky: MaskF32,
    /// Fine masks (≈1536 px wide, lightly blurred): the per-pixel decision.
    /// JPEG chroma is half-resolution anyway, so this loses no real detail
    /// while removing the chroma noise that made flat walls speckle.
    pub veg_fine: MaskF32,
    pub sky_fine: MaskF32,
}

/// Foliage likelihood: ExG on chromaticity, gated by brightness and by green
/// clearly above red. Chlorophyll leaves have g ≫ r; olive / khaki paint can
/// reach a leaf-like ExG (low blue) with red ≈ green and must not turn red
/// in Aerochrome. Mirrored in final.frag (`vegIndex`).
#[inline]
pub fn veg_index(c: [f32; 3], lo: f32, hi: f32) -> f32 {
    let s = c[0] + c[1] + c[2] + 1e-6;
    let (r, g, b) = (c[0] / s, c[1] / s, c[2] / s);
    let exg = 2.0 * g - r - b;
    smoothstep(lo, hi, exg) * smoothstep(0.01, 0.06, luma(c)) * smoothstep(0.03, 0.10, g - r)
}

#[inline]
pub fn sky_index(c: [f32; 3], v: f32) -> f32 {
    let s = c[0] + c[1] + c[2] + 1e-6;
    let (r, g, b) = (c[0] / s, c[1] / s, c[2] / s);
    smoothstep(0.04, 0.14, b - r.max(g)) * smoothstep(0.02, 0.1, luma(c)) * smoothstep(0.9, 0.3, v)
}

/// Skin likelihood on linear chromaticity: red-dominant, green in the
/// narrow band human skin occupies across skin tones (r ≈ 0.40–0.68; skin in cool shade sits low,
/// g ≈ 0.20–0.33; pink fingertips sit low), blue at most a little above green (flushed pink fingertips), not too dark. Saturated reds (r > 0.72),
/// greys and greens fall outside. Mirrored in final.frag (`skinIndex`).
#[inline]
pub fn skin_index(c: [f32; 3]) -> f32 {
    let s = c[0] + c[1] + c[2] + 1e-6;
    let (r, g, b) = (c[0] / s, c[1] / s, c[2] / s);
    let band = |x: f32, a: f32, b: f32, cc: f32, d: f32| {
        smoothstep(a, b, x) * (1.0 - smoothstep(cc, d, x))
    };
    band(r, 0.35, 0.41, 0.66, 0.72)
        * band(g, 0.16, 0.20, 0.33, 0.36)
        * smoothstep(-0.14, -0.08, g - b)
        * smoothstep(0.01, 0.04, luma(c))
}

/// Pseudo near-IR record. Most materials reflect NIR roughly like red light;
/// chlorophyll foliage reflects ~4–5× its green (Wood effect); clear sky and
/// water have very little NIR.
#[inline]
pub fn pseudo_ir(c: [f32; 3], veg: f32, sky: f32, ir: &IrSpec) -> f32 {
    // warm materials (brick, skin, wood) reflect more NIR; blue ones less
    let base = (0.75 * c[0] + 0.25 * c[1] + 0.4 * (c[0] - c[2])).max(0.0);
    // skin: melanin absorbs visible light far more than NIR, so skin reflects
    // much more NIR than red (pale peach on Aerochrome, glowing in B&W IR)
    let base = base + ((c[0] * ir.k_skin).max(base) - base) * skin_index(c);
    let foliage = c[1] * ir.k_veg;
    let v = base + (foliage.max(base) - base) * veg;
    v * (1.0 - ir.k_sky * sky)
}

fn index_masks(img: &ImageF32, width: usize, sigma_frac: f32, ir: &IrSpec) -> (MaskF32, MaskF32) {
    let factor = (img.w / width).max(1);
    let small = img.downsample(factor);
    let (w, h) = (small.w, small.h);
    let mut veg = MaskF32::new(w, h);
    let mut sky = MaskF32::new(w, h);
    veg.data
        .par_chunks_exact_mut(w)
        .zip(sky.data.par_chunks_exact_mut(w))
        .enumerate()
        .for_each(|(y, (vr, sr))| {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                let c = small.get(x, y);
                vr[x] = veg_index(c, ir.veg_lo, ir.veg_hi);
                sr[x] = sky_index(c, v);
            }
        });
    let sigma = (w as f32 * sigma_frac).max(0.8);
    gaussian_mask(&mut veg, sigma);
    gaussian_mask(&mut sky, sigma * 2.0);
    (veg, sky)
}

/// Semantic gate for a vegetation-probability sample: below ~15 % the pixel
/// is not a plant whatever its colour (towels, plastic, paint).
#[inline]
pub fn seg_gate(p: f32) -> f32 {
    smoothstep(0.15, 0.5, p)
}

/// Multiply a vegetation mask by [`seg_gate`] of the segmentation map, mapping
/// coordinates proportionally (the two maps differ in size and aspect).
fn gate_by_seg(mask: &mut MaskF32, seg: &MaskF32) {
    let (w, h) = (mask.w, mask.h);
    mask.data
        .par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            let v = (y as f32 + 0.5) / h as f32;
            for (x, m) in row.iter_mut().enumerate() {
                *m *= seg_gate(seg.sample_uv((x as f32 + 0.5) / w as f32, v));
            }
        });
}

/// Build the regional (blurred) and fine (per-pixel decision) masks. With a
/// vegetation-probability map `seg`, both vegetation masks are multiplied by
/// `smoothstep(0.15, 0.5, p_veg)` after blurring, so a non-plant region is
/// zero in the regional mask too and cannot be confirmed by its surroundings.
pub fn masks(img: &ImageF32, ir: &IrSpec, seg: Option<&MaskF32>) -> IrMasks {
    let (mut veg, sky) = index_masks(img, 512, 0.005, ir);
    let (mut veg_fine, sky_fine) = index_masks(img, 1536, 0.0, ir);
    if let Some(s) = seg {
        gate_by_seg(&mut veg, s);
        gate_by_seg(&mut veg_fine, s);
    }
    IrMasks {
        veg,
        sky,
        veg_fine,
        sky_fine,
    }
}

/// Per-pixel IR remap given mask values.
#[inline]
pub fn remap(c: [f32; 3], veg: f32, sky: f32, ir: &IrSpec) -> [f32; 3] {
    let irv = pseudo_ir(c, veg, sky, ir);
    let [r, g, b] = c;
    match ir.variant {
        IrVariant::Aerochrome => [irv, r, g],
        IrVariant::AerochromeOrange => [irv * 1.05, r * 0.95, 0.6 * g + 0.2 * r],
        IrVariant::Mono => [irv; 3],
        IrVariant::Golden => [irv, 0.35 * g + 0.55 * irv, 0.9 * b + 0.1 * r],
        IrVariant::Purple => {
            let rot = hue_rotate(c, 150.0).map(|v| v.max(0.0) * 1.5);
            let base = hue_rotate(c, -12.0).map(|v| v.max(0.0));
            mix3(base, rot, veg)
        }
    }
}

/// Combine a pixel's own (sharp) mask value with the blurred regional one.
/// The regional mask only confirms "this area is foliage/sky" (suppressing
/// isolated noisy pixels); the pixel's own color decides whether it gets the
/// effect. Using the blurred mask alone made foliage IR bleed onto neighbours.
/// Mirrored in final.frag (`gateMask`).
#[inline]
pub fn gate(own: f32, regional: f32) -> f32 {
    own * smoothstep(0.05, 0.35, regional)
}

pub fn apply(img: &mut ImageF32, ir: &IrSpec, seg: Option<&MaskF32>) {
    let m = masks(img, ir, seg);
    let (w, h) = (img.w as f32, img.h as f32);
    img.par_map_pixels(|x, y, p| {
        let u = (x as f32 + 0.5) / w;
        let v = (y as f32 + 0.5) / h;
        let veg = gate(m.veg_fine.sample_uv(u, v), m.veg.sample_uv(u, v));
        let sky = gate(m.sky_fine.sample_uv(u, v), m.sky.sample_uv(u, v));
        let o = remap([p[0], p[1], p[2]], veg, sky, ir);
        p.copy_from_slice(&o);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(v: IrVariant) -> IrSpec {
        IrSpec {
            variant: v,
            k_veg: 4.5,
            k_sky: 0.7,
            veg_lo: 0.07,
            veg_hi: 0.2,
            k_skin: 1.7,
        }
    }

    #[test]
    fn skin_renders_peach_not_yellow_green_in_aerochrome() {
        use crate::color::srgb_to_linear;
        let lin = |c: [f32; 3]| c.map(|v| srgb_to_linear(v / 255.0));
        let s = spec(IrVariant::Aerochrome);
        // palm and shaded finger from a device photo, plus a darker skin tone
        for px in [
            [134.0, 100.0, 87.0],
            [84.0, 60.0, 52.0],
            [92.0, 58.0, 40.0],
            [115.0, 68.0, 63.0],
        ] {
            let c = lin(px);
            assert!(
                skin_index(c) > 0.9,
                "{px:?} not detected as skin: {}",
                skin_index(c)
            );
            let o = remap(c, 0.0, 0.0, &s);
            // IR (→ red) well above the red record (→ green): peach, not yellow-green
            assert!(o[0] > o[1] * 1.5, "{px:?} → {o:?}");
            // k_skin = 0 restores the plain warm-material model
            let flat = IrSpec {
                k_skin: 0.0,
                ..s.clone()
            };
            assert!(remap(c, 0.0, 0.0, &flat)[0] < o[1] * 1.2);
        }
        // skin in cool shade (palm/finger under skylight) and pink fingertips still count
        for px in [
            [130.0, 112.0, 103.0],
            [126.0, 117.0, 109.0],
            [157.0, 104.0, 121.0],
        ] {
            let s = skin_index(lin(px));
            assert!(s > 0.5, "{px:?} shaded skin missed: {s}");
        }
        // greys, warm concrete, foliage, saturated red paint and blue are not skin
        for px in [
            [128.0, 128.0, 128.0],
            [200.0, 195.0, 185.0],
            [60.0, 140.0, 50.0],
            [220.0, 30.0, 25.0],
            [60.0, 90.0, 200.0],
            [130.0, 125.0, 101.0],
        ] {
            assert!(
                skin_index(lin(px)) < 0.05,
                "{px:?} read as skin: {}",
                skin_index(lin(px))
            );
        }
    }

    #[test]
    fn foliage_turns_red_in_aerochrome() {
        let leaf = [0.08, 0.30, 0.05];
        let veg = veg_index(leaf, 0.07, 0.2);
        assert!(veg > 0.9);
        let o = remap(leaf, veg, 0.0, &spec(IrVariant::Aerochrome));
        assert!(o[0] > o[1] * 2.0 && o[0] > o[2], "{o:?}");
    }

    #[test]
    fn foliage_ir_does_not_bleed_onto_neighbours() {
        // grey wall, then a leafy block on the right: the blurred mask is >0 on
        // the wall near the leaves, but the wall must keep its own color
        let s = spec(IrVariant::Aerochrome);
        let (wall, leaf) = ([0.45, 0.45, 0.45], [0.08, 0.30, 0.05]);
        let mut img = ImageF32::filled(1024, 64, wall);
        for y in 0..64 {
            for x in 512..1024 {
                img.set(x, y, leaf);
            }
        }
        let regional = masks(&img, &s, None).veg.sample_uv(508.5 / 1024.0, 0.5);
        assert!(
            regional > 0.1,
            "test needs a blurred mask that reaches the wall ({regional})"
        );
        apply(&mut img, &s, None);
        let near = img.get(508, 32);
        assert!(
            (near[0] - near[1]).abs() < 0.02 && (near[1] - near[2]).abs() < 0.02,
            "wall tinted: {near:?}"
        );
        let inside = img.get(700, 32);
        assert!(inside[0] > inside[1] * 2.0, "leaf lost its IR: {inside:?}");
    }

    #[test]
    fn faint_green_stains_are_not_foliage() {
        let s = spec(IrVariant::Aerochrome);
        // algae-stained concrete (ExG ≈ 0.05) and a shaded leaf (ExG ≈ 0.3)
        let stain = [0.30, 0.325, 0.30];
        let shaded_leaf = [0.04, 0.08, 0.03];
        assert!(veg_index(stain, s.veg_lo, s.veg_hi) < 0.01);
        assert!(veg_index(shaded_leaf, s.veg_lo, s.veg_hi) > 0.9);
        let d = IrSpec {
            veg_lo: default_veg_lo_for_test(),
            veg_hi: 0.2,
            ..s
        };
        assert!(veg_index(stain, d.veg_lo, d.veg_hi) < 0.01);
    }

    #[test]
    fn olive_paint_is_not_foliage() {
        use crate::color::srgb_to_linear;
        let d = default_spec_for_test();
        let lin = |c: [f32; 3]| c.map(|v| srgb_to_linear(v / 255.0));
        // dark olive door frame (device photo): leaf-like ExG, red ≈ green
        for paint in [
            lin([46.0, 47.0, 33.0]),
            lin([20.0, 31.0, 23.0]),
            [0.20, 0.21, 0.08],
        ] {
            let v = veg_index(paint, d.veg_lo, d.veg_hi);
            assert!(v < 0.05, "paint {paint:?} → {v}");
        }
        for leaf in [[0.08, 0.30, 0.05], [0.04, 0.08, 0.03]] {
            let v = veg_index(leaf, d.veg_lo, d.veg_hi);
            assert!(v > 0.9, "leaf {leaf:?} → {v}");
        }
    }

    fn default_spec_for_test() -> IrSpec {
        toml::from_str::<IrSpec>(r#"variant = "aerochrome""#).unwrap()
    }

    fn default_veg_lo_for_test() -> f32 {
        let toml = r#"variant = "aerochrome""#;
        toml::from_str::<IrSpec>(toml).unwrap().veg_lo
    }

    #[test]
    fn sky_darkens_in_mono_ir() {
        let skyc = [0.25, 0.4, 0.8];
        let s = sky_index(skyc, 0.1);
        assert!(s > 0.5, "sky {s}");
        let o = remap(skyc, 0.0, s, &spec(IrVariant::Mono));
        assert!(o[0] < luma(skyc) * 0.7);
    }

    #[test]
    fn foliage_is_bright_in_mono_ir() {
        let leaf = [0.05, 0.12, 0.03];
        let o = remap(leaf, 1.0, 0.0, &spec(IrVariant::Mono));
        assert!(o[0] > luma(leaf) * 3.0, "{o:?}");
    }

    /// Wall (left), green towel (middle) and leaf (right) in one frame.
    fn scene() -> ImageF32 {
        use crate::color::srgb_to_linear;
        let towel = [40.0, 140.0, 60.0].map(|v| srgb_to_linear(v / 255.0));
        let mut img = ImageF32::filled(600, 64, [0.45; 3]);
        for y in 0..64 {
            for x in 200..400 {
                img.set(x, y, towel);
            }
            for x in 400..600 {
                img.set(x, y, [0.08, 0.30, 0.05]);
            }
        }
        img
    }

    fn render_scene(seg: Option<&MaskF32>) -> ImageF32 {
        let mut img = scene();
        apply(&mut img, &spec(IrVariant::Aerochrome), seg);
        img
    }

    fn const_mask(v: f32) -> MaskF32 {
        // different size and aspect from the image on purpose
        let mut m = MaskF32::new(8, 4);
        m.data.fill(v);
        m
    }

    #[test]
    fn towel_is_not_foliage_when_segmentation_says_no() {
        let plain = render_scene(None);
        let no = render_scene(Some(&const_mask(0.0)));
        let yes = render_scene(Some(&const_mask(1.0)));
        let orig = scene();
        // the towel is detected as foliage by colour alone...
        let t = plain.get(300, 32);
        assert!(
            t[0] > t[1] * 1.5,
            "test needs a colour-detected towel {t:?}"
        );
        // ...veg = 1 is bit-identical to no map, veg = 0 keeps the original
        assert_eq!(yes.data, plain.data, "veg=1 must equal the no-veg path");
        assert_eq!(no.get(300, 32), {
            let o = orig.get(300, 32);
            remap(o, 0.0, 0.0, &spec(IrVariant::Aerochrome))
        });
        assert_ne!(no.get(300, 32), plain.get(300, 32));
        let leaf = no.get(500, 32);
        assert!(leaf[0] < leaf[1] * 2.0, "veg=0 should suppress leaf IR too");
    }

    #[test]
    fn segmentation_map_is_sampled_proportionally() {
        // seg says "plant" only on the right half: towel (middle) off, leaf on
        let mut m = MaskF32::new(8, 4);
        for y in 0..4 {
            for x in 4..8 {
                m.data[y * 8 + x] = 1.0;
            }
        }
        let plain = render_scene(None);
        let o = render_scene(Some(&m));
        assert_eq!(o.get(550, 32), plain.get(550, 32), "leaf on the plant side");
        assert_ne!(
            o.get(250, 32),
            plain.get(250, 32),
            "towel off the plant side"
        );
    }
}
