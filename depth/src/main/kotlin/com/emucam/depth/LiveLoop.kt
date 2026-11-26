package com.emucam.depth

import kotlinx.coroutines.channels.ReceiveChannel
import kotlinx.coroutines.delay

/**
 * Request → frame → infer cycle of the live viewfinder depth, at most one
 * cycle per [periodMs]. [request] asks the renderer for one frame, which
 * arrives on [frames]; [process] handles it and returns false to stop for good.
 *
 * There is deliberately no receive timeout: a request stays pending in the
 * renderer until the next drawn frame, so while the camera is unbound (app in
 * the background) this suspends at zero cost instead of polling.
 */
suspend fun liveDepthLoop(
    frames: ReceiveChannel<ByteArray>,
    request: () -> Unit,
    periodMs: Long,
    process: suspend (ByteArray) -> Boolean,
) = liveDepthLoop(frames, request, { periodMs }, process)

/** Like the fixed-period loop, re-reading [periodMs] every cycle (thermal throttling). */
suspend fun liveDepthLoop(
    frames: ReceiveChannel<ByteArray>,
    request: () -> Unit,
    periodMs: () -> Long,
    process: suspend (ByteArray) -> Boolean,
) {
    // a frame left over from a previous (cancelled) run shows an old scene/rotation
    frames.tryReceive()
    while (true) {
        val started = System.nanoTime()
        request()
        val frame = frames.receive()
        if (!process(frame)) return
        val spent = (System.nanoTime() - started) / 1_000_000
        delay((periodMs() - spent).coerceAtLeast(10))
    }
}

/**
 * Live-network period for an Android thermal status (PowerManager
 * THERMAL_STATUS_*: 0 none, 1 light, 2 moderate, 3 severe, 4+ critical…).
 * Viewfinder depth is a nicety: back off as the phone heats up so the
 * camera itself stays usable, down to one map every 2 s.
 */
fun thermalPeriod(baseMs: Long, status: Int): Long = when {
    status <= 0 -> baseMs
    status == 1 -> baseMs * 2
    status == 2 -> baseMs * 4
    else -> maxOf(baseMs * 8, 2_000L)
}
