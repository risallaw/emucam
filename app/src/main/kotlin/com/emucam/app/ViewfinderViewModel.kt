package com.emucam.app

import android.content.Context
import android.util.Log
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import androidx.camera.view.PreviewView
import com.emucam.camera.CameraController
import com.emucam.camera.CameraState
import com.emucam.develop.DevelopProgress
import com.emucam.develop.Shot
import com.emucam.gl.LiveParams
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
import kotlin.random.Random

enum class Panel { NONE, FILM, LENS, ADJUST, EFFECTS }
enum class PickerTab { FILM, CAMERA }

data class UiState(
    val catalog: Catalog,
    val recipe: Recipe,
    val panel: Panel = Panel.NONE,
    val pickerTab: PickerTab = PickerTab.FILM,
    val camera: CameraState = CameraState(),
    val developing: DevelopProgress? = null,
    val shots: List<Shot> = emptyList(),
    val showGallery: Boolean = false,
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

class ViewfinderViewModel(private val app: EmucamApp) : ViewModel() {
    private val engine = app.engine
    val camera = CameraController(app)
    private val prefs = app.getSharedPreferences("emucam", Context.MODE_PRIVATE)

    private val recipe = MutableStateFlow(loadRecipe())
    private val ui = MutableStateFlow(UiState(engine.catalog, recipe.value))

    val state: StateFlow<UiState> = combine(ui, recipe, camera.state, app.queue.active, app.queue.shots) { u, r, c, d, s ->
        u.copy(recipe = r, camera = c, developing = d, shots = s)
    }.stateIn(viewModelScope, SharingStarted.Eagerly, ui.value)

    init {
        // Recipe → GPU preview config (Rust bakes LUTs/uniforms), debounced.
        @OptIn(FlowPreview::class)
        viewModelScope.launch {
            recipe.debounce(40).distinctUntilChanged()
                .map { runCatching { engine.previewConfig(it) }.onFailure { e -> Log.e(TAG, "preview config", e) }.getOrNull() }
                .flowOn(Dispatchers.Default)
                .collect { cfg -> if (cfg != null) camera.processor.setConfig(cfg) }
        }
        viewModelScope.launch {
            recipe.debounce(500).collect { prefs.edit().putString(KEY_RECIPE, it.toJson()).apply() }
        }
    }

    private fun loadRecipe(): Recipe =
        prefs.getString(KEY_RECIPE, null)?.let { runCatching { Recipe.fromJson(it) }.getOrNull() }
            ?: Recipe(film = "portrait-400", lens = "standard")

    fun bindCamera(owner: LifecycleOwner, view: PreviewView) = camera.bind(owner, view)

    // ------------------------------------------------------------ selection

    fun selectFilm(id: String) = recipe.update { it.copy(film = id, body = null, lens = it.lens ?: engine.lensId(it)) }
    fun selectLens(id: String) = recipe.update {
        it.copy(lens = id, film = it.film ?: engine.filmId(it), body = null, params = it.params.copy(values = emptyMap(), blur = 1f))
    }
    fun selectBody(id: String) = recipe.update { it.copy(body = id, film = null, lens = null, disabledEffects = emptyList()) }

    fun setPanel(p: Panel) = ui.update { it.copy(panel = if (it.panel == p) Panel.NONE else p) }
    fun setPickerTab(t: PickerTab) = ui.update { it.copy(pickerTab = t) }
    fun showGallery(show: Boolean) = ui.update { it.copy(showGallery = show) }
    fun dismissMessage() = ui.update { it.copy(message = null) }

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
        })
    }

    fun userEffectOn(kind: String) = recipe.value.effects.any { (it["id"] as? JsonPrimitive)?.content == "user.$kind" }

    private fun dateText(): String {
        val c = java.util.Calendar.getInstance()
        return "'%02d %d %d".format(c.get(java.util.Calendar.YEAR) % 100, c.get(java.util.Calendar.MONTH) + 1, c.get(java.util.Calendar.DAY_OF_MONTH))
    }

    // -------------------------------------------------------------- capture

    fun shutter() {
        viewModelScope.launch {
            val (shot, file) = app.store.newShot()
            val r = recipe.value.copy(seed = Random.nextLong(0, 1L shl 32))
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

    /** Re-develop an existing shot's original with the current film/lens. */
    fun redevelop(shotId: String) {
        viewModelScope.launch(Dispatchers.IO) {
            val src: File = app.store.original(shotId)
            if (!src.exists()) return@launch
            val (shot, file) = app.store.newShot()
            src.copyTo(file, overwrite = true)
            val r = recipe.value.copy(seed = Random.nextLong(0, 1L shl 32))
            app.store.saveRecipe(shot.id, r)
            app.queue.enqueue(shot.id)
            ui.update { it.copy(message = "Re-developing with ${engine.describe(r)}") }
        }
    }

    fun deleteShot(id: String) {
        app.store.delete(id)
        app.queue.refresh()
    }

    override fun onCleared() {
        camera.release()
    }

    companion object {
        private const val TAG = "Viewfinder"
        private const val KEY_RECIPE = "recipe"

        fun factory(app: EmucamApp) = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = ViewfinderViewModel(app) as T
        }
    }
}
