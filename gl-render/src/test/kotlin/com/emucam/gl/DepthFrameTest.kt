package com.emucam.gl

import org.junit.Assert.assertEquals
import org.junit.Test

class DepthFrameTest {
    private val n = 64

    /** Frame whose GL row-order bytes are [f](x, imageY) (image y down). */
    private fun frame(f: (Int, Int) -> Int) =
        DepthFrame(ByteArray(n * n) { i -> f(i % n, n - 1 - i / n).toByte() }, n, 1)

    @Test
    fun focusNearIsTheWindowMedianInImageCoords() {
        // top half near (255), bottom half far (0): GL rows are bottom-up
        val d = frame { _, y -> if (y < n / 2) 255 else 0 }
        assertEquals(1f, d.focusNear(floatArrayOf(0.5f, 0.1f)), 0f)
        assertEquals(0f, d.focusNear(floatArrayOf(0.5f, 0.9f)), 0f)
    }

    @Test
    fun medianRejectsOutliersAndEdgesClamp() {
        val d = frame { x, y -> if (x == 32 && y == 32) 255 else 100 }
        assertEquals(100 / 255f, d.focusNear(floatArrayOf(0.5f, 0.5f)), 0f)
        assertEquals(100 / 255f, d.focusNear(floatArrayOf(1f, 1f)), 0f)
        assertEquals(100 / 255f, d.focusNear(floatArrayOf(0f, 0f)), 0f)
    }
}
