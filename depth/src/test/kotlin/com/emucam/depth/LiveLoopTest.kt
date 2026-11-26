package com.emucam.depth

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

class LiveLoopTest {
    @Test
    fun withoutFramesItRequestsOnceAndSleeps() = runBlocking {
        val frames = Channel<ByteArray>(Channel.CONFLATED)
        var requests = 0
        val job = launch { liveDepthLoop(frames, { requests++ }, 10) { true } }
        delay(300) // camera unbound: nothing is ever rendered
        job.cancel()
        assertEquals(1, requests)
    }

    @Test
    fun staleFrameFromAnEarlierRunIsDropped() = runBlocking {
        val frames = Channel<ByteArray>(Channel.CONFLATED)
        frames.trySend(ByteArray(1))
        val fresh = ByteArray(2)
        var got: ByteArray? = null
        // the renderer answers a request on a later frame, not synchronously
        liveDepthLoop(frames, { launch { delay(20); frames.trySend(fresh) } }, 10) { got = it; false }
        assertSame(fresh, got)
    }

    @Test
    fun processReturningFalseStops() = runBlocking {
        val frames = Channel<ByteArray>(Channel.CONFLATED)
        var n = 0
        liveDepthLoop(frames, { frames.trySend(ByteArray(0)) }, 1) { ++n < 3 }
        assertEquals(3, n)
    }
}
