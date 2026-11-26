//! Desktop CLI for look development and testing.
//!
//!   emucam catalog
//!   emucam apply <in.jpg> -o out.jpg [--film ID] [--lens ID] [--body ID]
//!                [--shape heart] [--blur 1.0] [--ev 0] [--center 0.5,0.5]
//!                [--recipe recipe.json] [--max 2048] [--seed N] [--set key=value]
//!   emucam sheet <in.jpg> -o sheet.jpg [--kind films|lenses|shapes] [--max 640]
//!   emucam lut --film ID -o out.cube
//!   emucam refs -o dir/      (synthetic reference images)

use emucore::image::ImageF32;
use emucore::{io, pipeline, resolve, Catalog, Hooks, Recipe};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

struct Args {
    pos: Vec<String>,
    opts: HashMap<String, Vec<String>>,
}

impl Args {
    fn parse() -> Self {
        let mut pos = vec![];
        let mut opts: HashMap<String, Vec<String>> = HashMap::new();
        let mut it = std::env::args().skip(1).peekable();
        while let Some(a) = it.next() {
            if let Some(k) = a.strip_prefix("--").or_else(|| a.strip_prefix('-').filter(|s| s.len() == 1)) {
                let v = it.next().unwrap_or_else(|| die(format!("missing value for {a}")));
                opts.entry(k.to_string()).or_default().push(v);
            } else {
                pos.push(a);
            }
        }
        Args { pos, opts }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.opts.get(k).and_then(|v| v.last()).map(|s| s.as_str())
    }
    fn f(&self, k: &str) -> Option<f32> {
        self.get(k).map(|v| v.parse().unwrap_or_else(|_| die(format!("bad number for --{k}"))))
    }
}

fn load_input(path: &str, max: Option<usize>) -> ImageF32 {
    let bytes = std::fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let mut img = io::decode(&bytes).unwrap_or_else(|e| die(e));
    if let Some(m) = max {
        let long = img.w.max(img.h);
        if long > m {
            img = img.downsample(long.div_ceil(m));
        }
    }
    img
}

fn recipe_from_args(a: &Args) -> Recipe {
    let mut r = match a.get("recipe") {
        Some(p) => Recipe::from_json(&std::fs::read_to_string(p).unwrap_or_else(|e| die(e))).unwrap_or_else(|e| die(e)),
        None => Recipe::default(),
    };
    if let Some(v) = a.get("film") {
        r.film = Some(v.into());
    }
    if let Some(v) = a.get("lens") {
        r.lens = Some(v.into());
    }
    if let Some(v) = a.get("body") {
        r.body = Some(v.into());
    }
    if let Some(v) = a.get("shape") {
        r.params.aperture_shape = Some(v.into());
    }
    if let Some(v) = a.f("blur") {
        r.params.blur = v;
    }
    if let Some(v) = a.f("ev") {
        r.params.exposure = v;
    }
    if let Some(v) = a.f("warmth") {
        r.params.warmth = v;
    }
    if let Some(v) = a.f("punch") {
        r.params.punch = v;
    }
    if let Some(v) = a.f("angle") {
        r.params.focus_angle = v;
    }
    if let Some(v) = a.f("size") {
        r.params.focus_size = v;
    }
    if let Some(v) = a.get("seed") {
        r.seed = v.parse().unwrap_or(0);
    }
    if let Some(c) = a.get("center") {
        let p: Vec<f32> = c.split(',').filter_map(|s| s.parse().ok()).collect();
        if p.len() == 2 {
            r.params.focus_center = Some([p[0], p[1]]);
        }
    }
    for kv in a.opts.get("set").into_iter().flatten() {
        if let Some((k, v)) = kv.split_once('=') {
            r.params.values.insert(k.into(), v.parse().unwrap_or_else(|_| die(format!("bad --set {kv}"))));
        }
    }
    for d in a.opts.get("disable").into_iter().flatten() {
        r.disabled_effects.push(d.clone());
    }
    r
}

fn render(img: &ImageF32, r: &Recipe, cat: &Catalog) -> ImageF32 {
    let res = resolve(r, cat).unwrap_or_else(|e| die(e));
    pipeline::render(img, &res, &Hooks::default()).unwrap_or_else(|e| die(e))
}

fn save(img: &ImageF32, out: &str) {
    let path = PathBuf::from(out);
    if out.ends_with(".png") {
        io::to_rgb8(img).save(&path).unwrap_or_else(|e| die(e));
    } else {
        std::fs::write(&path, io::encode_jpeg(img, 92).unwrap_or_else(|e| die(e))).unwrap_or_else(|e| die(e));
    }
    eprintln!("wrote {out} ({}x{})", img.w, img.h);
}

