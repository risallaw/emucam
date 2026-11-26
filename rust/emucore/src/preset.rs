//! Data-driven presets. Films, lenses and camera bodies are TOML files under
//! `presets/`; adding a new look = adding a file. All spatial values are
//! fractions of image width (or half-diagonal) so presets are resolution-free.

use crate::color::{Mat3, IDENTITY};
use serde::{Deserialize, Serialize};

fn one() -> f32 {
    1.0
}
fn one3() -> [f32; 3] {
    [1.0; 3]
}
fn identity() -> Mat3 {
    IDENTITY
}

/// UI metadata shared by every preset kind. Kotlin builds carousels from this.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct UiMeta {
    pub name: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub category: String,
    /// Hex card color, e.g. "#C2185B".
    #[serde(default)]
    pub color: String,
    /// User-adjustable parameter descriptors.
    #[serde(default)]
    pub controls: Vec<ControlSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlSpec {
    /// Parameter key in `UserParams.values` (e.g. "blur", "bokeh_control").
    pub key: String,
    pub label: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    #[serde(default)]
    pub step: f32,
}

// ---------------------------------------------------------------- effects

/// Pipeline slot an effect runs in. The order is physical:
/// optical (in the lens) → exposure (light hitting film) → emulsion → print.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// After bokeh, before film: flare, ghosts, veiling haze, prism.
    Optical,
    /// Added light in exposure space: leaks, fog. Gets halation + film curve.
    Exposure,
    /// On the developed image: dust, scratches.
    Emulsion,
    /// Final output: frames, toning, date stamp.
    Print,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SeedMode {
    /// Same every shot (deterministic per recipe seed).
    Fixed,
    /// Varies every shot like real film (seed mixed with shot seed).
    #[default]
    PerShot,
}

/// A generic effect entry. `kind` selects an implementation in the effect
/// registry; `params` are free-form so new kinds need no schema change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectSpec {
    pub kind: String,
    /// Stable id so the user can toggle a bundled effect (defaults to kind).
    #[serde(default)]
    pub id: String,
    #[serde(default = "one")]
    pub strength: f32,
    #[serde(default)]
    pub seed_mode: SeedMode,
    #[serde(default)]
    pub params: toml::Table,
}

impl EffectSpec {
    pub fn key(&self) -> &str {
        if self.id.is_empty() {
            &self.kind
        } else {
            &self.id
        }
    }
    /// Numeric param; non-finite values (TOML `nan`, `inf`) fall back to the default.
    pub fn f(&self, k: &str, default: f32) -> f32 {
        match self.params.get(k) {
            Some(toml::Value::Float(v)) if (*v as f32).is_finite() => *v as f32,
            Some(toml::Value::Integer(v)) => *v as f32,
            _ => default,
        }
    }
    pub fn s<'a>(&'a self, k: &str, default: &'a str) -> &'a str {
        match self.params.get(k) {
            Some(toml::Value::String(v)) => v.as_str(),
            _ => default,
        }
    }
    pub fn rgb(&self, k: &str, default: [f32; 3]) -> [f32; 3] {
        match self.params.get(k) {
            Some(toml::Value::Array(a)) if a.len() == 3 => {
                let mut o = default;
                for (i, v) in a.iter().enumerate() {
                    o[i] = match v {
                        toml::Value::Float(f) => *f as f32,
                        toml::Value::Integer(n) => *n as f32,
                        _ => default[i],
                    };
                }
                o
            }
            _ => default,
        }
    }
}

// ------------------------------------------------------------------- film

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilmKind {
    #[default]
    ColorNegative,
    Slide,
    BwNegative,
    ColorInfrared,
    BwInfrared,
}

/// Tone model: per-channel logistic characteristic curve in log2 exposure,
/// preceded by a dye cross-talk matrix (interimage effects).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToneSpec {
    /// Exposure-space color matrix (film spectral response / balance).
    #[serde(default = "identity")]
    pub input_matrix: Mat3,
    /// Film white balance in Kelvin (3200 = tungsten stock). 6500 = neutral.
    #[serde(default)]
    pub balance_kelvin: f32,
    /// Cross-talk applied to log exposure (negative off-diagonals = more saturation).
    #[serde(default = "identity")]
    pub crosstalk: Mat3,
    /// Contrast per channel; 1.0 ≈ neutral sRGB slope. Negatives ≈0.85–1.0, slides ≈1.3–1.5.
    pub contrast: [f32; 3],
    /// Mid-point offset per channel in stops (shifts color balance in mids).
    #[serde(default)]
    pub pivot: [f32; 3],
    /// Output black (toe) and white (shoulder) levels per channel, display-encoded.
    #[serde(default)]
    pub black: [f32; 3],
    #[serde(default = "one3")]
    pub white: [f32; 3],
    /// Monochrome spectral weights (B&W films). Ignored when all zero.
    #[serde(default)]
    pub mono: [f32; 3],
    #[serde(default = "one")]
    pub saturation: f32,
    /// Hue-selective adjustments (display space, after the curve), e.g. muted
    /// reds or cyan-leaning blues. Neutrals are never affected.
    #[serde(default)]
    pub hue: Vec<HueBand>,
}

