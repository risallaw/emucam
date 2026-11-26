package com.emucam.app

import android.content.Context
import android.util.Log
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import androidx.camera.view.PreviewView
import androidx.core.content.edit
import com.emucam.camera.CameraController
import com.emucam.camera.CameraState
import com.emucam.develop.DevelopProgress
import com.emucam.develop.OutputFormat
import com.emucam.develop.Shot
import com.emucam.gl.LiveParams
import com.emucam.model.Grid
import com.emucam.model.Omni
import com.emucam.model.PLACEABLE_EFFECTS
import com.emucam.model.withEffectPosition
import com.emucam.model.forCaptureRotation
import com.emucam.model.withOmni
import com.emucam.model.updateOmni
import com.emucam.model.redevelopWith
import com.emucam.model.Catalog
import com.emucam.model.Recipe
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.io.IOException
import kotlin.random.Random

enum class Panel { NONE, FILM, LENS, ADJUST, EFFECTS }
enum class PickerTab { FILM, CAMERA }

data class UiState(
    val catalog: Catalog,
    val recipe: Recipe,
    val panel: Panel = Panel.NONE,
    /** Composition guide over the viewfinder, and its orientation (0..3). */
    val grid: Grid = Grid.OFF,
    val gridOrient: Int = 0,
    val pickerTab: PickerTab = PickerTab.FILM,
    val camera: CameraState = CameraState(),
    val developing: DevelopProgress? = null,
    val shots: List<Shot> = emptyList(),
    val showGallery: Boolean = false,
    /** Container for newly developed photos (persisted). */
    val outputFormat: OutputFormat = OutputFormat.DEFAULT,
    val flash: Boolean = false,
    val message: String? = null,
)

/** User-addable creative effects (kind, label). Rust validates/ignores unknown kinds. */
val USER_EFFECTS = listOf(
    "light_leak" to "Light leak",
    "date_stamp" to "Date stamp",
    "frame" to "Frame",
    "dust" to "Dust",
    "scratches" to "Scratches",
    "flare" to "Flare",
    "prism" to "Prism",
    "haze" to "Haze",
    "fog" to "Edge fog",
    "color_shift" to "Expired",
    "toning" to "Sepia tone",
    "vignette" to "Vignette",
)

@OptIn(FlowPreview::class) // debounce
class ViewfinderViewModel(private val app: EmucamApp) : ViewModel() {
    private val engine = app.engine
    val camera = CameraController(app)
    private val prefs = app.getSharedPreferences("emucam", Context.MODE_PRIVATE)

    private val liveDepth = LiveDepth(app, camera.processor, viewModelScope) { camera.deviceRotation }
    private val liveSeg = LiveSeg(app, camera.processor, viewModelScope) { camera.deviceRotation }

    private val recipe = MutableStateFlow(loadRecipe())
    private val ui = MutableStateFlow(
        UiState(
            engine.catalog, recipe.value,
            outputFormat = OutputFormat.fromName(prefs.getString(KEY_FORMAT, null)),
            grid = Grid.fromName(prefs.getString(KEY_GRID, null)),
            gridOrient = prefs.getInt(KEY_GRID_ORIENT, 0),
        ),
    )

    val state: StateFlow<UiState> = combine(ui, recipe, camera.state, app.queue.active, app.queue.shots) { u, r, c, d, s ->
        u.copy(recipe = r, camera = c, developing = d, shots = s)
    }.stateIn(viewModelScope, SharingStarted.Eagerly, ui.value)

    init {
        // Recipe → GPU preview config (Rust bakes LUTs/uniforms), debounced.
        viewModelScope.launch {
            recipe.debounce(40).distinctUntilChanged()
                .map { runCatching { engine.previewConfig(it) }.onFailure { e -> Log.e(TAG, "preview config", e) }.getOrNull() }
                .flowOn(Dispatchers.Default)
                .collect { cfg ->
                    if (cfg != null) {
                        camera.processor.setConfig(cfg)
                        // Rust decides whether this lens/recipe uses depth
                        liveDepth.setEnabled((cfg.uniforms["uDepthParams"]?.getOrNull(2) ?: 0f) > 0.5f)
                        // the IR define is set exactly when pipeline::wants_veg (infrared film)
                        liveSeg.setEnabled("IR" in cfg.defines)
                    }
                }
        }
        viewModelScope.launch {
            recipe.debounce(500).collect { prefs.edit { putString(KEY_RECIPE, it.toJson()) } }
        }
        // A new lens frames like its focal length: CameraX picks the physical
        // camera (ultra-wide / main / tele) for that zoom. The zoom chips
        // override it until the lens changes again.
        viewModelScope.launch {
            // keyed on the lens id, not the focal: two 35 mm lenses are still a lens change
            recipe.map { engine.lensId(it) }.distinctUntilChanged().collect { id ->
                val f = engine.catalog.lens(id)?.focal
                if (f != null && f > 0) camera.matchFocal(f)
            }
        }
    }

    private fun loadRecipe(): Recipe =
        (prefs.getString(KEY_RECIPE, null)?.let { runCatching { Recipe.fromJson(it) }.getOrNull() }
            ?: Recipe(film = "portrait-400", lens = "standard")).let { it.withSeedIfUnset() }

