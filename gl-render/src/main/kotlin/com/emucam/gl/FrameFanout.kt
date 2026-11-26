package com.emucam.gl

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Hands the live readback frame to several consumers (live depth, live
 * segmentation). Each consumer gets a [Tap] and asks for a frame with
 * [Tap.request]; the renderer captures one frame whenever any tap is
 * [wanted] and [deliver]s it only to the taps that asked, so a worker never
 * finds an unsolicited (older) frame waiting, and a request works whether or
 * not the other workers are running.
 *
 * Thread-safe: taps are added/removed and requests made from any thread;
 * [wanted]/[deliver] run on the GL thread. Sinks must not block (trySend to a
 * conflated channel) and must treat the shared array as read-only.
 */
class FrameFanout {
    inner class Tap internal constructor(internal val sink: (ByteArray) -> Unit) {
        internal val pending = AtomicBoolean(false)

        /** Ask for one frame on the next render. */
        fun request() = pending.set(true)

        /** Stop receiving frames. Idempotent. */
        fun remove() {
            taps.remove(this)
        }
    }

    private val taps = CopyOnWriteArrayList<Tap>()

    fun add(sink: (ByteArray) -> Unit): Tap = Tap(sink).also { taps.add(it) }

    /** Whether some tap is waiting for a frame. */
    val wanted: Boolean get() = taps.any { it.pending.get() }

    /** Give [frame] to every tap that requested one (clearing its request). */
    fun deliver(frame: ByteArray) {
        for (t in taps) if (t.pending.getAndSet(false)) t.sink(frame)
    }
}
