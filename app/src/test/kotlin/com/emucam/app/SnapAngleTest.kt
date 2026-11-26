package com.emucam.app

import com.emucam.ds.snapAngle
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SnapAngleTest {
    @Test
    fun snapsToTheFourEdges() {
        assertEquals(90f, snapAngle(92f)!!, 0f)
        assertEquals(180f, snapAngle(177f)!!, 0f)
        assertEquals(270f, snapAngle(270f)!!, 0f)
        assertNull(snapAngle(45f))
    }

    @Test
    fun wrapAroundSnapsToTheSameValueOnBothSides() {
        // 358° and 2° are one snap point: no second haptic tick when crossing 0
        assertEquals(0f, snapAngle(358f)!!, 0f)
        assertEquals(0f, snapAngle(2f)!!, 0f)
    }
}