    fun bindCamera(owner: LifecycleOwner, view: PreviewView) = camera.bind(owner, view)

    fun setZoom(ratio: Float) = camera.setZoom(ratio)

    // ------------------------------------------------------------ selection

    fun selectFilm(id: String) = recipe.update { it.copy(film = id, body = null, lens = it.lens ?: engine.lensId(it)) }
    fun selectLens(id: String) = recipe.update {
        it.copy(lens = id, film = it.film ?: engine.filmId(it), body = null, params = it.params.copy(values = emptyMap(), blur = 1f))
    }
    fun selectBody(id: String) = recipe.update { it.copy(body = id, film = null, lens = null, disabledEffects = emptyList()) }

    fun setPanel(p: Panel) = ui.update { it.copy(panel = if (it.panel == p) Panel.NONE else p) }
    fun setPickerTab(t: PickerTab) = ui.update { it.copy(pickerTab = t) }
    fun showGallery(show: Boolean) = ui.update { it.copy(showGallery = show) }
    /** Next composition guide (off → thirds → … → off); names it briefly. */
    fun cycleGrid() {
        val g = ui.value.grid.next()
        ui.update { it.copy(grid = g, message = g.label) }
        prefs.edit { putString(KEY_GRID, g.name) }
    }

    /** Turn the spiral / triangle so its focus lands on another corner. */
    fun rotateGrid() {
        val o = (ui.value.gridOrient + 1) % 4
        ui.update { it.copy(gridOrient = o) }
        prefs.edit { putInt(KEY_GRID_ORIENT, o) }
    }

    fun dismissMessage() = ui.update { it.copy(message = null) }

    fun setOutputFormat(f: OutputFormat) {
        ui.update { it.copy(outputFormat = f) }
        prefs.edit { putString(KEY_FORMAT, f.name) }
    }

    // ---------------------------------------------------------- adjustments

    fun setExposure(v: Float) = recipe.update { it.copy(params = it.params.copy(exposure = v)) }
    fun setGrain(v: Float) = recipe.update { it.copy(params = it.params.copy(grain = v)) }
    fun setHalation(v: Float) = recipe.update { it.copy(params = it.params.copy(halation = v)) }
    fun setCharacter(x: Float, y: Float) = recipe.update { it.copy(params = it.params.copy(warmth = x, punch = y)) }
    fun setControl(key: String, v: Float) = recipe.update {
        if (key == "blur") it.copy(params = it.params.copy(blur = v))
        else it.copy(params = it.params.copy(values = it.params.values + (key to v)))
    }
    fun setShape(id: String?) = recipe.update { it.copy(params = it.params.copy(apertureShape = id)) }

    fun moveFocus(nx: Float, ny: Float) {
        recipe.update { it.copy(params = it.params.copy(focusCenter = listOf(nx, ny))) }
        camera.processor.setLive(LiveParams(focusCenter = floatArrayOf(nx, ny), focusAngle = Math.toRadians(recipe.value.params.focusAngle.toDouble()).toFloat()))
    }
    fun scaleFocus(zoom: Float) = recipe.update {
        it.copy(params = it.params.copy(focusSize = (it.params.focusSize * zoom).coerceIn(0.25f, 4f)))
    }
    fun rotateFocus(deg: Float) {
        recipe.update { it.copy(params = it.params.copy(focusAngle = it.params.focusAngle + deg)) }
        val c = recipe.value.params.focusCenter ?: listOf(0.5f, 0.5f)
        camera.processor.setLive(LiveParams(focusCenter = c.toFloatArray(), focusAngle = Math.toRadians(recipe.value.params.focusAngle.toDouble()).toFloat()))
    }
    fun focusGestureEnd() {
        val c = recipe.value.params.focusCenter ?: listOf(0.5f, 0.5f)
        camera.focusAt(c[0], c[1])
        camera.processor.setLive(LiveParams())
    }
    /** Focus point back to the centre at default size and angle; camera back to continuous AF. */
    fun resetFocus() {
        recipe.update { it.copy(params = it.params.copy(focusCenter = null, focusSize = 1f, focusAngle = 0f)) }
        camera.processor.setLive(LiveParams())
        camera.resetFocus()
        ui.update { it.copy(message = "Focus reset") }
    }

    fun tapFocus(nx: Float, ny: Float) {
        moveFocus(nx, ny)
        focusGestureEnd()
    }

    fun toggleBundledEffect(id: String) = recipe.update {
        val d = it.disabledEffects
        it.copy(disabledEffects = if (id in d) d - id else d + id)
    }

    fun toggleUserEffect(kind: String) = recipe.update { r ->
        val id = "user.$kind"
        val exists = r.effects.any { (it["id"] as? JsonPrimitive)?.content == id }
        if (exists) r.copy(effects = r.effects.filterNot { (it["id"] as? JsonPrimitive)?.content == id })
        else r.copy(effects = r.effects + buildJsonObject {
            put("kind", kind); put("id", id); put("strength", 1.0)
            if (kind == "date_stamp") put("params", buildJsonObject { put("text", dateText()) })
            PLACEABLE_EFFECTS[kind]?.let { deg -> put("params", buildJsonObject { put("position", deg) }) }
        })
    }

