package com.emucam.develop

import android.content.Context
import android.util.Log
import com.emucam.depth.DepthEstimator
import com.emucam.model.Recipe
import com.emucam.nativebridge.EmuNative
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder

data class DevelopProgress(val shotId: String, val progress: Float)

/**
 * Develops shots one at a time in the Rust engine (memory-bound: a 12 MP
 * render needs several hundred MB). Jobs are idempotent — everything is read
 * from the ShotStore — so pending shots are simply re-queued on startup.
 */
class DevelopQueue(
    private val context: Context,
    private val store: ShotStore,
    private val describe: (Recipe) -> String,
    /** Capture depth network; null disables depth-driven defocus. */
    private val depth: DepthEstimator? = null,
    /** Segmentation network (PIDNet); null disables the vegetation mask for infrared films. */
    private val seg: DepthEstimator? = null,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val jobs = Channel<String>(Channel.UNLIMITED)

    private val _active = MutableStateFlow<DevelopProgress?>(null)
    /** Currently developing shot and its progress (null when idle). */
    val active: StateFlow<DevelopProgress?> = _active.asStateFlow()

    private val _shots = MutableStateFlow(store.all())
    val shots: StateFlow<List<Shot>> = _shots.asStateFlow()

    init {
        scope.launch { for (id in jobs) develop(id) }
        // Resume anything interrupted (app killed mid-develop).
        store.all().filter { it.state == ShotState.PENDING || it.state == ShotState.DEVELOPING }.forEach { jobs.trySend(it.id) }
    }

    fun enqueue(id: String) {
        refresh()
        jobs.trySend(id)
    }

    fun refresh() {
        _shots.value = store.all()
    }

    private suspend fun develop(id: String) {
        val shot = store.read(id) ?: return
        store.write(shot.copy(state = ShotState.DEVELOPING))
        refresh()
        _active.value = DevelopProgress(id, 0f)
        try {
            val recipe = store.recipe(id)
            val bytes = withContext(Dispatchers.IO) { store.original(id).readBytes() }
            val raw = depthFor(id, bytes, recipe)
            val veg = vegFor(id, bytes, recipe)
            val out = runNative(bytes, recipe, id, raw, veg)
            val developed = store.developed(id)
            withContext(Dispatchers.IO) {
                developed.writeBytes(out)
                ExifWriter.copy(store.original(id), developed, describe(recipe), recipe.toJson(), "emucam ${EmuNative.version()}")
            }
            val uri = withContext(Dispatchers.IO) { MediaStoreSaver.save(context, developed, shot.createdAt) }
            store.write(shot.copy(state = ShotState.DONE, uri = uri.toString(), error = null))
        } catch (t: Throwable) {
            Log.e(TAG, "develop $id failed", t)
            store.write(shot.copy(state = ShotState.FAILED, error = t.message))
        } finally {
            _active.value = null
            refresh()
        }
    }

    /**
     * Raw depth for this shot: the stored map if a previous develop made one,
     * otherwise run the network (only when the recipe's lens uses depth).
     * Any failure just means no depth — the lens falls back to its blur field.
     */
    private suspend fun depthFor(id: String, jpeg: ByteArray, recipe: Recipe): FloatArray? {
        val est = depth ?: return null
        val m = est.model
        return cachedInference(
            "depth", id, store.depth(id), m.outputSize,
            wants = { EmuNative.wantsDepth(recipe.toJson()) },
            input = { EmuNative.depthInput(jpeg, m.width, m.height, m.nchw) },
            run = est::run,
        )
    }

    /**
     * Raw segmentation logits for this shot, with the same caching and
     * failure rules as [depthFor]: only for infrared recipes, and any failure
     * just means no vegetation mask (the film develops colour-only).
     */
    private suspend fun vegFor(id: String, jpeg: ByteArray, recipe: Recipe): FloatArray? {
        val est = seg ?: return null
        val m = est.model
        return cachedInference(
            "veg", id, store.veg(id), m.outputSize,
            wants = { EmuNative.wantsVeg(recipe.toJson()) },
            input = { EmuNative.depthInput(jpeg, m.width, m.height, m.nchw) },
            run = est::run,
        )
    }

    private suspend fun runNative(
        bytes: ByteArray, recipe: Recipe, id: String, depthRaw: FloatArray?, vegRaw: FloatArray?,
    ): ByteArray {
        val m = depth?.model
        val sm = seg?.model
        val h = if (vegRaw != null && sm != null) {
            val dm = m.takeIf { depthRaw != null }
            EmuNative.developStartAux(
                bytes, recipe.toJson(), 0, JPEG_QUALITY,
                depthRaw.takeIf { dm != null }, dm?.width ?: 0, dm?.height ?: 0, dm?.kind ?: 0,
                vegRaw, sm.classes, sm.outW, sm.outH,
            )
        } else if (depthRaw != null && m != null) {
            EmuNative.developStartDepth(bytes, recipe.toJson(), 0, JPEG_QUALITY, depthRaw, m.width, m.height, m.kind)
        } else {
            EmuNative.developStart(bytes, recipe.toJson(), 0, JPEG_QUALITY)
        }
        try {
            while (scope.isActive) {
                when (EmuNative.developStatus(h)) {
                    EmuNative.STATUS_DONE -> return EmuNative.developResult(h)
                    EmuNative.STATUS_FAILED -> error(EmuNative.developError(h))
                    EmuNative.STATUS_CANCELLED -> error("cancelled")
                }
                _active.update { DevelopProgress(id, EmuNative.developProgress(h)) }
                delay(50)
            }
            EmuNative.developCancel(h)
            error("queue stopped")
        } finally {
            EmuNative.developFree(h)
        }
    }

    companion object {
        private const val TAG = "DevelopQueue"
        /** 95, not 92: film grain is fine texture that lower qualities smear. */
        private const val JPEG_QUALITY = 95

        /**
         * A network's raw output for one shot: the cached file if it has
         * exactly [outputSize] values, otherwise build the [input], [run] the
         * network and cache the result. Null when not [wants]ed or on any
         * failure (never throws, except for coroutine cancellation).
         */
        internal suspend fun cachedInference(
            label: String,
            id: String,
            file: File,
            outputSize: Int,
            wants: () -> Boolean,
            input: () -> FloatArray,
            run: suspend (FloatArray) -> FloatArray?,
        ): FloatArray? = try {
            if (!wants()) {
                null
            } else {
                val cached = withContext(Dispatchers.IO) { readFloats(file, outputSize) }
                if (cached != null) {
                    cached
                } else {
                    val t0 = System.nanoTime()
                    val out = run(input())
                    if (out == null) {
                        null
                    } else {
                        Log.i(TAG, "$label for $id in ${(System.nanoTime() - t0) / 1_000_000} ms")
                        // the cache is optional: a full disk must not cost this shot its map
                        withContext(Dispatchers.IO) { runCatching { writeFloats(file, out) } }
                            .onFailure { Log.w(TAG, "$label cache for $id not saved", it) }
                        out
                    }
                }
            }
        } catch (e: CancellationException) {
            throw e
        } catch (t: Throwable) {
            Log.w(TAG, "$label for $id failed; developing without", t)
            null
        }

        internal fun writeFloats(f: File, v: FloatArray) {
            val buf = ByteBuffer.allocate(v.size * 4).order(ByteOrder.LITTLE_ENDIAN)
            buf.asFloatBuffer().put(v)
            val tmp = File(f.parentFile, f.name + ".tmp")
            tmp.writeBytes(buf.array())
            // cache only: if it can't be stored, the next develop re-runs the network
            if (!tmp.renameTo(f)) tmp.delete()
        }

        /** Floats from [f], or null if missing or not exactly [count] values. */
        internal fun readFloats(f: File, count: Int): FloatArray? {
            if (!f.exists() || f.length() != count * 4L) return null
            val out = FloatArray(count)
            ByteBuffer.wrap(f.readBytes()).order(ByteOrder.LITTLE_ENDIAN).asFloatBuffer().get(out)
            return out
        }
    }
}
