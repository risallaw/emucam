package com.emucam.gl

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameFanoutTest {
    @Test
    fun onlyRequestingTapsReceiveAFrame() {
        val f = FrameFanout()
        val a = mutableListOf<ByteArray>()
        val b = mutableListOf<ByteArray>()
        val ta = f.add { a += it }
        f.add { b += it }
        assertFalse(f.wanted)
        ta.request()
        assertTrue(f.wanted)
        val frame = ByteArray(1)
        f.deliver(frame)
        assertEquals(listOf(frame), a)
        assertEquals(emptyList<ByteArray>(), b)
        // the request is consumed: no frame until asked again
        assertFalse(f.wanted)
        f.deliver(ByteArray(1))
        assertEquals(1, a.size)
    }

    @Test
    fun bothTapsShareOneCapture() {
        val f = FrameFanout()
        var n = 0
        val ta = f.add { n++ }
        val tb = f.add { n++ }
        ta.request(); tb.request()
        f.deliver(ByteArray(1))
        assertEquals(2, n)
    }

    @Test
    fun removedTapNeitherWantsNorReceives() {
        val f = FrameFanout()
        var got = 0
        val t = f.add { got++ }
        t.request()
        t.remove()
        assertFalse(f.wanted)
        f.deliver(ByteArray(1))
        assertEquals(0, got)
    }
}
