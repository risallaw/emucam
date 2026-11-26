//! `Recipe` = what the user picked (ids + adjustments). It is stored with every
//! shot (JSON) so a photo can be re-developed later. `resolve()` turns it into
//! concrete parameters using the catalog.

use crate::catalog::Catalog;
use crate::preset::{
    Aperture, BlurField, EffectSpec, FilmPreset, LensOptics, LensPreset, SeedMode,
};
use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RECIPE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    #[serde(default = "v1")]
    pub version: u32,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub film: Option<String>,
    #[serde(default)]
    pub lens: Option<String>,
    #[serde(default)]
    pub params: UserParams,
    /// Extra user-added effects (appended after preset effects).
    #[serde(default)]
    pub effects: Vec<EffectSpec>,
    /// Effect ids (bundled by presets) the user switched off.
    #[serde(default)]
    pub disabled_effects: Vec<String>,
    /// Shot seed (grain, leaks). Same seed ⇒ identical output.
    #[serde(default)]
    pub seed: u64,
    /// How the device was turned when the shot was taken (0..3 =
    /// Surface.ROTATION_0/90/180/270), viewfinder → photo frame. Placed
    /// effects and focus are already rotated by the app; effects positioned
    /// at random in image uv (a body's light leak, prism side) use this so the
    /// photo matches the portrait viewfinder. 0 = as composed (default).
    #[serde(default)]
    pub capture_rotation: u8,
}
fn v1() -> u32 {
    RECIPE_VERSION
}

impl PartialEq for EffectSpec {
    fn eq(&self, o: &Self) -> bool {
        self.kind == o.kind
            && self.id == o.id
            && self.strength == o.strength
            && self.params == o.params
    }
}

/// User adjustments. Named fields are common across presets; `values` holds
/// preset-declared controls (see `UiMeta.controls`) like "bokeh_control".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UserParams {
    /// Exposure compensation in stops (applied in linear before film).
    #[serde(default)]
    pub exposure: f32,
    /// Character pad X: −1 cool … +1 warm.
    #[serde(default)]
    pub warmth: f32,
    /// Character pad Y: −1 faded/soft … +1 punchy.
    #[serde(default)]
    pub punch: f32,
    #[serde(default = "one")]
    pub grain: f32,
    #[serde(default = "one")]
    pub halation: f32,
    /// Blur amount multiplier (≈ aperture: 1 = wide open).
    #[serde(default = "one")]
    pub blur: f32,
    /// Sweet spot / slice center in normalized image coords (0..1).
    #[serde(default)]
    pub focus_center: Option<[f32; 2]>,
    /// Sweet spot size multiplier.
    #[serde(default = "one")]
    pub focus_size: f32,
    /// Slice angle in degrees (Edge) / sweet-spot ellipse angle.
    #[serde(default)]
    pub focus_angle: f32,
    /// Creative aperture override (e.g. "star", "heart").
    #[serde(default)]
    pub aperture_shape: Option<String>,
    #[serde(default)]
    pub values: BTreeMap<String, f32>,
    /// Use the shot's depth map (when one exists) for defocus.
    #[serde(default = "yes")]
    pub depth: bool,
}
fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

impl Default for UserParams {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            warmth: 0.0,
            punch: 0.0,
            grain: 1.0,
            halation: 1.0,
            blur: 1.0,
            focus_center: None,
            focus_size: 1.0,
            focus_angle: 0.0,
            aperture_shape: None,
            values: BTreeMap::new(),
            depth: true,
        }
    }
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            version: RECIPE_VERSION,
            body: None,
            film: None,
            lens: None,
            params: UserParams::default(),
            effects: vec![],
            disabled_effects: vec![],
            seed: 0,
            capture_rotation: 0,
        }
    }
}

impl Recipe {
    pub fn from_json(s: &str) -> Result<Self, Error> {
        let mut v: serde_json::Value =
            serde_json::from_str(s).map_err(|e| Error::Recipe(e.to_string()))?;
        migrate(&mut v);
        serde_json::from_value(v).map_err(|e| Error::Recipe(e.to_string()))
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("recipe serializes")
    }
}

/// Upgrade older recipe JSON in place. Add a branch per version bump.
fn migrate(v: &mut serde_json::Value) {
    let ver = v.get("version").and_then(|x| x.as_u64()).unwrap_or(1);
    if ver < 1 {
        // (placeholder for future migrations)
    }
    if let Some(o) = v.as_object_mut() {
        o.insert("version".into(), RECIPE_VERSION.into());
    }
}

/// Concrete, catalog-resolved parameters for rendering.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub film: Option<FilmPreset>,
    pub lens: Option<LensPreset>,
    pub optics: LensOptics,
    pub focus_center: [f32; 2],
    pub focus_angle: f32,
    pub params: UserParams,
    pub effects: Vec<EffectSpec>,
    pub seed: u64,
    /// See [`Recipe::capture_rotation`] (0..3).
    pub capture_rotation: u8,
}

impl Resolved {
    pub fn value(&self, key: &str) -> Option<f32> {
        if let Some(v) = self.params.values.get(key) {
            return Some(*v);
        }
        let metas = self
            .lens
            .iter()
            .map(|l| &l.ui)
            .chain(self.film.iter().map(|f| &f.ui));
        for m in metas {
            if let Some(c) = m.controls.iter().find(|c| c.key == key) {
                return Some(c.default);
            }
        }
        None
    }
}

