package com.emucam.app

import com.emucam.app.ui.zoomStops
import org.junit.Assert.assertEquals
import org.junit.Test

class ZoomStopsTest {
    @Test
    fun ultraWideThenMainThenTeleSteps() {
        assertEquals(listOf(0.6f, 1f, 2f, 3f, 5f, 10f), zoomStops(0.6f, 10f))
    }

    @Test
    fun noUltraWideWhenMinIsOne() {
        assertEquals(listOf(1f, 2f, 3f), zoomStops(1f, 3f))
    }
}