/// One hue band: centre hue in degrees (0 red, 60 yellow, 120 green, 180 cyan,
/// 240 blue, 300 magenta), half-width, and what to do to colors inside it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct HueBand {
    pub hue: f32,
    #[serde(default = "default_band_width")]
    pub width: f32,
    /// Hue rotation in degrees.
    #[serde(default)]
    pub shift: f32,
    /// Chroma multiplier.
    #[serde(default = "one")]
    pub sat: f32,
    /// Relative brightness change (−0.3 darkens by 30%).
    #[serde(default)]
    pub lum: f32,
}
fn default_band_width() -> f32 {
    45.0
}

/// Print/finish stage, applied after grain in display space.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PrintSpec {
    /// Lifted blacks (0..0.2).
    #[serde(default)]
    pub fade: f32,
    /// Additive tint in shadows / highlights (display space, small values).
    #[serde(default)]
    pub shadow_tint: [f32; 3],
    #[serde(default)]
    pub highlight_tint: [f32; 3],
    /// Extra saturation multiplier at print.
    #[serde(default)]
    pub saturation: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrainSpec {
    /// Amplitude (0 = off). ~0.15 fine grain, ~0.45 heavy.
    pub amount: f32,
    /// Grain clump size in µm on a 36 mm frame.
    #[serde(default = "default_grain_um")]
    pub size_um: f32,
    /// 0 = monochrome grain, 1 = fully independent per channel.
    #[serde(default)]
    pub chroma: f32,
}
fn default_grain_um() -> f32 {
    15.0
}

