//! JPEG/PNG decode → linear f32, and display f32 → JPEG encode.

use crate::color::{linear_to_srgb, srgb_to_linear};
use crate::image::ImageF32;
use crate::noise::{pcg3d, unit};
use crate::Error;
use image::{DynamicImage, ImageDecoder, ImageReader};
use rayon::prelude::*;
use std::io::Cursor;
use std::sync::OnceLock;

fn srgb_table() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0.0; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = srgb_to_linear(i as f32 / 255.0);
        }
        t
    })
}

/// Decode an encoded image (JPEG/PNG), apply EXIF orientation, return linear RGB.
pub fn decode(bytes: &[u8]) -> Result<ImageF32, Error> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Io(e.to_string()))?;
    let mut dec = reader
        .into_decoder()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let orient = dec
        .orientation()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let mut img = DynamicImage::from_decoder(dec).map_err(|e| Error::Decode(e.to_string()))?;
    img.apply_orientation(orient);
    Ok(from_rgb8(&img.to_rgb8()))
}

pub fn from_rgb8(rgb: &image::RgbImage) -> ImageF32 {
    let t = srgb_table();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let raw = rgb.as_raw();
    let mut out = ImageF32::new(w, h);
    out.data
        .par_iter_mut()
        .zip(raw.par_iter())
        .for_each(|(o, b)| *o = t[*b as usize]);
    out
}

pub fn linear_from_display(img: &ImageF32) -> ImageF32 {
    let mut o = img.clone();
    o.data
        .par_iter_mut()
        .for_each(|v| *v = srgb_to_linear(v.clamp(0.0, 1.0)));
    o
}

/// Display (sRGB-encoded 0..1) float → 8-bit with blue-ish hash dither.
pub fn to_rgb8(img: &ImageF32) -> image::RgbImage {
    let w = img.w;
    let mut buf = vec![0u8; img.w * img.h * 3];
    buf.par_chunks_exact_mut(w * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w {
                let h = pcg3d([x as u32, y as u32, 0xd17]);
                for c in 0..3 {
                    let d = unit(h[c]) - 0.5;
                    let v = img.data[(y * w + x) * 3 + c] * 255.0 + d;
                    row[x * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        });
    image::RgbImage::from_raw(img.w as u32, img.h as u32, buf).expect("size")
}

pub fn encode_jpeg(img: &ImageF32, quality: u8) -> Result<Vec<u8>, Error> {
    let rgb = to_rgb8(img);
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    rgb.write_with_encoder(enc)
        .map_err(|e| Error::Encode(e.to_string()))?;
    Ok(out)
}

/// Linear → display, for "no film" previews / tests.
pub fn display_from_linear(img: &ImageF32) -> ImageF32 {
    let mut o = img.clone();
    o.data
        .par_iter_mut()
        .for_each(|v| *v = linear_to_srgb(*v).clamp(0.0, 1.0));
    o
}
