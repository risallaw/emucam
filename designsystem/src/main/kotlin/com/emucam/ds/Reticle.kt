package com.emucam.ds

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateRotation
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.sin

enum class FocusShape { NONE, SPOT, SLICE }

/**
 * Sweet-spot / slice reticle over the viewfinder. One finger drags the focus
 * center, pinch resizes, two-finger rotate turns the slice. Coordinates are
 * normalized image space (0..1, y down) — the same space Rust and GLSL use.
 */
@Composable
fun SweetSpotReticle(
    shape: FocusShape,
    center: Offset,
    /** Sweet-spot radius as fraction of the half-diagonal (s0 × size). */
    radius: Float,
    falloff: Float,
    angleDeg: Float,
    onMove: (Offset) -> Unit,
    onResize: (Float) -> Unit,
    onRotate: (Float) -> Unit,
    onTap: (Offset) -> Unit,
    onGestureEnd: () -> Unit,
    modifier: Modifier = Modifier,
    /** Double tap: back to the default focus (centre, size, angle). */
    onReset: () -> Unit = {},
) {
    val c by rememberUpdatedState(center)
    val move by rememberUpdatedState(onMove)
    val resize by rememberUpdatedState(onResize)
    val rotate by rememberUpdatedState(onRotate)
    val tap by rememberUpdatedState(onTap)
    val end by rememberUpdatedState(onGestureEnd)
    val reset by rememberUpdatedState(onReset)
    Canvas(
        modifier
            .fillMaxSize()
            .pointerInput(Unit) {
                detectTapGestures(
                    onDoubleTap = { reset() },
                    onTap = { p -> tap(Offset(p.x / size.width, p.y / size.height)) },
                )
            }
            .pointerInput(Unit) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    var moved = false
                    do {
                        val event = awaitPointerEvent()
                        val pan = event.calculatePan()
                        val zoom = event.calculateZoom()
                        val rot = event.calculateRotation()
                        if (pan != Offset.Zero || zoom != 1f || rot != 0f) {
                            moved = true
                            move(Offset((c.x + pan.x / size.width).coerceIn(0f, 1f), (c.y + pan.y / size.height).coerceIn(0f, 1f)))
                            if (zoom != 1f) resize(zoom)
                            if (rot != 0f) rotate(rot)
                            event.changes.forEach { if (it.positionChanged()) it.consume() }
                        }
                    } while (event.changes.any { it.pressed })
                    if (moved) end()
                }
            },
    ) {
        if (shape == FocusShape.NONE) return@Canvas
        val hd = hypot(size.width, size.height) / 2
        val p = Offset(c.x * size.width, c.y * size.height)
        val dash = PathEffect.dashPathEffect(floatArrayOf(6.dp.toPx(), 6.dp.toPx()))
        when (shape) {
            FocusShape.SPOT -> {
                drawCircle(Emu.Bone.copy(alpha = 0.9f), radius * hd, p, style = Stroke(1.dp.toPx()))
                drawCircle(Emu.Bone.copy(alpha = 0.35f), (radius + falloff * 0.5f) * hd, p, style = Stroke(1.dp.toPx(), pathEffect = dash))
                drawCircle(Emu.Signal, 3.dp.toPx(), p)
            }
            FocusShape.SLICE -> {
                val a = Math.toRadians(angleDeg.toDouble())
                val dir = Offset(cos(a).toFloat(), sin(a).toFloat())
                val n = Offset(-dir.y, dir.x)
                val len = hd * 2
                for (s in listOf(-1f, 1f)) {
                    val o = p + n * (s * radius * hd)
                    drawLine(Emu.Bone.copy(alpha = 0.85f), o - dir * len, o + dir * len, 1.dp.toPx())
                    val o2 = p + n * (s * (radius + falloff * 0.5f) * hd)
                    drawLine(Emu.Bone.copy(alpha = 0.3f), o2 - dir * len, o2 + dir * len, 1.dp.toPx(), pathEffect = dash)
                }
                drawCircle(Emu.Signal, 3.dp.toPx(), p)
            }
            FocusShape.NONE -> {}
        }
    }
}
