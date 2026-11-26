package com.emucam.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs

class CompositionTest {
    private val all = Grid.entries

    @Test
    fun everyGuideStaysInsideTheFrame() {
        for (g in all) for (o in 0..3) for (a in listOf(0.75f, 1.333f)) {
            gridSegments(g, a, o).forEach { s ->
                assertTrue("$g $o $a ${s.toList()}", s.all { it in -1e-4f..1.0001f })
            }
        }
    }

    @Test
    fun thirdsAndPhiLinesSitWhereTheyShould() {
        val xs = gridSegments(Grid.THIRDS, 0.75f).filter { it[0] == it[2] }.map { it[0] }
        assertEquals(listOf(1f / 3, 2f / 3), xs)
        val ps = gridSegments(Grid.PHI, 0.75f).filter { it[0] == it[2] }.map { it[0] }
        assertEquals(0.382f, ps[0], 1e-3f)
        assertEquals(0.618f, ps[1], 1e-3f)
    }

    @Test
    fun triangleCornerLinesMeetTheDiagonalAtRightAngles() {
        val a = 0.75f
        val (d, p1, _) = gridSegments(Grid.TRIANGLE, a)
        // back to pixel-like units (width = aspect) for the dot product
        val dv = floatArrayOf((d[2] - d[0]) * a, d[3] - d[1])
        val pv = floatArrayOf((p1[2] - p1[0]) * a, p1[3] - p1[1])
        assertEquals(0f, dv[0] * pv[0] + dv[1] * pv[1], 1e-4f)
    }

    @Test
    fun spiralIsOneContinuousCurveAndFlips() {
        val arcs = gridSegments(Grid.SPIRAL, 0.75f).filter { abs(it[0] - it[2]) > 0 && abs(it[1] - it[3]) > 0 }
        for (i in 1 until arcs.size) {
            assertEquals(arcs[i - 1][2], arcs[i][0], 1e-4f)
            assertEquals(arcs[i - 1][3], arcs[i][1], 1e-4f)
        }
        val a = gridSegments(Grid.SPIRAL, 0.75f, 0)[0]
        val b = gridSegments(Grid.SPIRAL, 0.75f, 1)[0]
        assertEquals(a[0], 1 - b[0], 1e-6f)
    }

    @Test
    fun cyclingVisitsEveryGuide() {
        var g = Grid.OFF
        val seen = mutableSetOf<Grid>()
        repeat(all.size) { seen += g; g = g.next() }
        assertEquals(all.toSet(), seen)
        assertEquals(Grid.OFF, g)
        assertEquals(Grid.THIRDS, Grid.fromName("THIRDS"))
        assertEquals(Grid.OFF, Grid.fromName("nope"))
    }
}
