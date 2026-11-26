//! JPEG/PNG decode → linear f32, and display f32 → JPEG/PNG encode.

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

/// Highest JPEG quality: every quantiser step is 1. Use this for heavy lens
/// blur, whose large smooth gradients band and block at lower qualities.
pub const JPEG_MAX_QUALITY: u8 = 100;

/// Container for a developed photo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// Baseline JPEG, 4:4:4 (no chroma subsampling), quality from the options.
    #[default]
    Jpeg,
    /// 8-bit RGB PNG (lossless after the 8-bit dither).
    Png,
}

impl OutputFormat {
    /// FFI code: 0 = JPEG, 1 = PNG.
    pub fn from_code(code: i32) -> Result<Self, Error> {
        match code {
            0 => Ok(Self::Jpeg),
            1 => Ok(Self::Png),
            c => Err(Error::Encode(format!("unknown output format {c}"))),
        }
    }
}

/// JPEG at `quality` (1..=100). The `image` encoder never subsamples chroma,
/// so the output is always 4:4:4; at [`JPEG_MAX_QUALITY`] every quantiser
/// step is 1.
pub fn encode_jpeg(img: &ImageF32, quality: u8) -> Result<Vec<u8>, Error> {
    let rgb = to_rgb8(img);
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    rgb.write_with_encoder(enc)
        .map_err(|e| Error::Encode(e.to_string()))?;
    Ok(out)
}

/// 8-bit RGB PNG (same dither as the JPEG path).
pub fn encode_png(img: &ImageF32) -> Result<Vec<u8>, Error> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let rgb = to_rgb8(img);
    let mut out = Vec::new();
    let enc =
        PngEncoder::new_with_quality(&mut out, CompressionType::Default, FilterType::Adaptive);
    rgb.write_with_encoder(enc)
        .map_err(|e| Error::Encode(e.to_string()))?;
    Ok(out)
}

/// Encode in `format`; `jpeg_quality` is ignored for PNG.
pub fn encode(img: &ImageF32, format: OutputFormat, jpeg_quality: u8) -> Result<Vec<u8>, Error> {
    match format {
        OutputFormat::Jpeg => encode_jpeg(img, jpeg_quality),
        OutputFormat::Png => encode_png(img),
    }
}

/// Linear → display, for "no film" previews / tests.
pub fn display_from_linear(img: &ImageF32) -> ImageF32 {
    let mut o = img.clone();
    o.data
        .par_iter_mut()
        .for_each(|v| *v = linear_to_srgb(*v).clamp(0.0, 1.0));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> ImageF32 {
        let mut img = ImageF32::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                img.data[i] = x as f32 / w as f32;
                img.data[i + 1] = y as f32 / h as f32;
                img.data[i + 2] = 0.5;
            }
        }
        img
    }

    /// (h, v) sampling factors of each component in the SOF0 frame header.
    fn sof_sampling(jpeg: &[u8]) -> Vec<u8> {
        let mut i = 2;
        while i + 4 < jpeg.len() {
            assert_eq!(jpeg[i], 0xFF);
            let marker = jpeg[i + 1];
            let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
            if marker == 0xC0 {
                let n = jpeg[i + 9] as usize;
                return (0..n).map(|c| jpeg[i + 10 + c * 3 + 1]).collect();
            }
            i += 2 + len;
        }
        panic!("no SOF0");
    }

    /// The quantisation tables (DQT payloads after the Pq/Tq byte).
    fn dqt_values(jpeg: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 2;
        while i + 4 < jpeg.len() && jpeg[i + 1] != 0xDA {
            let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
            if jpeg[i + 1] == 0xDB {
                let seg = &jpeg[i + 4..i + 2 + len];
                for t in seg.chunks(65) {
                    out.extend_from_slice(&t[1..]);
                }
            }
            i += 2 + len;
        }
        out
    }

    #[test]
    fn max_jpeg_is_444_with_unit_quantisers() {
        let j = encode_jpeg(&gradient(64, 48), JPEG_MAX_QUALITY).unwrap();
        assert_eq!(sof_sampling(&j), vec![0x11, 0x11, 0x11]);
        let q = dqt_values(&j);
        assert!(!q.is_empty());
        assert!(q.iter().all(|&v| v == 1), "{q:?}");
    }

    #[test]
    fn png_round_trips_8bit_exactly() {
        let img = gradient(40, 30);
        let png = encode(&img, OutputFormat::Png, 0).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let back = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(back.as_raw(), to_rgb8(&img).as_raw());
    }

    #[test]
    fn format_codes() {
        assert_eq!(OutputFormat::from_code(0).unwrap(), OutputFormat::Jpeg);
        assert_eq!(OutputFormat::from_code(1).unwrap(), OutputFormat::Png);
        assert!(OutputFormat::from_code(7).is_err());
        let j = encode(&gradient(16, 16), OutputFormat::Jpeg, 95).unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
    }
}