pub fn resolve(recipe: &Recipe, cat: &Catalog) -> Result<Resolved, Error> {
    let body = match &recipe.body {
        Some(id) => Some(
            cat.bodies
                .get(id)
                .ok_or_else(|| Error::UnknownPreset(format!("body {id}")))?,
        ),
        None => None,
    };
    let film_id = recipe
        .film
        .clone()
        .or_else(|| body.and_then(|b| b.film.clone()));
    let lens_id = recipe
        .lens
        .clone()
        .or_else(|| body.and_then(|b| b.lens.clone()));
    let film = match film_id {
        Some(id) => Some(
            cat.films
                .get(&id)
                .cloned()
                .ok_or_else(|| Error::UnknownPreset(format!("film {id}")))?,
        ),
        None => None,
    };
    let lens = match lens_id {
        Some(id) => Some(
            cat.lenses
                .get(&id)
                .cloned()
                .ok_or_else(|| Error::UnknownPreset(format!("lens {id}")))?,
        ),
        None => None,
    };

    let p = &recipe.params;
    let mut optics = lens.as_ref().map(|l| l.optics.clone()).unwrap_or_default();
    // User adjustments on optics.
    let blur = p.blur.clamp(0.0, 3.0);
    optics.rmax *= blur;
    optics.glow *= blur.min(1.5);
    optics.s0 *= p.focus_size.clamp(0.2, 4.0);
    if let Some(shape) = &p.aperture_shape {
        optics.aperture = match shape.as_str() {
            "circle" => Aperture::Circle,
            s if s.starts_with("poly") => Aperture::Polygon {
                blades: s[4..].parse().unwrap_or(6),
                rotation: 0.0,
            },
            s => Aperture::Shape { id: s.to_string() },
        };
    }

    let mut res = Resolved {
        film,
        lens,
        optics,
        focus_center: p.focus_center.unwrap_or([0.5, 0.5]),
        focus_angle: p.focus_angle,
        params: p.clone(),
        effects: vec![],
        seed: recipe.seed,
        capture_rotation: recipe.capture_rotation % 4,
    };

    // Preset-declared controls mapped onto optics.
    if let Some(bc) = res.value("bokeh_control") {
        // Petzval bokeh control 1..7 → swirl ≈ 0.2·L.
        res.optics.swirl = 0.2 * bc;
        res.optics.cats_eye = (0.4 + 0.08 * bc).min(1.2);
    }
    if let Some(v) = res.value("vignette_slider") {
        // Burnside second aperture.
        res.optics.cats_eye = v;
        res.optics.vignette = 0.3 + 0.4 * v;
    }
    if let Some(v) = res.value("fisheye_angle") {
        res.optics.fisheye_angle = v;
    }
    if matches!(res.optics.field, BlurField::None) && res.optics.rmax > 0.0 {
        res.optics.field = BlurField::Uniform;
    }

    // Effect union: body → lens → film → user, minus disabled.
    let mut effects: Vec<EffectSpec> = vec![];
    if let Some(b) = body {
        effects.extend(b.effects.iter().cloned());
    }
    if let Some(l) = &res.lens {
        effects.extend(l.effects.iter().cloned());
    }
    if let Some(f) = &res.film {
        effects.extend(f.effects.iter().cloned());
    }
    effects.extend(recipe.effects.iter().cloned());
    effects.retain(|e| !recipe.disabled_effects.iter().any(|d| d == e.key()));
    // Per-effect strength overrides via values["fx.<id>"].
    for e in effects.iter_mut() {
        if let Some(s) = p.values.get(&format!("fx.{}", e.key())) {
            e.strength = *s;
        }
    }
    res.effects = effects;
    Ok(res)
}

/// Seed for an effect instance: fixed effects ignore the shot seed.
pub fn effect_seed(e: &EffectSpec, shot_seed: u64, index: usize) -> u64 {
    let base = crate::noise::hash64(
        e.kind
            .as_bytes()
            .iter()
            .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(*b as u64)),
    );
    match e.seed_mode {
        SeedMode::Fixed => base ^ index as u64,
        SeedMode::PerShot => base ^ crate::noise::hash64(shot_seed.wrapping_add(index as u64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_and_defaults() {
        let r = Recipe::from_json(r#"{"film":"portrait-400"}"#).unwrap();
        assert_eq!(r.params.grain, 1.0);
        let r2 = Recipe::from_json(&r.to_json()).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn resolves_every_builtin_combination_head() {
        let cat = Catalog::builtin();
        for f in cat.films.keys() {
            for l in cat.lenses.keys().take(3) {
                let r = Recipe {
                    film: Some(f.clone()),
                    lens: Some(l.clone()),
                    ..Default::default()
                };
                resolve(&r, cat).unwrap();
            }
        }
    }

    #[test]
    fn disabled_effects_are_removed() {
        let cat = Catalog::builtin();
        let r = Recipe {
            body: Some("toy-cam".into()),
            disabled_effects: vec!["leak".into()],
            ..Default::default()
        };
        let res = resolve(&r, cat).unwrap();
        assert!(res.effects.iter().all(|e| e.key() != "leak"));
    }

    #[test]
    fn capture_rotation_defaults_to_upright_and_is_resolved() {
        let r = Recipe::from_json(r#"{"lens":"standard"}"#).unwrap();
        assert_eq!(r.capture_rotation, 0);
        let r = Recipe::from_json(r#"{"lens":"standard","capture_rotation":5}"#).unwrap();
        assert_eq!(resolve(&r, Catalog::builtin()).unwrap().capture_rotation, 1);
    }
}
