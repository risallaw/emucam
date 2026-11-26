package com.emucam.app

import android.content.Context
import android.util.Log
import com.emucam.depth.DepthEstimator
import com.emucam.depth.DepthModel
import com.emucam.depth.PreviewTensor
import com.emucam.depth.liveDepthLoop
import com.emucam.depth.thermalPeriod
import com.emucam.gl.DepthFrame
import com.emucam.gl.EmuSurfaceProcessor
import com.emucam.gl.LIVE_DEPTH_SIZE
import com.emucam.nativebridge.EmuNative
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.launch

/**
 * Live depth for the viewfinder: asks the GL preview for a small frame, runs
 * the fast preview network, and hands the nearness map back to the renderer
 * (~5 maps/s, fewer as the phone heats up). Runs only while [setEnabled] is true (the current lens uses
 * depth), and turns itself off for good if the network can't run here.
 */
class LiveDepth(
    context: Context,
    private val processor: EmuSurfaceProcessor,
    private val scope: CoroutineScope,
    /** Current device rotation (Surface.ROTATION_*). */
    private val rotation: () -> Int,
) {
    private val estimator = DepthEstimator(context, DepthModel.PREVIEW, idleMillis = 10_000)
    private val power = context.getSystemService(android.os.PowerManager::class.java)
    private val frames = Channel<ByteArray>(Channel.CONFLATED)
    private val tap = processor.frames.add { frames.trySend(it) }

    /** Guards [job] and every setDepth, so a cancelled loop can't publish after a disable cleared the map. */
    private val lock = Any()
    private var job: Job? = null
    private var version = 0L

    init {
        require(DepthModel.PREVIEW.width == LIVE_DEPTH_SIZE)
    }

    fun setEnabled(on: Boolean) {
        synchronized(lock) { update(on) }
    }

    private fun update(on: Boolean) {
        if (on && estimator.unavailable == null) {
            // tensor packing + normalisation are CPU work: keep them off the main thread
            if (job?.isActive != true) job = scope.launch(Dispatchers.Default) { loop() }
        } else {
            job?.cancel()
            job = null
            processor.setDepth(null)
        }
    }

    /** Run [f] (a setDepth) only if this coroutine is still the enabled loop. */
    private suspend fun publish(f: () -> Unit) {
        val me = currentCoroutineContext()[Job]
        synchronized(lock) { if (me != null && me === job && me.isActive) f() }
    }

    private suspend fun loop() = liveDepthLoop(frames, tap::request, { thermalPeriod(PERIOD_MS, power.currentThermalStatus) }) { rgba ->
        val n = LIVE_DEPTH_SIZE
        val m = DepthModel.PREVIEW
        val rot = rotation()
        val raw = estimator.run(PreviewTensor.toTensor(rgba, n, rot))
        if (raw == null) {
            // null = this frame failed; [unavailable] = the network can't run here at all
            val gone = estimator.unavailable
            if (gone != null) {
                Log.w(TAG, "live depth unavailable: $gone")
                publish { processor.setDepth(null) }
            }
            return@liveDepthLoop gone == null
        }
        val near = try {
            EmuNative.nearness(raw, m.width, m.height, m.kind)
        } catch (e: RuntimeException) {
            Log.w(TAG, "nearness failed", e) // an exception here would crash the app (viewModelScope)
            null
        }
        // publish() won't overwrite the setDepth(null) of a setEnabled(false) that cancelled us meanwhile
        if (near != null) {
            val frame = DepthFrame(PreviewTensor.toGlBytes(near, n, rot), n, ++version)
            publish { processor.setDepth(frame) }
        }
        true
    }

    fun close() {
        setEnabled(false)
        tap.remove()
        estimator.close()
    }

    private companion object {
        const val TAG = "LiveDepth"
        // 5 maps/s: enough to follow a moving subject; 10/s kept the GPU busy
        // and, with the camera, heated the phone over a few minutes
        const val PERIOD_MS = 200L
    }
}
