package com.emucam.depth

import org.junit.Assert.assertEquals
import org.junit.Test

class PreviewTensorTest {
    private val n = 8

    /** RGBA readback (GL rows) with one white pixel at screen (x, y), y down. */
    private fun readbackWithDot(x: Int, y: Int) = ByteArray(n * n * 4).also {
        val i = ((n - 1 - y) * n + x) * 4
        for (c in 0..2) it[i + c] = 255.toByte()
    }

    private fun brightest(t: FloatArray, stride: Int) =
        (0 until n * n).maxBy { t[it * stride] }.let { it % n to it / n }

    @Test
    fun uprightMappingFollowsDeviceRotation() {
        // dot near the screen's top-right corner
        val rb = readbackWithDot(6, 1)
        assertEquals(6 to 1, brightest(PreviewTensor.toTensor(rb, n, 0), 3))
        // phone top pointing left: screen right edge is up → dot is top-left
        assertEquals(1 to 1, brightest(PreviewTensor.toTensor(rb, n, 1), 3))
        assertEquals(1 to 6, brightest(PreviewTensor.toTensor(rb, n, 2), 3))
        assertEquals(6 to 6, brightest(PreviewTensor.toTensor(rb, n, 3), 3))
    }

    @Test
    fun nearnessMapsBackToTheSameScreenPixel() {
        for (rot in 0..3) {
            val rb = readbackWithDot(5, 2)
            val t = PreviewTensor.toTensor(rb, n, rot)
            // "network" output = red channel of its input
            val near = FloatArray(n * n) { if (t[it * 3] > 0f) 1f else 0f }
            val gl = PreviewTensor.toGlBytes(near, n, rot)
            val hot = gl.indexOfFirst { it.toInt() and 0xFF == 255 }
            assertEquals("rotation $rot", (n - 1 - 2) * n + 5, hot)
        }
    }
}