/// Tiny 5x7 bitmap font for contact-sheet labels.
fn draw_label(img: &mut ImageF32, x0: usize, y0: usize, text: &str, scale: usize) {
    const FONT: &[(char, [u8; 7])] = &[
        ('A', [14, 17, 17, 31, 17, 17, 17]), ('B', [30, 17, 17, 30, 17, 17, 30]), ('C', [14, 17, 16, 16, 16, 17, 14]),
        ('D', [30, 17, 17, 17, 17, 17, 30]), ('E', [31, 16, 16, 30, 16, 16, 31]), ('F', [31, 16, 16, 30, 16, 16, 16]),
        ('G', [14, 17, 16, 23, 17, 17, 15]), ('H', [17, 17, 17, 31, 17, 17, 17]), ('I', [14, 4, 4, 4, 4, 4, 14]),
        ('J', [7, 2, 2, 2, 2, 18, 12]), ('K', [17, 18, 20, 24, 20, 18, 17]), ('L', [16, 16, 16, 16, 16, 16, 31]),
        ('M', [17, 27, 21, 21, 17, 17, 17]), ('N', [17, 17, 25, 21, 19, 17, 17]), ('O', [14, 17, 17, 17, 17, 17, 14]),
        ('P', [30, 17, 17, 30, 16, 16, 16]), ('Q', [14, 17, 17, 17, 21, 18, 13]), ('R', [30, 17, 17, 30, 20, 18, 17]),
        ('S', [15, 16, 16, 14, 1, 1, 30]), ('T', [31, 4, 4, 4, 4, 4, 4]), ('U', [17, 17, 17, 17, 17, 17, 14]),
        ('V', [17, 17, 17, 17, 17, 10, 4]), ('W', [17, 17, 17, 21, 21, 21, 10]), ('X', [17, 17, 10, 4, 10, 17, 17]),
        ('Y', [17, 17, 10, 4, 4, 4, 4]), ('Z', [31, 1, 2, 4, 8, 16, 31]), ('0', [14, 17, 19, 21, 25, 17, 14]),
        ('1', [4, 12, 4, 4, 4, 4, 14]), ('2', [14, 17, 1, 2, 4, 8, 31]), ('3', [30, 1, 1, 14, 1, 1, 30]),
        ('4', [2, 6, 10, 18, 31, 2, 2]), ('5', [31, 16, 30, 1, 1, 17, 14]), ('6', [6, 8, 16, 30, 17, 17, 14]),
        ('7', [31, 1, 2, 4, 8, 8, 8]), ('8', [14, 17, 17, 14, 17, 17, 14]), ('9', [14, 17, 17, 15, 1, 2, 12]),
        ('-', [0, 0, 0, 31, 0, 0, 0]), ('.', [0, 0, 0, 0, 0, 12, 12]), (' ', [0; 7]),
    ];
    let mut x = x0;
    for ch in text.to_uppercase().chars() {
        let rows = FONT.iter().find(|(c, _)| *c == ch).map(|(_, r)| *r).unwrap_or([0; 7]);
        for (ry, bits) in rows.iter().enumerate() {
            for rx in 0..5 {
                if bits & (1 << (4 - rx)) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = x + rx * scale + sx;
                            let py = y0 + ry * scale + sy;
                            if px < img.w && py < img.h {
                                img.set(px, py, [0.93, 0.92, 0.89]);
                            }
                        }
                    }
                }
            }
        }
        x += 6 * scale;
    }
}

fn sheet(a: &Args, cat: &Catalog) {
    let input = a.pos.get(1).unwrap_or_else(|| die("sheet <in.jpg> -o out.jpg"));
    let out = a.get("o").unwrap_or("sheet.jpg");
    let max = a.f("max").map(|v| v as usize).unwrap_or(640);
    let img = load_input(input, Some(max));
    let kind = a.get("kind").unwrap_or("films");
    let base = recipe_from_args(a);
    let variants: Vec<(String, Recipe)> = match kind {
        "films" => cat.films.values().map(|f| (f.ui.name.clone(), Recipe { film: Some(f.id.clone()), ..base.clone() })).collect(),
        "lenses" => cat.lenses.values().map(|l| (l.ui.name.clone(), Recipe { lens: Some(l.id.clone()), ..base.clone() })).collect(),
        "bodies" => cat.bodies.values().map(|b| (b.ui.name.clone(), Recipe { body: Some(b.id.clone()), ..base.clone() })).collect(),
        "shapes" => emucore::lens::aperture::SHAPE_IDS
            .iter()
            .map(|s| {
                let mut r = base.clone();
                r.lens.get_or_insert("creative-50".into());
                r.params.aperture_shape = Some(s.to_string());
                (s.to_string(), r)
            })
            .collect(),
        _ => die("--kind films|lenses|bodies|shapes"),
    };
    let only: Option<Vec<String>> = a.get("only").map(|v| v.split(',').map(|s| s.to_string()).collect());
    let ids: Vec<String> = match kind {
        "films" => cat.films.values().map(|f| f.id.clone()).collect(),
        "lenses" => cat.lenses.values().map(|l| l.id.clone()).collect(),
        "bodies" => cat.bodies.values().map(|b| b.id.clone()).collect(),
        _ => emucore::lens::aperture::SHAPE_IDS.iter().map(|s| s.to_string()).collect(),
    };
    let variants: Vec<(String, Recipe)> = variants
        .into_iter()
        .zip(ids)
        .filter(|(_, id)| only.as_ref().map(|o| o.contains(id)).unwrap_or(true))
        .map(|(v, _)| v)
        .collect();
    let t = Instant::now();
    use rayon::prelude::*;
    let tiles: Vec<(String, ImageF32)> = variants.par_iter().map(|(n, r)| (n.clone(), render(&img, r, cat))).collect();
    let cols = a.f("cols").map(|v| v as usize).unwrap_or(4).min(tiles.len()).max(1);
    let rows = tiles.len().div_ceil(cols);
    let pad = 12;
    let label_h = 28;
    let (tw, th) = (img.w, img.h);
    let mut sheet = ImageF32::filled(cols * (tw + pad) + pad, rows * (th + pad + label_h) + pad, [0.043, 0.043, 0.047]);
    for (i, (name, t)) in tiles.iter().enumerate() {
        let ox = pad + (i % cols) * (tw + pad);
        let oy = pad + (i / cols) * (th + pad + label_h);
        for y in 0..th {
            for x in 0..tw {
                sheet.set(ox + x, oy + y, t.get(x, y));
            }
        }
        draw_label(&mut sheet, ox, oy + th + 8, name, 2);
    }
    eprintln!("rendered {} variants in {:.2?}", tiles.len(), t.elapsed());
    save(&sheet, out);
}

