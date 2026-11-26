//! emucore — film & lens emulation engine.
//!
//! Pure Rust, no platform bindings: used by the Android app (via emucore-ffi),
//! the desktop CLI, and anything else that can call a C ABI.

pub mod blur;
pub mod catalog;
pub mod color;
pub mod depth;
pub mod effects;
pub mod film;
pub mod image;
pub mod io;
pub mod lens;
pub mod lut;
pub mod noise;
pub mod pipeline;
pub mod preset;
pub mod preview;
pub mod recipe;
pub mod seg;

pub use catalog::Catalog;
pub use pipeline::Hooks;
pub use recipe::{resolve, Recipe, Resolved};

#[derive(Debug)]
pub enum Error {
    Io(String),
    Decode(String),
    Encode(String),
    Recipe(String),
    Preset(String),
    UnknownPreset(String),
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Decode(e) => write!(f, "decode: {e}"),
            Error::Encode(e) => write!(f, "encode: {e}"),
            Error::Recipe(e) => write!(f, "recipe: {e}"),
            Error::Preset(e) => write!(f, "preset: {e}"),
            Error::UnknownPreset(e) => write!(f, "unknown preset: {e}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}
impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct DevelopOptions {
    /// Downscale so the long edge ≤ this (None = full resolution).
    pub max_dim: Option<usize>,
    /// JPEG quality (1..=100); ignored for PNG.
    pub jpeg_quality: u8,
    /// Output container (JPEG by default).
    pub format: io::OutputFormat,
    /// Raw depth-network output for this photo (see [`depth`]).
    pub depth: Option<RawDepth>,
    /// Raw segmentation logits for this photo (see [`seg`]); infrared films
    /// use the vegetation class. Invalid maps fall back to colour-only.
    pub veg: Option<RawSeg>,
}

impl Default for DevelopOptions {
    fn default() -> Self {
        Self {
            max_dim: None,
            jpeg_quality: 92,
            format: io::OutputFormat::Jpeg,
            depth: None,
            veg: None,
        }
    }
}

/// Raw output of a depth network, in its (portrait) tensor layout: one
/// `w`×`h` map, or the boosted set (see [`depth_input_boosted_jpeg`]).
#[derive(Clone, Debug)]
pub struct RawDepth {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
    pub kind: depth::DepthKind,
}

/// Raw output of a segmentation network: `classes` logit planes of `w`×`h`
/// in NCHW order (`data[c*w*h + y*w + x]`).
#[derive(Clone, Debug)]
pub struct RawSeg {
    pub classes: usize,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

/// Decode a JPEG (EXIF-oriented, like [`develop_jpeg`]) and build the depth
/// network's input tensor for it.
pub fn depth_input_jpeg(input: &[u8], layout: depth::ModelLayout) -> Result<Vec<f32>, Error> {
    layout.validate().map_err(Error::Recipe)?;
    let img = io::decode(input)?;
    Ok(depth::model_input(&img, layout))
}

/// Decode a JPEG and build the boosted-depth inputs: the whole-frame tensor
/// followed by one per [`depth::detail_tiles`] tile, concatenated. Their
/// outputs, concatenated in the same order, go in [`RawDepth::data`].
pub fn depth_input_boosted_jpeg(
    input: &[u8],
    layout: depth::ModelLayout,
) -> Result<Vec<f32>, Error> {
    layout.validate().map_err(Error::Recipe)?;
    let img = io::decode(input)?;
    let mut out = depth::model_input(&img, layout);
    for r in depth::detail_tiles() {
        out.extend(depth::model_input_region(&img, layout, r));
    }
    Ok(out)
}

/// Raw depth → nearness for a `img_w`×`img_h` photo. `data` holds one
/// `w`×`h` output (whole frame) or, for boosted depth, the whole-frame output
/// followed by the detail-tile outputs. None when unusable.
fn near_from_raw(d: &RawDepth, img_w: usize, img_h: usize) -> Option<image::MaskF32> {
    let n = d.w.checked_mul(d.h)?;
    let tiles = depth::TILE_GRID * depth::TILE_GRID;
    if n > 0 && d.data.len() == n * (1 + tiles) {
        let (g, rest) = d.data.split_at(n);
        let t: Vec<&[f32]> = rest.chunks_exact(n).collect();
        depth::nearness_boosted(g, &t, d.w, d.h, d.kind, img_w, img_h)
    } else {
        depth::nearness(&d.data, d.w, d.h, d.kind, img_w, img_h)
    }
}

/// Decode → render → encode (JPEG or PNG per `opts.format`; the name is
/// historical). The main entry point for capture.
pub fn develop_jpeg(
    input: &[u8],
    recipe: &Recipe,
    catalog: &Catalog,
    opts: &DevelopOptions,
    hooks: &Hooks,
) -> Result<Vec<u8>, Error> {
    let res = resolve(recipe, catalog)?;
    let mut img = io::decode(input)?;
    if let Some(md) = opts.max_dim {
        let long = img.w.max(img.h);
        if long > md {
            let f = long.div_ceil(md);
            img = img.downsample(f);
        }
    }
    // A bad depth map (wrong size, NaN, flat) falls back to the lens field.
    let near = opts
        .depth
        .as_ref()
        .and_then(|d| near_from_raw(d, img.w, img.h));
    // Likewise a bad segmentation map means colour-only vegetation.
    let veg = opts
        .veg
        .as_ref()
        .and_then(|s| seg::vegetation(&s.data, s.classes, s.w, s.h));
    let out = pipeline::render_with_aux(&img, &res, near.as_ref(), veg.as_ref(), hooks)?;
    let bytes = io::encode(&out, opts.format, opts.jpeg_quality)?;
    (hooks.progress)(1.0);
    Ok(bytes)
}
