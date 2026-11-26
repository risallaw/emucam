package com.emucam.app.ui

import android.graphics.BitmapFactory
import androidx.camera.view.PreviewView
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import com.emucam.app.EmucamApp
import com.emucam.app.Panel
import com.emucam.app.PickerTab
import com.emucam.app.USER_EFFECTS
import com.emucam.app.UiState
import com.emucam.app.ViewfinderViewModel
import com.emucam.develop.ShotState
import com.emucam.model.Grid
import com.emucam.model.OMNI_WANDS
import com.emucam.model.PLACEABLE_EFFECTS
import com.emucam.model.effectPosition
import com.emucam.model.gridSegments
import com.emucam.model.Omni
import com.emucam.model.omni
import com.emucam.ds.Chip
import com.emucam.ds.DetentWheel
import com.emucam.ds.PositionDial
import com.emucam.ds.Emu
import com.emucam.ds.FilmCard
import com.emucam.ds.FocusShape
import com.emucam.ds.GlyphButton
import com.emucam.ds.GuideOverlay
import com.emucam.ds.ShutterButton
import com.emucam.ds.SweetSpotReticle
import com.emucam.ds.XYPad
import com.emucam.ds.parseHex
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.math.abs

@Composable
fun ViewfinderScreen(s: UiState, vm: ViewfinderViewModel) {
    val engine = (LocalContext.current.applicationContext as EmucamApp).engine
    val film = s.catalog.film(engine.filmId(s.recipe))
    val lens = s.catalog.lens(engine.lensId(s.recipe))
    val body = s.catalog.body(s.recipe.body)

    Column(Modifier.fillMaxSize().background(Emu.Ink).statusBarsPadding().navigationBarsPadding()) {
        StatusStrip(s, film?.name ?: "Digital", film?.iso, lens?.name ?: "Standard", lens?.fstop, body?.name)

        // ---- viewfinder (3:4, FIT so overlay coords == image coords)
        Box(Modifier.fillMaxWidth().aspectRatio(3f / 4f).background(Color.Black)) {
            val owner = LocalLifecycleOwner.current
            AndroidView(
                factory = { ctx ->
                    PreviewView(ctx).apply {
                        implementationMode = PreviewView.ImplementationMode.COMPATIBLE
                        scaleType = PreviewView.ScaleType.FIT_CENTER
                        vm.bindCamera(owner, this)
                    }
                },
                modifier = Modifier.fillMaxSize(),
            )
            val optics = lens?.optics
            val shape = when (optics?.field) {
                "sweet_spot" -> FocusShape.SPOT
                "slice" -> FocusShape.SLICE
                else -> FocusShape.NONE
            }
            val c = s.recipe.params.focusCenter ?: listOf(0.5f, 0.5f)
            SweetSpotReticle(
                shape = shape,
                center = Offset(c[0], c[1]),
                radius = (optics?.s0 ?: 0f) * s.recipe.params.focusSize,
                falloff = optics?.falloff ?: 0.5f,
                angleDeg = s.recipe.params.focusAngle,
                onMove = { vm.moveFocus(it.x, it.y) },
                onResize = { vm.scaleFocus(it) },
                onRotate = { vm.rotateFocus(it) },
                onTap = { vm.tapFocus(it.x, it.y) },
                onReset = { vm.resetFocus() },
                onGestureEnd = { vm.focusGestureEnd() },
            )
            if (s.flash) Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.6f)))
            s.camera.effectError?.let {
                Text(
                    "Live look unavailable on this device — photos still develop with full film & lens.",
                    style = Emu.Body.copy(color = Emu.Amber),
                    modifier = Modifier.align(Alignment.TopCenter).padding(12.dp).background(Emu.Ink.copy(alpha = 0.7f)).padding(8.dp),
                )
            }
            if (s.grid != Grid.OFF) {
                val segs = remember(s.grid, s.gridOrient) { gridSegments(s.grid, 3f / 4f, s.gridOrient) }
                GuideOverlay(segs, Modifier.fillMaxSize())
                if (s.grid.orientable) {
                    GlyphButton("↻", { vm.rotateGrid() }, Modifier.align(Alignment.TopEnd).padding(10.dp))
                }
            }
            if (s.panel == Panel.NONE && s.camera.maxZoom > s.camera.minZoom) {
                ZoomChips(s.camera.zoom, s.camera.minZoom, s.camera.maxZoom, vm::setZoom, Modifier.align(Alignment.BottomCenter).padding(bottom = 12.dp))
            }
            // ---- panels overlay the lower part of the viewfinder (glass over the image)
            androidx.compose.animation.AnimatedVisibility(
                s.panel != Panel.NONE,
                modifier = Modifier.align(Alignment.BottomCenter),
                enter = slideInVertically { it } + fadeIn(),
                exit = slideOutVertically { it } + fadeOut(),
            ) {
                Column(Modifier.fillMaxWidth().background(Emu.Ink.copy(alpha = 0.82f)).padding(vertical = 10.dp)) {
                    when (s.panel) {
                        Panel.FILM -> FilmPanel(s, vm)
                        Panel.LENS -> LensPanel(s, vm)
                        Panel.ADJUST -> AdjustPanel(s, vm)
                        Panel.EFFECTS -> EffectsPanel(s, vm)
                        Panel.NONE -> {}
                    }
                }
            }
            s.message?.let {
                LaunchedEffect(it) { delay(2500); vm.dismissMessage() }
                Text(it, style = Emu.Body.copy(color = Emu.Bone), modifier = Modifier.align(Alignment.BottomCenter).padding(12.dp).background(Emu.Ink.copy(alpha = 0.75f)).padding(8.dp))
            }
        }

        Spacer(Modifier.weight(1f))

        // ---- control deck
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            FilmWindow(film?.name ?: body?.name ?: "Digital", parseHex(film?.color ?: "#8A8780"), s.panel == Panel.FILM) { vm.setPanel(Panel.FILM) }
            Spacer(Modifier.weight(1f))
            ShutterButton(onClick = { vm.shutter() }, developing = s.developing?.progress)
            Spacer(Modifier.weight(1f))
            LensWindow(lens?.name ?: "Standard", lens?.focal, s.panel == Panel.LENS) { vm.setPanel(Panel.LENS) }
        }
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 18.dp).padding(bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            LastShotThumb(s, vm)
            Spacer(Modifier.weight(1f))
            GlyphButton("±", { vm.setPanel(Panel.ADJUST) }, active = s.panel == Panel.ADJUST)
            GlyphButton("FX", { vm.setPanel(Panel.EFFECTS) }, active = s.panel == Panel.EFFECTS)
            GlyphButton("#", { vm.cycleGrid() }, active = s.grid != Grid.OFF)
            GlyphButton("⟲", { vm.camera.switchCamera() })
        }
    }
}