    /** Move a placed user effect (light leak, fog, prism) around the frame. */
    fun placeEffect(kind: String, deg: Float) = recipe.update { it.withEffectPosition("user.$kind", deg) }

    /** Set, change or (null) remove the OMNI filter. */
    fun setOmni(o: Omni?) = recipe.update { it.withOmni(o) }

    /** Change one OMNI setting on the current filter (safe from stale UI callbacks). */
    fun updateOmni(f: (Omni) -> Omni) = recipe.update { it.updateOmni(f) }

    fun userEffectOn(kind: String) = recipe.value.effects.any { (it["id"] as? JsonPrimitive)?.content == "user.$kind" }

    private fun dateText(): String {
        val c = java.util.Calendar.getInstance()
        return "'%02d %d %d".format(c.get(java.util.Calendar.YEAR) % 100, c.get(java.util.Calendar.MONTH) + 1, c.get(java.util.Calendar.DAY_OF_MONTH))
    }

    // -------------------------------------------------------------- capture

    fun shutter() {
        viewModelScope.launch {
            val (shot, file) = app.store.newShot(ui.value.outputFormat)
            val (r, next) = nextShot(recipe.value, camera.deviceRotation)
            recipe.value = next
            app.store.saveRecipe(shot.id, r)
            ui.update { it.copy(flash = true) }
            try {
                camera.takePicture(file)
                app.queue.enqueue(shot.id)
            } catch (t: Throwable) {
                Log.e(TAG, "capture failed", t)
                app.store.delete(shot.id)
                ui.update { it.copy(message = "Capture failed: ${t.message}") }
            } finally {
                ui.update { it.copy(flash = false) }
            }
        }
    }

    /**
     * Re-develop an existing shot's original as a new frame with [look]
     * (null = the viewfinder's current look) and the current output format.
     * The shot's own focus geometry is kept. [onCreated] gets the new id on
     * the main thread.
     */
    fun redevelop(shotId: String, look: Recipe? = null, onCreated: (String) -> Unit = {}) {
        viewModelScope.launch(Dispatchers.IO) {
            val src: File = app.store.original(shotId)
            if (!src.exists()) return@launch
            val (shot, file) = app.store.newShot(ui.value.outputFormat)
            try {
                src.copyTo(file, overwrite = true)
            } catch (e: IOException) {
                // e.g. disk full: an uncaught exception here would crash the app (viewModelScope)
                app.store.delete(shot.id)
                ui.update { it.copy(message = "Re-develop failed: ${e.message}") }
                return@launch
            }
            // same photo → same depth / vegetation maps; saves re-running the networks.
            // The caches are optional: a failed copy just means they're recomputed.
            for (cache in listOf(app.store::depth, app.store::veg)) {
                runCatching { cache(shotId).takeIf { it.exists() }?.copyTo(cache(shot.id), overwrite = true) }
            }
            val base = look ?: recipe.value
            val withSeed = base.copy(seed = Random.nextLong(0, 1L shl 32))
            // keep the shot's own focus point; the viewfinder's belongs to another frame
            val r = runCatching { app.store.recipe(shotId) }.getOrNull()?.redevelopWith(withSeed) ?: withSeed
            app.store.saveRecipe(shot.id, r)
            app.queue.enqueue(shot.id)
            ui.update { it.copy(message = "Developing a new frame with ${engine.describe(r)}") }
            launch(Dispatchers.Main) { onCreated(shot.id) }
        }
    }

    /** Develop a failed shot again with its own recipe. */
    fun retry(id: String) {
        viewModelScope.launch(Dispatchers.IO) { app.queue.retry(id) }
    }

    fun deleteShot(id: String) {
        viewModelScope.launch(Dispatchers.IO) {
            if (!app.queue.delete(id)) {
                ui.update { it.copy(message = "Removed from the contact sheet; the photo stays in your gallery.") }
            }
        }
    }

    override fun onCleared() {
        liveSeg.close()
        liveDepth.close()
        camera.release()
    }

    companion object {
        private const val TAG = "Viewfinder"
        private const val KEY_RECIPE = "recipe"
        private const val KEY_FORMAT = "output_format"
        private const val KEY_GRID = "grid"
        private const val KEY_GRID_ORIENT = "grid_orient"

        fun factory(app: EmucamApp) = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = ViewfinderViewModel(app) as T
        }
    }
}

private fun rollSeed(): Long = Random.nextLong(0, 1L shl 32)

/** The viewfinder recipe always carries the seed of the next shot, so the preview shows what that shot will develop with. */
internal fun Recipe.withSeedIfUnset(roll: () -> Long = ::rollSeed): Recipe = if (seed == 0L) copy(seed = roll()) else this

/** (recipe to save for the shot taken now, live recipe for the following shot). */
internal fun nextShot(live: Recipe, rotation: Int, roll: () -> Long = ::rollSeed): Pair<Recipe, Recipe> =
    live.forCaptureRotation(rotation) to live.copy(seed = roll())
