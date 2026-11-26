# emucam

A film & lens emulation camera for Android. Pick a **film** (infrared, color
negative, slide, B&W), a **lens** (Lensbaby-style sweet spot / tilt slice /
glow, swirly Petzval & Helios, soap-bubble Trioplan, creative heart/star
apertures, pinhole) or a whole **camera body** (toy cam, disposable, expired
roll). The viewfinder shows the look live on the GPU; every photo is then
*developed* at full resolution by a Rust engine.

```
rust/                   Rust workspace (the engine — pure, platform-free)
  emucore/              film, lens, effects, pipeline, presets/*.toml
  emucore-ffi/          JNI bridge (JSON control plane + background develop jobs)
  emucore-cli/          desktop CLI for look development (`emucam`)
app/                    Compose UI: viewfinder, pickers, panels, contact sheet
camera/                 CameraX: preview (through GL effect) + ZSL capture
gl-render/              live preview: SurfaceProcessor + GLSL (assets/shaders)
develop/                shot store, develop queue, EXIF, MediaStore
designsystem/           theme tokens + components
core-model/             Kotlin mirror of Recipe / Catalog JSON
native-bridge/          EmuNative + Gradle task that runs cargo-ndk
```

## Build

Requirements: Android SDK + NDK 27.2, JDK 17, Rust with `aarch64-linux-android`,
`cargo-ndk`.

```sh
./gradlew :app:installDebug                 # builds Rust automatically (arm64)
./gradlew :app:installDebug -Pemucam.abis=arm64-v8a,x86_64
./gradlew :app:assembleRelease -Pemucam.rustRelease=true
cd rust && cargo test --release             # engine tests
```

## Look development on desktop

```sh
cd rust && cargo build --release
./target/release/emucam catalog                                   # list presets
./target/release/emucam apply in.jpg --film aero-ir --lens petzval-58 --set bokeh_control=7 -o out.jpg
./target/release/emucam sheet in.jpg --kind films -o films.jpg     # contact sheet of all films
./target/release/emucam sheet in.jpg --kind lenses --only helios-58,trioplan-100 -o l.jpg
./target/release/emucam sheet points.png --kind shapes -o shapes.jpg
./target/release/emucam refs -o refs/                             # synthetic test charts
EMUCAM_PROF=1 ./target/release/emucam apply big.jpg --lens sweet-50 -o x.jpg   # stage timings
```

## Adding looks (no code needed)

Everything is data in `rust/emucore/presets/`:

- **Film** → `films/<id>.toml`: tone curve (`contrast`, `pivot`, `black/white`,
  `crosstalk`), per-hue bands (`hue = [{ hue = 120, shift = -15, sat = 0.7 }]`,
  0° red · 120° green · 240° blue), `print`, `grain`, `halation`, optional `[ir]`,
  optional `[[effects]]`. Fuji-style (`fx-*`) and Leica-style (`lx-*`) looks are
  good templates.
- **Lens** → `lenses/<id>.toml`: `[optics]` (`field` = none/uniform/sweet_spot/slice,
  `s0`, `falloff`, `rmax`, `aperture`, `swirl`, `cats_eye`, `bubble`, `onion`,
  `ca_lateral`, `ca_long`, `glow`, `veil`, `vignette`), UI `controls`, and
  bundled `[[effects]]` (e.g. a toy lens that leaks light).
- **Camera body** → `bodies/<id>.toml`: default film + lens + effects.

The app's pickers, sliders and effect toggles are generated from the catalog,
so a new file shows up after rebuilding. Packs can also be loaded at runtime
(`EmuNative.loadPack(dir)`).

## Adding a new effect kind (code)

1. Implement `Effect` in `rust/emucore/src/effects.rs` and pick its physical
   `Slot` (Optical → Exposure → Emulsion → Print).
2. Add a `build()` arm and its name to `KINDS`.
3. Optional live preview: emit uniforms in `preview.rs` and use them in
   `gl-render/src/main/assets/shaders/final.frag` behind a `#define`.

Unknown kinds are skipped, so older app builds stay compatible with newer
recipes.

## How it works

- **Film**: per-channel logistic characteristic curve in log exposure with dye
  cross-talk, grain in density space (∝ √(y(1−y)), sized to a 36 mm frame),
  halation as red-weighted multi-lobe glow from highlights.
- **Infrared**: phones block IR, so the near-IR record is estimated (≈ red for
  most materials, ≈ 4–5× green for chlorophyll foliage via an ExG vegetation
  mask, low for blue sky), then the film's layer swap is applied
  (Aerochrome: IR→R, R→G, G→B).
- **Lens**: blur radius comes from the sweet spot / slice position (depth
  support planned). Bokeh is scatter-as-gather: each pixel spreads light in its
  *own* position-dependent kernel — aperture shape (SDF), tangential swirl,
  cat's-eye vignetting, soap-bubble rims, onion rings, chromatic aberration —
  over a factor-2 pyramid for speed. Highlights are expanded before blurring
  so point lights become bright shapes.
- **Preview ↔ capture parity**: Rust is the single source of truth. It exports
  shader uniforms by name and bakes the film 3D LUTs the GPU uses; GLSL mirrors
  the same formulas.