impl Default for GrainSpec {
    fn default() -> Self {
        Self {
            amount: 0.0,
            size_um: 15.0,
            chroma: 0.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HalationSpec {
    pub strength: f32,
    /// Linear luma where halation starts (ramps to full at clipping, 1.0).
    #[serde(default = "default_halation_threshold")]
    pub threshold: f32,
    /// Radius multiplier on the default 0.3/0.8/2 % of width kernel.
    #[serde(default = "one")]
    pub radius: f32,
    /// Per-channel weights. Red halation ≈ [1, .3, .02]; neutral ≈ [1,1,1].
    #[serde(default = "default_halation_tint")]
    pub tint: [f32; 3],
}
fn default_halation_threshold() -> f32 {
    0.8
}
fn default_halation_tint() -> [f32; 3] {
    [1.0, 0.3, 0.02]
}
impl Default for HalationSpec {
    fn default() -> Self {
        Self {
            strength: 0.0,
            threshold: default_halation_threshold(),
            radius: 1.0,
            tint: default_halation_tint(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IrVariant {
    /// Kodak Aerochrome-like, yellow #12 filter: foliage red/magenta.
    Aerochrome,
    /// Orange #21 filter: pinker foliage, warmer, deeper skies.
    AerochromeOrange,
    /// B&W IR (HIE-like): white foliage, black skies.
    Mono,
    /// Golden IR (590nm-style channel swap): foliage yellow/gold, blue skies.
    Golden,
    /// Purple (LomoChrome-like): greens to purple.
    Purple,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IrSpec {
    pub variant: IrVariant,
    /// Foliage NIR reflectance relative to its green (≈4–5).
    #[serde(default = "default_kveg")]
    pub k_veg: f32,
    /// Sky darkening (0.6–0.8).
    #[serde(default = "default_ksky")]
    pub k_sky: f32,
    /// Vegetation index thresholds (ExG on chromaticity).
    #[serde(default = "default_veg_lo")]
    pub veg_lo: f32,
    #[serde(default = "default_veg_hi")]
    pub veg_hi: f32,
    /// Skin NIR reflectance relative to its red (melanin is nearly transparent
    /// in NIR). 0 = treat skin like any warm material.
    #[serde(default = "default_kskin")]
    pub k_skin: f32,
}
fn default_kskin() -> f32 {
    1.7
}
fn default_kveg() -> f32 {
    4.5
}
fn default_ksky() -> f32 {
    0.7
}
// ExG on linear chromaticity: sunlit leaves ≈0.3–0.7, shaded leaves ≈0.15,
// algae-stained concrete or green bounce light ≈0.02–0.06 (must stay out).
fn default_veg_lo() -> f32 {
    0.07
}
fn default_veg_hi() -> f32 {
    0.2
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilmPreset {
    pub id: String,
    pub kind: FilmKind,
    pub iso: u32,
    pub ui: UiMeta,
    pub tone: ToneSpec,
    #[serde(default)]
    pub print: PrintSpec,
    #[serde(default)]
    pub grain: GrainSpec,
    #[serde(default)]
    pub halation: HalationSpec,
    #[serde(default)]
    pub ir: Option<IrSpec>,
    #[serde(default)]
    pub effects: Vec<EffectSpec>,
}

// ------------------------------------------------------------------- lens

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlurField {
    /// No positional blur (sharp everywhere, e.g. plain glow lenses).
    #[default]
    None,
    /// Equal blur across the frame (pinhole, soft focus).
    Uniform,
    /// Sharp disc/ellipse sweet spot with blur growing outward.
    SweetSpot,
    /// Sharp slice (tilt) with blur growing perpendicular to it.
    Slice,
}

/// How the lens maps the scene onto the frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    /// Ordinary lens: straight lines stay straight.
    #[default]
    Rectilinear,
    /// Full-frame (diagonal) fisheye: equidistant projection filling the frame.
    Fisheye,
    /// Circular fisheye: the image circle sits inside the frame, black around it.
    Circular,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Aperture {
    #[default]
    Circle,
    Polygon {
        blades: u32,
        #[serde(default)]
        rotation: f32,
    },
    /// Procedural or bitmap creative aperture: "star", "heart", "sparkle", ...
    Shape { id: String },
    /// Slit (Sol bokeh blades) — ratio = minor/major.
    Slit {
        ratio: f32,
        #[serde(default)]
        rotation: f32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LensOptics {
    #[serde(default)]
    pub field: BlurField,
    /// Sweet spot radius (fraction of half-diagonal) / slice half-width.
    #[serde(default)]
    pub s0: f32,
    /// Falloff width (fraction of half-diagonal).
    #[serde(default = "default_falloff")]
    pub falloff: f32,
    /// Max blur radius, fraction of image width.
    #[serde(default)]
    pub rmax: f32,
    #[serde(default)]
    pub aperture: Aperture,
    /// Tangential stretch of off-axis bokeh (swirl / Petzval).
    #[serde(default)]
    pub swirl: f32,
    /// Mechanical vignetting → cat's-eye bokeh.
    #[serde(default)]
    pub cats_eye: f32,
    /// Soap-bubble edge brightness (+) or smooth/soft edge (−).
    #[serde(default)]
    pub bubble: f32,
    /// Onion-ring ripple strength.
    #[serde(default)]
    pub onion: f32,
    /// Lateral chromatic aberration.
    #[serde(default)]
    pub ca_lateral: f32,
    /// Longitudinal CA (per-channel blur radius delta).
    #[serde(default)]
    pub ca_long: f32,
    /// Spherical-aberration glow (Velvet / soft focus).
    #[serde(default)]
    pub glow: f32,
    /// Glow radius, fraction of width.
    #[serde(default = "default_glow_radius")]
    pub glow_radius: f32,
    /// Optical vignetting strength.
    #[serde(default)]
    pub vignette: f32,
    /// Contrast loss (uncoated/plastic) — veiling flare lift.
    #[serde(default)]
    pub veil: f32,
    /// Highlight boost before blur (so specular points become bokeh shapes).
    #[serde(default = "default_hl_gain")]
    pub highlight_gain: f32,
    /// The blur field is an optical property of the lens (Lensbaby sweet
    /// spot / tilt slice), so with a depth map it is *combined* with depth
    /// defocus. When false (classic lenses) the field was only a stand-in for
    /// depth and a depth map *replaces* it.
    #[serde(default)]
    pub field_optical: bool,
    /// Depth defocus strength: >1 = background goes soft faster.
    #[serde(default = "one")]
    pub depth_scale: f32,
    #[serde(default)]
    pub projection: Projection,
    /// Fisheye curvature: the half-angle (degrees, 5..85) the frame edge
    /// (diagonal, or circle rim) stands for. Larger = stronger bulge.
    #[serde(default = "default_fisheye_angle")]
    pub fisheye_angle: f32,
}
impl Default for LensOptics {
    /// Same values as an empty `[optics]` table, so presets and code agree.
    fn default() -> Self {
        serde_json::from_str("{}").expect("all LensOptics fields have serde defaults")
    }
}

fn default_falloff() -> f32 {
    0.5
}
fn default_fisheye_angle() -> f32 {
    70.0
}
fn default_glow_radius() -> f32 {
    0.012
}
fn default_hl_gain() -> f32 {
    6.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LensPreset {
    pub id: String,
    /// Display focal length (mm) / max aperture, for the UI.
    #[serde(default)]
    pub focal: u32,
    #[serde(default)]
    pub fstop: f32,
    pub ui: UiMeta,
    pub optics: LensOptics,
    #[serde(default)]
    pub effects: Vec<EffectSpec>,
}

// ------------------------------------------------------------------- body

/// A camera body bundles a film + lens + effects (e.g. "toy cam", "disposable").
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BodyPreset {
    pub id: String,
    pub ui: UiMeta,
    #[serde(default)]
    pub film: Option<String>,
    #[serde(default)]
    pub lens: Option<String>,
    #[serde(default)]
    pub effects: Vec<EffectSpec>,
}
