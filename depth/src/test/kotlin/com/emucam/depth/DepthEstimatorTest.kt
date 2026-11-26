package com.emucam.depth

import com.google.ai.edge.litert.Accelerator
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DepthEstimatorTest {
    private val model = DepthModel.PREVIEW
    private val input = FloatArray(model.inputSize)

    /** Fake LiteRT: records opens/closes; [failRun] / [failOpen] per accelerator. */
    private class Fake(
        val failOpen: Set<Accelerator> = emptySet(),
        val failRun: Set<Accelerator> = emptySet(),
        val outSize: Int = DepthModel.PREVIEW.outputSize,
    ) {
        val opened = mutableListOf<Accelerator>()
        val attempts = mutableListOf<Accelerator>()
        var live = 0

        fun open(acc: Accelerator): DepthSession {
            attempts += acc
            if (acc in failOpen) error("no $acc")
            opened += acc
            live++
            return object : DepthSession {
                var closed = false
                override fun run(input: FloatArray): FloatArray {
                    check(!closed) { "used after close" }
                    if (acc in failRun) error("$acc died")
                    return FloatArray(outSize)
                }
                override fun close() {
                    check(!closed) { "double close" }
                    closed = true
                    live--
                }
            }
        }
    }

    private fun estimator(f: Fake, idle: Long = 60_000) = DepthEstimator(model, idle) { f.open(it) }

    @Test
    fun runtimeFailureIsReportedAsNullAndFallsBackToCpu() = runBlocking {
        val f = Fake(failRun = setOf(Accelerator.GPU))
        val e = estimator(f)
        // GPU dies mid-inference: no exception escapes (it would crash the live loop)
        assertNull(e.run(input))
        assertNull(e.unavailable)
        assertEquals(0, f.live)
        assertNotNull(e.run(input))
        assertEquals(listOf(Accelerator.GPU, Accelerator.CPU), f.opened)
        e.close()
    }

    @Test
    fun failingEverywhereMarksUnavailable() = runBlocking {
        val f = Fake(failRun = setOf(Accelerator.GPU, Accelerator.CPU))
        val e = estimator(f)
        assertNull(e.run(input))
        assertNull(e.run(input))
        assertNotNull(e.unavailable)
        assertNull(e.run(input))
        assertEquals(2, f.opened.size)
    }

    @Test(timeout = 10_000)
    fun loadFailureFallsBackWithoutRetryingGpu() = runBlocking {
        val f = Fake(failOpen = setOf(Accelerator.GPU))
        val e = estimator(f, idle = 1)
        assertNotNull(e.run(input))
        delay(50) // idle release
        assertEquals(0, f.live)
        assertNotNull(e.run(input))
        assertEquals(listOf(Accelerator.GPU, Accelerator.CPU, Accelerator.CPU), f.attempts)
        e.close()
    }

    @Test
    fun wrongOutputSizeIsNull() = runBlocking {
        val e = estimator(Fake(outSize = 3))
        assertNull(e.run(input))
        e.close()
    }

    @Test
    fun closeReleasesAndRunAfterCloseNeverReloads() = runBlocking {
        val f = Fake()
        val e = estimator(f)
        assertNotNull(e.run(input))
        e.close()
        e.close() // idempotent
        assertNull(e.run(input))
        delay(50)
        assertEquals(1, f.opened.size)
        assertEquals("model released, nothing leaked", 0, f.live)
        assertTrue(e.unavailable == null)
    }
}

class DepthModelTest {
    @Test fun segmentOutputIsNineteenLogitPlanes() {
        val m = DepthModel.SEGMENT
        assertEquals(19 * 128 * 128, m.outputSize)
        assertEquals(1024 * 1024 * 3, m.inputSize)
    }

    @Test fun depthModelsKeepOneInputSizedPlane() {
        assertEquals(504 * 896, DepthModel.CAPTURE.outputSize)
        assertEquals(256 * 256, DepthModel.PREVIEW.outputSize)
    }
}
