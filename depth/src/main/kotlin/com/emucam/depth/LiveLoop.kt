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
) {
    // a frame left over from a previous (cancelled) run shows an old scene/rotation
    frames.tryReceive()
    while (true) {
        val started = System.nanoTime()
        request()
        val frame = frames.receive()
        if (!process(frame)) return
        val spent = (System.nanoTime() - started) / 1_000_000
        delay((periodMs - spent).coerceAtLeast(10))
    }
}
