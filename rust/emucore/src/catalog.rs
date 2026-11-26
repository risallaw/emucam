//! Preset catalog: built-in presets embedded at compile time, plus optional
//! external packs (directories of TOML files) loaded at runtime.

use crate::preset::{BodyPreset, FilmPreset, LensPreset};
use crate::Error;
use include_dir::{include_dir, Dir};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

static BUILTIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/presets");

#[derive(Default, Clone)]
pub struct Catalog {
    pub films: BTreeMap<String, FilmPreset>,
    pub lenses: BTreeMap<String, LensPreset>,
    pub bodies: BTreeMap<String, BodyPreset>,
}

#[derive(Serialize)]
struct Entry<'a> {
    id: &'a str,
    #[serde(flatten)]
    ui: &'a crate::preset::UiMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    iso: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    focal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fstop: Option<f32>,
    /// Lens geometry for UI overlays (reticle): field, s0, falloff.
    #[serde(skip_serializing_if = "Option::is_none")]
    optics: Option<serde_json::Value>,
    /// Body defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    film: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lens: Option<&'a str>,
    effects: Vec<&'a str>,
}

impl Catalog {
    /// Built-in presets (cached).
    pub fn builtin() -> &'static Catalog {
        static C: OnceLock<Catalog> = OnceLock::new();
        C.get_or_init(|| {
            let mut c = Catalog::default();
            c.load_dir_embedded(&BUILTIN)
                .expect("built-in presets must parse");
            c
        })
    }

    fn load_dir_embedded(&mut self, dir: &Dir<'_>) -> Result<(), Error> {
        for f in dir.files() {
            if f.path().extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let text = f.contents_utf8().unwrap_or_default();
            let kind = f
                .path()
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");
            self.add(kind, text, &f.path().display().to_string())?;
        }
        for d in dir.dirs() {
            self.load_dir_embedded(d)?;
        }
        Ok(())
    }

    /// Load an external preset pack: `<root>/{films,lenses,bodies}/*.toml`.
    /// Later files with the same id override earlier ones.
    pub fn load_pack(&mut self, root: &Path) -> Result<(), Error> {
        for kind in ["films", "lenses", "bodies"] {
            let d = root.join(kind);
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                    let text = std::fs::read_to_string(&p)?;
                    self.add(kind, &text, &p.display().to_string())?;
                }
            }
        }
        Ok(())
    }

    fn add(&mut self, kind: &str, text: &str, origin: &str) -> Result<(), Error> {
        let err = |e: toml::de::Error| Error::Preset(format!("{origin}: {e}"));
        match kind {
            "films" => {
                let p: FilmPreset = toml::from_str(text).map_err(err)?;
                self.films.insert(p.id.clone(), p);
            }
            "lenses" => {
                let p: LensPreset = toml::from_str(text).map_err(err)?;
                self.lenses.insert(p.id.clone(), p);
            }
            "bodies" => {
                let p: BodyPreset = toml::from_str(text).map_err(err)?;
                self.bodies.insert(p.id.clone(), p);
            }
            _ => {}
        }
        Ok(())
    }

    /// JSON catalog for UIs: `{films:[...], lenses:[...], bodies:[...]}`.
    pub fn to_ui_json(&self) -> String {
        let films: Vec<Entry> = self
            .films
            .values()
            .map(|f| Entry {
                id: &f.id,
                ui: &f.ui,
                iso: Some(f.iso),
                focal: None,
                fstop: None,
                optics: None,
                film: None,
                lens: None,
                effects: f.effects.iter().map(|e| e.key()).collect(),
            })
            .collect();
        let lenses: Vec<Entry> = self
            .lenses
            .values()
            .map(|l| Entry {
                id: &l.id,
                ui: &l.ui,
                iso: None,
                focal: Some(l.focal),
                fstop: Some(l.fstop),
                optics: Some(serde_json::json!({
                    "field": l.optics.field,
                    "s0": l.optics.s0,
                    "falloff": l.optics.falloff,
                    "blur": l.optics.rmax > 0.0 && l.optics.field != crate::preset::BlurField::None,
                })),
                film: None,
                lens: None,
                effects: l.effects.iter().map(|e| e.key()).collect(),
            })
            .collect();
        let bodies: Vec<Entry> = self
            .bodies
            .values()
            .map(|b| Entry {
                id: &b.id,
                ui: &b.ui,
                iso: None,
                focal: None,
                fstop: None,
                optics: None,
                film: b.film.as_deref(),
                lens: b.lens.as_deref(),
                effects: b.effects.iter().map(|e| e.key()).collect(),
            })
            .collect();
        serde_json::json!({
            "films": films,
            "lenses": lenses,
            "bodies": bodies,
            "shapes": crate::lens::aperture::SHAPE_IDS,
            "effect_kinds": crate::effects::KINDS,
        })
        .to_string()
    }
}