fn refs(a: &Args) {
    let dir = PathBuf::from(a.get("o").unwrap_or("refs"));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| die(e));
    // Point-light grid (bokeh shapes) over dark gradient.
    let (w, h) = (1200, 800);
    let mut img = ImageF32::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = 0.02 + 0.03 * (y as f32 / h as f32);
            img.set(x, y, [v, v * 0.9, v * 1.2]);
        }
    }
    for gy in 0..6 {
        for gx in 0..9 {
            let cx = 70 + gx * 132;
            let cy = 70 + gy * 132;
            for dy in 0..3 {
                for dx in 0..3 {
                    let tint = [[1.0, 0.8, 0.5], [0.6, 0.8, 1.0], [1.0, 0.5, 0.6]][(gx + gy) % 3];
                    img.set(cx + dx, cy + dy, tint);
                }
            }
        }
    }
    save(&io::display_from_linear(&img), dir.join("points.png").to_str().unwrap());
    // Exposure wedge + color patches.
    let mut wedge = ImageF32::new(w, 400);
    let patches: [[f32; 3]; 8] = [
        [0.45, 0.32, 0.25], [0.08, 0.30, 0.05], [0.25, 0.40, 0.80], [0.70, 0.12, 0.10],
        [0.85, 0.70, 0.15], [0.10, 0.45, 0.45], [0.50, 0.20, 0.55], [0.60, 0.55, 0.50],
    ];
    for y in 0..400 {
        for x in 0..w {
            let c = if y < 200 {
                let ev = -8.0 + 12.0 * (x / 100) as f32 / 11.0;
                let v = (0.18 * ev.exp2()).min(1.0);
                [v, v, v]
            } else {
                patches[(x * 8 / w).min(7)]
            };
            wedge.set(x, y, c);
        }
    }
    save(&io::display_from_linear(&wedge), dir.join("wedge.png").to_str().unwrap());
}

fn main() {
    let a = Args::parse();
    let cat = Catalog::builtin();
    match a.pos.first().map(|s| s.as_str()) {
        Some("catalog") => {
            let v: serde_json::Value = serde_json::from_str(&cat.to_ui_json()).unwrap();
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        }
        Some("apply") => {
            let input = a.pos.get(1).unwrap_or_else(|| die("apply <in.jpg> -o out.jpg"));
            let out = a.get("o").unwrap_or("out.jpg");
            let img = load_input(input, a.f("max").map(|v| v as usize));
            let r = recipe_from_args(&a);
            let t = Instant::now();
            let res = render(&img, &r, cat);
            eprintln!("rendered {}x{} in {:.2?}", img.w, img.h, t.elapsed());
            eprintln!("recipe: {}", r.to_json());
            save(&res, out);
        }
        Some("sheet") => sheet(&a, cat),
        Some("lut") => {
            let r = recipe_from_args(&a);
            let res = resolve(&r, cat).unwrap_or_else(|e| die(e));
            let luts = emucore::lut::bake(&res, 33);
            let out = a.get("o").unwrap_or("film.cube");
            std::fs::write(out, luts.develop.to_cube(r.film.as_deref().unwrap_or("digital"))).unwrap_or_else(|e| die(e));
            eprintln!("wrote {out} (input: log2 shaper {}..{} EV around 0.18)", emucore::film::LOG_MIN, emucore::film::LOG_MAX);
        }
        Some("refs") => refs(&a),
        Some("preview") => {
            let r = recipe_from_args(&a);
            let res = resolve(&r, cat).unwrap_or_else(|e| die(e));
            println!("{}", serde_json::to_string_pretty(&emucore::preview::params(&res)).unwrap());
        }
        _ => {
            eprintln!("usage: emucam <catalog|apply|sheet|lut|refs|preview> ...");
            std::process::exit(2);
        }
    }
}
