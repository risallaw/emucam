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
    pub jpeg_quality: u8,
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
            depth: None,
            veg: None,
        }
    }
}

/// Raw output of a depth network, in its (portrait) tensor layout.
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

/// Decode → render → encode. The main entry point for capture.
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
        .and_then(|d| depth::nearness(&d.data, d.w, d.h, d.kind, img.w, img.h));
    // Likewise a bad segmentation map means colour-only vegetation.
    let veg = opts
        .veg
        .as_ref()
        .and_then(|s| seg::vegetation(&s.data, s.classes, s.w, s.h));
    let out = pipeline::render_with_aux(&img, &res, near.as_ref(), veg.as_ref(), hooks)?;
    let bytes = io::encode_jpeg(&out, opts.jpeg_quality)?;
    (hooks.progress)(1.0);
    Ok(bytes)
}
