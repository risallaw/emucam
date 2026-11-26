package com.emucam.develop

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class CachedInferenceTest {
    @get:Rule val tmp = TemporaryFolder()

    private fun infer(
        file: java.io.File,
        wants: Boolean = true,
        input: () -> FloatArray = { FloatArray(2) },
        run: suspend (FloatArray) -> FloatArray?,
    ) = runBlocking {
        DevelopQueue.cachedInference("veg", "s1", file, 4, { wants }, input, run)
    }

    @Test fun failingNetworkGivesNull() {
        assertNull(infer(tmp.newFile("a")) { error("gpu died") })
    }

    @Test fun unavailableNetworkGivesNull() {
        assertNull(infer(tmp.newFile("b")) { null })
    }

    @Test fun failingInputGivesNull() {
        assertNull(infer(tmp.newFile("c"), input = { error("bad jpeg") }) { FloatArray(4) })
    }

    @Test fun notWantedSkipsTheNetwork() {
        var ran = false
        assertNull(infer(tmp.newFile("d"), wants = false) { ran = true; FloatArray(4) })
        assertEquals(false, ran)
    }

    @Test fun resultIsCachedAndReused() {
        val f = java.io.File(tmp.root, "veg.f32")
        val v = floatArrayOf(1f, 2f, 3f, 4f)
        assertArrayEquals(v, infer(f) { v }, 0f)
        var ran = false
        val again = infer(f) { ran = true; null }
        assertNotNull(again)
        assertArrayEquals(v, again, 0f)
        assertEquals(false, ran)
    }

    @Test fun wrongSizedCacheIsRecomputed() {
        val f = tmp.newFile("e").apply { writeBytes(ByteArray(5)) }
        assertArrayEquals(FloatArray(4) { 9f }, infer(f) { FloatArray(4) { 9f } }, 0f)
    }

    @Test fun runChunksConcatenatesPassesInOrder() = runBlocking {
        val out = DevelopQueue.runChunks(floatArrayOf(1f, 2f, 3f, 4f, 5f, 6f), 3) { c -> floatArrayOf(c[0] * 10, c[1] * 10) }
        assertEquals(listOf(10f, 20f, 30f, 40f, 50f, 60f), out!!.toList())
    }

    @Test fun runChunksFailsWhenAnyPassFails() = runBlocking {
        var calls = 0
        val out = DevelopQueue.runChunks(FloatArray(4), 2) { calls++; if (calls == 2) null else it }
        assertNull(out)
    }
}
