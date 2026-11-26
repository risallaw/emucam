package com.emucam.model

import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.sqrt

/** Viewfinder composition guides (overlay only, never part of the photo). */
enum class Grid(val label: String, val orientable: Boolean = false) {
    OFF("Grid off"),
    THIRDS("Rule of thirds"),
    PHI("Golden ratio grid"),
    SPIRAL("Golden spiral", orientable = true),
    TRIANGLE("Golden triangle", orientable = true),
    DIAGONALS("Diagonals"),
    CENTER("Centre cross");

    fun next(): Grid = entries[(ordinal + 1) % entries.size]

    companion object {
        fun fromName(s: String?): Grid = entries.firstOrNull { it.name == s } ?: OFF
    }
}

const val PHI = 1.618034f

/**
 * Line segments (x0, y0, x1, y1), normalized 0..1 with y down, for [grid] in
 * a frame of width/height [aspect]. [orient] 0..3 mirrors the spiral /
 * triangle (bit 0 horizontally, bit 1 vertically) so the eye can be placed
 * at any corner.
 */
fun gridSegments(grid: Grid, aspect: Float, orient: Int = 0): List<FloatArray> {
    val fx = orient and 1 != 0
    val fy = orient and 2 != 0
    fun seg(x0: Float, y0: Float, x1: Float, y1: Float) = floatArrayOf(
        if (fx) 1 - x0 else x0, if (fy) 1 - y0 else y0,
        if (fx) 1 - x1 else x1, if (fy) 1 - y1 else y1,
    )
    fun lines(vararg t: Float) = t.flatMap { listOf(seg(it, 0f, it, 1f), seg(0f, it, 1f, it)) }
    return when (grid) {
        Grid.OFF -> emptyList()
        Grid.THIRDS -> lines(1f / 3, 2f / 3)
        Grid.PHI -> lines(1 - 1 / PHI, 1 / PHI)
        Grid.DIAGONALS -> listOf(seg(0f, 0f, 1f, 1f), seg(1f, 0f, 0f, 1f))
        Grid.CENTER -> listOf(seg(0.5f, 0.4f, 0.5f, 0.6f), seg(0.5f - 0.1f / aspect, 0.5f, 0.5f + 0.1f / aspect, 0.5f))
        Grid.TRIANGLE -> {
            // main diagonal plus perpendiculars from the other two corners (true right angles)
            val w = aspect
            val l = sqrt(w * w + 1)
            val dx = w / l
            val dy = 1 / l
            val t1 = w * dx // foot from (w, 0)
            val t2 = 1 * dy // foot from (0, 1)
            listOf(
                seg(0f, 0f, 1f, 1f),
                seg(1f, 0f, dx * t1 / w, dy * t1),
                seg(0f, 1f, dx * t2 / w, dy * t2),
            )
        }
        Grid.SPIRAL -> spiral(aspect).map { seg(it[0], it[1], it[2], it[3]) }
    }
}

/**
 * Golden (Fibonacci) spiral: square cuts of a golden rectangle with a
 * quarter arc in each, stretched to the frame (transposed for portrait).
 */
private fun spiral(aspect: Float): List<FloatArray> {
    val out = mutableListOf<FloatArray>()
    var x = 0f
    var y = 0f
    var w = PHI
    var h = 1f
    val portrait = aspect < 1f
    fun add(x0: Float, y0: Float, x1: Float, y1: Float) {
        // golden rect [0, φ]×[0, 1] → unit frame
        out += if (portrait) floatArrayOf(y0, x0 / PHI, y1, x1 / PHI) else floatArrayOf(x0 / PHI, y0, x1 / PHI, y1)
    }
    fun arc(cx: Float, cy: Float, r: Float, a0: Float) {
        val n = 12
        for (i in 0 until n) {
            val a = (a0 + 90f * i / n) * PI.toFloat() / 180f
            val b = (a0 + 90f * (i + 1) / n) * PI.toFloat() / 180f
            add(cx + r * cos(a), cy + r * sin(a), cx + r * cos(b), cy + r * sin(b))
        }
    }
    for (k in 0 until 9) {
        when (k % 4) {
            0 -> { val s = h; arc(x + s, y + s, s, 180f); add(x + s, y, x + s, y + h); x += s; w -= s }
            1 -> { val s = w; arc(x, y + s, s, 270f); add(x, y + s, x + w, y + s); y += s; h -= s }
            2 -> { val s = h; arc(x + w - s, y, s, 0f); add(x + w - s, y, x + w - s, y + h); w -= s }
            else -> { val s = w; arc(x + w, y + h - s, s, 90f); add(x, y + h - s, x + w, y + h - s); h -= s }
        }
    }
    return out
}
