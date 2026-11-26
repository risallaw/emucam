package com.emucam.depth

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

class UprightUpscalerTest {
    private val n = 8
    private val m = 32 // 4× like 256 → 1024

    /** RGBA readback (GL rows) with one (255, 0, 128) pixel at screen (x, y), y down; the rest black. */
    private fun readbackWithDot(x: Int, y: Int, size: Int = n) = ByteArray(size * size * 4).also {
        val i = ((size - 1 - y) * size + x) * 4
        it[i] = 255.toByte()
        it[i + 2] = 128.toByte()
    }

    private fun norm(byte: Int, c: Int) = (byte / 255f - PreviewTensor.MEAN[c]) / PreviewTensor.STD[c]

    /** (u, v) of the first maximum of NCHW plane [c] of an [size]² tensor. */
    private fun brightest(t: FloatArray, c: Int, size: Int) =
        (0 until size * size).maxBy { t[c * size * size + it] }.let { it % size to it / size }

    @Test
    fun uprightMappingFollowsDeviceRotationAndScales() {
        val up = UprightUpscaler(n, m)
        val rb = readbackWithDot(6, 1)
        // same upright positions as PreviewTensorTest, scaled by m/n
        val expected = mapOf(0 to (6 to 1), 1 to (1 to 1), 2 to (1 to 6), 3 to (6 to 6))
        for ((rot, uv) in expected) {
            val (u, v) = brightest(up.tensor(rb, rot), 0, m)
            assertEquals("rotation $rot", uv, u / (m / n) to v / (m / n))
        }
    }

    @Test
    fun planesAreNchwRgbAndImageNetNormalised() {
        val t = UprightUpscaler(n, m).tensor(readbackWithDot(3, 3), 0)
        val mm = m * m
        // far from the dot: black in every plane
        for (c in 0..2) assertEquals(norm(0, c), t[c * mm], 1e-5f)
        // at the dot's centre texel (4·3+1 is the first of the two nearest dst pixels)
        val at = (4 * 3 + 1) * m + 4 * 3 + 1
        val w = 1f - 0.125f // bilinear weight of the source texel on each axis
        for ((c, byte) in listOf(0 to 255, 1 to 0, 2 to 128)) {
            val expected = norm(0, c) + (norm(byte, c) - norm(0, c)) * w * w
            assertEquals("plane $c", expected, t[c * mm + at], 1e-4f)
        }
    }

    @Test
    fun buffersAreReused() {
        val up = UprightUpscaler(n, m)
        assertSame(up.tensor(readbackWithDot(1, 1), 0), up.tensor(readbackWithDot(2, 2), 1))
    }

    @Test
    fun vegetationMapsBackToTheSameScreenPixel() {
        // 16² readback → 64² tensor → "network" output 8² (stride 8, like 1024 → 128)
        val rn = 16
        val tn = 64
        val o = 8
        val up = UprightUpscaler(rn, tn)
        for (rot in 0..3) {
            val (sx, sy) = 11 to 4
            val t = up.tensor(readbackWithDot(sx, sy, rn), rot)
            // network: box-sum red plane over each output cell, the hottest cell is "vegetation"
            val cell = tn / o
            val sums = FloatArray(o * o) { i ->
                var s = 0f
                for (dy in 0 until cell) for (dx in 0 until cell) s += t[((i / o) * cell + dy) * tn + (i % o) * cell + dx]
                s
            }
            val hot = sums.indices.maxBy { sums[it] }
            val veg = FloatArray(o * o) { if (it == hot) 1f else 0f }
            val gl = PreviewTensor.toGlBytes(veg, o, rot)
            val got = gl.indexOfFirst { it.toInt() and 0xFF == 255 }
            val (x, y) = sx * o / rn to sy * o / rn
            assertEquals("rotation $rot", (o - 1 - y) * o + x, got)
        }
    }
}
