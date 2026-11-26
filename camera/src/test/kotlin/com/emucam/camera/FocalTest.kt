package com.emucam.camera

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FocalTest {
    @Test
    fun mainCameraEquivalentFromSensorSize() {
        // 1-inch type sensor (13.2 × 8.8 mm, diag ≈ 15.9 mm), 8.7 mm lens → ≈ 23.7 mm eq
        assertEquals(23.7f, CameraController.mainFocal35(floatArrayOf(8.7f), 13.2f, 8.8f), 0.2f)
    }

    @Test
    fun logicalCameraPicksTheLensNearest24mm() {
        // ultra-wide, main and tele focal lengths reported against one sensor size
        val eq = CameraController.mainFocal35(floatArrayOf(5.2f, 8.7f, 25.0f), 13.2f, 8.8f)
        assertEquals(23.7f, eq, 0.2f)
    }

    @Test
    fun implausibleCharacteristicsFallBackTo24mm() {
        assertEquals(24f, CameraController.mainFocal35(floatArrayOf(), 13.2f, 8.8f))
        assertEquals(24f, CameraController.mainFocal35(floatArrayOf(4f), 0f, 0f))
        assertEquals(24f, CameraController.mainFocal35(floatArrayOf(100f), 5f, 4f))
    }

    @Test
    fun zoomMatchesTheLensFocalLength() {
        assertEquals(3.75f, CameraController.zoomForFocal(90f, 24f), 1e-4f)
        assertEquals(0.625f, CameraController.zoomForFocal(15f, 24f), 1e-4f)
    }

    @Test
    fun zoomIsClampedOnlyOnceTheRangeIsKnown() {
        assertEquals(0.6f, CameraController.applicableZoom(0.3f, 0.6f, 10f)!!, 0f)
        assertEquals(10f, CameraController.applicableZoom(25f, 0.6f, 10f)!!, 0f)
        assertEquals(3.75f, CameraController.applicableZoom(3.75f, 0.6f, 10f)!!, 0f)
        // default 1..1 state must not squash a wanted zoom to 1×
        assertNull(CameraController.applicableZoom(3.75f, 1f, 1f))
    }
}
