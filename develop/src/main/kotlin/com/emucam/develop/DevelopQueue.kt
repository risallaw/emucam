package com.emucam.develop

import android.content.Context
import android.util.Log
import com.emucam.model.Recipe
import com.emucam.nativebridge.EmuNative
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
            val out = runNative(bytes, recipe, id)
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

    private suspend fun runNative(bytes: ByteArray, recipe: Recipe, id: String): ByteArray {
        val h = EmuNative.developStart(bytes, recipe.toJson(), 0, 92)
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
    }
}
