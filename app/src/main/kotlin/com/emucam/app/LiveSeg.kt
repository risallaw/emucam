package com.emucam.app

import android.content.Context
import android.util.Log
import com.emucam.depth.DepthEstimator
import com.emucam.depth.DepthModel
import com.emucam.depth.PreviewTensor
import com.emucam.depth.UprightUpscaler
import com.emucam.depth.liveDepthLoop
import com.emucam.depth.thermalPeriod
import com.emucam.gl.EmuSurfaceProcessor
import com.emucam.gl.LIVE_DEPTH_SIZE
import com.emucam.gl.LIVE_VEG_SIZE
import com.emucam.gl.VegFrame
import com.emucam.nativebridge.EmuNative
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.launch

/**
 * Live segmentation for the infrared viewfinder: every ~0.5 s takes the live
 * readback frame, runs PIDNet-S (the same network the develop step uses) and
 * hands the vegetation probability map to the renderer, which gates the IR
 * foliage masks with it exactly like `infrared::gate_by_seg` does for the
 * saved photo. Runs only while [setEnabled] is true (the film is infrared);
 * any failure falls back to colour-only masks (`setVeg(null)`), and it turns
 * itself off for good if the network can't run here.
 */
class LiveSeg(
    context: Context,
    private val processor: EmuSurfaceProcessor,
    private val scope: CoroutineScope,
    /** Current device rotation (Surface.ROTATION_*). */
    private val rotation: () -> Int,
) {
    private val model = DepthModel.SEGMENT
    private val estimator = DepthEstimator(context, model, idleMillis = 10_000)
    private val power = context.getSystemService(android.os.PowerManager::class.java)
    private val frames = Channel<ByteArray>(Channel.CONFLATED)
    private val tap = processor.frames.add { frames.trySend(it) }

    /** Guards [job] and every setVeg, so a cancelled loop can't publish after a disable cleared the map. */
    private val lock = Any()
    private var job: Job? = null
    private var version = 0L

    init {
        require(model.outW == LIVE_VEG_SIZE && model.outH == LIVE_VEG_SIZE)
    }

    fun setEnabled(on: Boolean) {
        synchronized(lock) { update(on) }
    }

    private fun update(on: Boolean) {
        if (on && estimator.unavailable == null) {
            // 1024² tensor building (~3M floats) is CPU work: keep it off the main thread
            if (job?.isActive != true) {
                // publish() can't see the old/null job: it waits for this lock, held until job is set
                job = scope.launch(Dispatchers.Default) { loop() }
            }
        } else {
            job?.cancel()
            job = null
            processor.setVeg(null)
        }
    }

    /** Run [f] (a setVeg) only if this coroutine is still the enabled loop. */
    private suspend fun publish(f: () -> Unit) {
        val me = currentCoroutineContext()[Job]
        synchronized(lock) { if (me != null && me === job && me.isActive) f() }
    }

    private suspend fun loop() {
        // buffers live only while enabled (the tensor alone is 12 MB)
        val up = UprightUpscaler(LIVE_DEPTH_SIZE, model.width)
        liveDepthLoop(frames, tap::request, { thermalPeriod(PERIOD_MS, power.currentThermalStatus) }) { rgba -> step(up, rgba) }
    }

    /** One frame → vegetation map. Never throws (except cancellation); false = stop for good. */
    private suspend fun step(up: UprightUpscaler, rgba: ByteArray): Boolean {
        val rot = rotation()
        val prob = try {
            val raw = estimator.run(up.tensor(rgba, rot))
            if (raw == null) {
                // null = this frame failed; [unavailable] = the network can't run here at all
                val gone = estimator.unavailable
                publish { processor.setVeg(null) }
                if (gone != null) {
                    Log.w(TAG, "live segmentation unavailable: $gone")
                    return false
                }
                return true
            }
            EmuNative.vegetation(raw, model.classes, model.outW, model.outH)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // an exception here would crash the app (viewModelScope)
            Log.w(TAG, "live segmentation failed", e)
            null
        } catch (e: LinkageError) {
            Log.w(TAG, "live segmentation: native library mismatch", e)
            publish { processor.setVeg(null) }
            return false
        }
        val frame = prob?.let { VegFrame(PreviewTensor.toGlBytes(it, LIVE_VEG_SIZE, rot), LIVE_VEG_SIZE, ++version) }
        publish { processor.setVeg(frame) } // null → colour-only until the next good map
        return true
    }

    fun close() {
        setEnabled(false)
        tap.remove()
        estimator.close()
    }

    private companion object {
        const val TAG = "LiveSeg"
        const val PERIOD_MS = 500L
    }
}