@Composable
private fun StatusStrip(s: UiState, film: String, iso: Int?, lens: String, fstop: Float?, body: String?) {
    val p = s.recipe.params
    val ev = if (abs(p.exposure) < 0.05f) "±0.0" else "%+.1f".format(p.exposure)
    val text = listOfNotNull(
        body?.uppercase(),
        film.uppercase(),
        iso?.let { "ISO $it" },
        lens.uppercase(),
        fstop?.let { "ƒ/" + if (it >= 10) it.toInt().toString() else "%.1f".format(it) },
        "EV $ev",
    ).joinToString("  ·  ")
    Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(6.dp).clip(RoundedCornerShape(50)).background(if (s.camera.bound) Emu.Signal else Emu.Muted))
        Spacer(Modifier.width(8.dp))
        Text(text, style = Emu.Mono.copy(color = Emu.Bone), maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun FilmWindow(name: String, color: Color, active: Boolean, onClick: () -> Unit) {
    Column(
        Modifier.width(96.dp).clip(RoundedCornerShape(6.dp)).border(1.dp, if (active) Emu.Bone else Emu.Line, RoundedCornerShape(6.dp)).clickable(onClick = onClick),
    ) {
        Box(Modifier.fillMaxWidth().height(5.dp).background(color))
        Column(Modifier.padding(horizontal = 8.dp, vertical = 6.dp)) {
            Text("FILM", style = Emu.Label)
            Text(name.uppercase(), style = Emu.Mono.copy(color = Emu.Bone), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun LensWindow(name: String, focal: Int?, active: Boolean, onClick: () -> Unit) {
    Column(
        Modifier.width(96.dp).clip(RoundedCornerShape(48.dp)).border(1.dp, if (active) Emu.Bone else Emu.Line, RoundedCornerShape(48.dp)).clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(focal?.let { "${it}mm" } ?: "LENS", style = Emu.Label)
        Text(name.uppercase(), style = Emu.Mono.copy(color = Emu.Bone), maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun LastShotThumb(s: UiState, vm: ViewfinderViewModel) {
    val last = s.shots.firstOrNull()
    val ctx = LocalContext.current
    val app = ctx.applicationContext as EmucamApp
    val bmp = produceState<ImageBitmap?>(null, last?.id, last?.state) {
        value = last?.takeIf { it.state == ShotState.DONE }?.let { loadThumb(app.store.developed(it), 160) }
    }.value
    Box(
        Modifier.size(44.dp).clip(RoundedCornerShape(6.dp)).background(Emu.Graphite).border(1.dp, if (s.developing != null) Emu.Amber else Emu.Line, RoundedCornerShape(6.dp)).clickable { vm.showGallery(true) },
        contentAlignment = Alignment.Center,
    ) {
        if (bmp != null) Image(bmp, null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        else Text(if (s.developing != null) "DEV" else "▦", style = Emu.Mono.copy(color = if (s.developing != null) Emu.Amber else Emu.Muted))
    }
}

suspend fun loadThumb(f: File, target: Int): ImageBitmap? = withContext(Dispatchers.IO) {
    if (!f.exists()) return@withContext null
    val o = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeFile(f.path, o)
    var sample = 1
    while (o.outWidth / (sample * 2) >= target && o.outHeight / (sample * 2) >= target) sample *= 2
    BitmapFactory.decodeFile(f.path, BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
}

// ----------------------------------------------------------------- panels

@Composable
private fun FilmPanel(s: UiState, vm: ViewfinderViewModel) {
    Row(Modifier.padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Chip("Film", s.pickerTab == PickerTab.FILM, { vm.setPickerTab(PickerTab.FILM) })
        Chip("Cameras", s.pickerTab == PickerTab.CAMERA, { vm.setPickerTab(PickerTab.CAMERA) })
    }
    Spacer(Modifier.height(10.dp))
    val engine = (LocalContext.current.applicationContext as EmucamApp).engine
    if (s.pickerTab == PickerTab.FILM) {
        val films = remember(s.catalog) { s.catalog.films.sortedWith(compareBy({ categoryOrder(it.category) }, { it.name })) }
        val current = engine.filmId(s.recipe)
        LazyRow(contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            items(films, key = { it.id }) { f ->
                FilmCard(f.name, f.iso, f.note, parseHex(f.color), f.id == current, { vm.selectFilm(f.id) })
            }
        }
    } else {
        LazyRow(contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            items(s.catalog.bodies, key = { it.id }) { b ->
                FilmCard(b.name, null, b.note, parseHex(b.color), b.id == s.recipe.body, { vm.selectBody(b.id) })
            }
        }
    }
}

internal fun categoryOrder(c: String) = when (c) {
    "Infrared" -> 0
    "Color negative" -> 1
    "Slide" -> 2
    "Black & white" -> 3
    else -> 4
}

@Composable
private fun LensPanel(s: UiState, vm: ViewfinderViewModel) {
    val engine = (LocalContext.current.applicationContext as EmucamApp).engine
    val current = engine.lensId(s.recipe)
    LazyRow(contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        items(s.catalog.lenses.sortedBy { it.category + it.name }, key = { it.id }) { l ->
            FilmCard(l.name, null, "${l.focal ?: ""}mm ƒ/${l.fstop ?: ""} · ${l.category}\n${l.note}", parseHex(l.color), l.id == current, { vm.selectLens(l.id) })
        }
    }
    val lens = s.catalog.lens(current)
    if (lens?.optics?.blur == true) {
        Spacer(Modifier.height(10.dp))
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            val sel = s.recipe.params.apertureShape
            Chip("Default", sel == null, { vm.setShape(null) })
            Chip("Circle", sel == "circle", { vm.setShape("circle") })
            Chip("Hexagon", sel == "poly6", { vm.setShape("poly6") })
            s.catalog.shapes.forEach { id -> Chip(id, sel == id, { vm.setShape(id) }, accent = Emu.Signal) }
        }
    }
    lens?.controls?.forEach { c ->
        val v = if (c.key == "blur") s.recipe.params.blur else s.recipe.params.values[c.key] ?: c.default
        Spacer(Modifier.height(6.dp))
        DetentWheel(
            label = c.label, value = v, min = c.min, max = c.max,
            step = if (c.step > 0f) c.step else (c.max - c.min) / 40f, default = c.default,
            format = { if (c.step >= 1f) it.toInt().toString() else "%.2f".format(it) },
            onChange = { vm.setControl(c.key, it) },
            modifier = Modifier.padding(horizontal = 14.dp),
        )
    }
}

@Composable
private fun AdjustPanel(s: UiState, vm: ViewfinderViewModel) {
    val p = s.recipe.params
    Row(Modifier.padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        XYPad(p.warmth, p.punch, { x, y -> vm.setCharacter(x, y) })
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            DetentWheel("Exposure", p.exposure, -3f, 3f, 1f / 3f, 0f, { "%+.1f EV".format(it) }, { vm.setExposure(it) })
            DetentWheel("Grain", p.grain, 0f, 2f, 0.1f, 1f, { "%.1f×".format(it) }, { vm.setGrain(it) })
            DetentWheel("Halation", p.halation, 0f, 3f, 0.1f, 1f, { "%.1f×".format(it) }, { vm.setHalation(it) })
        }
    }
}

@Composable
private fun EffectsPanel(s: UiState, vm: ViewfinderViewModel) {
    val engine = (LocalContext.current.applicationContext as EmucamApp).engine
    val bundled = listOfNotNull(
        s.catalog.body(s.recipe.body),
        s.catalog.lens(engine.lensId(s.recipe)),
        s.catalog.film(engine.filmId(s.recipe)),
    ).flatMap { it.effects }.distinct()
    if (bundled.isNotEmpty()) {
        Text("BUILT IN", style = Emu.Label, modifier = Modifier.padding(horizontal = 14.dp))
        Spacer(Modifier.height(6.dp))
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            bundled.forEach { id -> Chip(id, id !in s.recipe.disabledEffects, { vm.toggleBundledEffect(id) }, accent = Emu.Amber) }
        }
        Spacer(Modifier.height(10.dp))
    }
    Text("ADD", style = Emu.Label, modifier = Modifier.padding(horizontal = 14.dp))
    Spacer(Modifier.height(6.dp))
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        USER_EFFECTS.forEach { (kind, label) ->
            val on = s.recipe.effects.any { it["id"]?.toString()?.trim('"') == "user.$kind" }
            Chip(label, on, { vm.toggleUserEffect(kind) }, accent = Emu.Signal)
        }
    }
    // placed effects: one dial each, to move them around the frame
    val placed = USER_EFFECTS.mapNotNull { (kind, label) ->
        s.recipe.effectPosition("user.$kind")?.takeIf { kind in PLACEABLE_EFFECTS }?.let { Triple(kind, label, it) }
    }
    if (placed.isNotEmpty()) {
        Spacer(Modifier.height(10.dp))
        Text("PLACE", style = Emu.Label, modifier = Modifier.padding(horizontal = 14.dp))
        Spacer(Modifier.height(6.dp))
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            placed.forEach { (kind, label, deg) ->
                PositionDial(deg, { vm.placeEffect(kind, it) }, label = label.uppercase(), dialSize = 60.dp)
            }
        }
    }
    Spacer(Modifier.height(10.dp))
    OmniSection(s, vm)
    Spacer(Modifier.height(6.dp))
    Text("Some effects (dust, scratches, flare, prism…) appear when the photo develops.", style = Emu.Body, modifier = Modifier.padding(horizontal = 14.dp))
}

/** OMNI-style filter: a crystal / wand in front of any lens, placed around the frame. */
@Composable
private fun OmniSection(s: UiState, vm: ViewfinderViewModel) {
    val o = s.recipe.omni()
    Text("OMNI FILTER", style = Emu.Label, modifier = Modifier.padding(horizontal = 14.dp))
    Spacer(Modifier.height(6.dp))
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Chip("Off", o == null, { vm.setOmni(null) }, accent = Emu.Signal)
        OMNI_WANDS.forEach { (id, label) ->
            Chip(label, o?.wand == id, { vm.setOmni((o ?: Omni()).copy(wand = id)) }, accent = Emu.Signal)
        }
    }
    if (o == null) return
    Spacer(Modifier.height(8.dp))
    Row(Modifier.padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        PositionDial(o.position, { vm.setOmni(o.copy(position = it)) }, label = "PLACE")
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            // DetentWheel keeps its first onChange while shown: update the live filter, not `o`
            DetentWheel("Reach", o.reach, 0.15f, 1f, 0.05f, 0.45f, { "%.0f%%".format(it * 100) }, { v -> vm.updateOmni { it.copy(reach = v) } })
            DetentWheel("Strength", o.strength, 0f, 1.5f, 0.05f, 1f, { "%.0f%%".format(it * 100) }, { v -> vm.updateOmni { it.copy(strength = v) } })
        }
    }
}

/** Zoom stops the phone can reach: ultra-wide (when offered), 1×, then tele steps. */
internal fun zoomStops(min: Float, max: Float): List<Float> =
    (listOfNotNull(min.takeIf { it < 0.95f }) + listOf(1f, 2f, 3f, 5f, 10f)).filter { it in min..max + 0.01f }

@Composable
private fun ZoomChips(zoom: Float, min: Float, max: Float, onZoom: (Float) -> Unit, modifier: Modifier = Modifier) {
    val stops = zoomStops(min, max)
    // the stop the current zoom belongs to (nearest in log space) shows the exact value
    val active = stops.minByOrNull { abs(kotlin.math.ln(zoom / it)) }
    Row(
        modifier.clip(RoundedCornerShape(50)).background(Emu.Ink.copy(alpha = 0.55f)).padding(4.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        stops.forEach { z ->
            val on = z == active
            val v = if (on) zoom else z
            val label = (if (v < 1f || v % 1f > 0.05f) "%.1f".format(v) else "%.0f".format(v)) + if (on) "×" else ""
            Box(
                Modifier.size(36.dp).clip(RoundedCornerShape(50))
                    .background(if (on) Emu.Bone.copy(alpha = 0.18f) else Color.Transparent)
                    .clickable { onZoom(z) },
                contentAlignment = Alignment.Center,
            ) {
                Text(label, style = Emu.Label.copy(color = if (on) Emu.Amber else Emu.Bone))
            }
        }
    }
}
