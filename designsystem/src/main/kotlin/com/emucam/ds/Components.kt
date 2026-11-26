package com.emucam.ds

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlin.math.roundToInt

/** Shutter: bone ring, signal-red core; amber progress arc while developing. */
@Composable
fun ShutterButton(onClick: () -> Unit, developing: Float?, modifier: Modifier = Modifier) {
    val src = remember { MutableInteractionSource() }
    val pressed by src.collectIsPressedAsState()
    val s by animateFloatAsState(if (pressed) 0.9f else 1f, spring(dampingRatio = 0.5f), label = "shutter")
    Box(
        modifier
            .size(76.dp)
            .scale(s)
            .clickable(interactionSource = src, indication = null, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(76.dp)) {
            drawCircle(Emu.Bone, radius = size.minDimension / 2 - 3.dp.toPx(), style = Stroke(3.dp.toPx()))
            drawCircle(Emu.Signal, radius = size.minDimension / 2 - 11.dp.toPx())
            if (developing != null) {
                drawArc(
                    Emu.Amber, -90f, 360f * developing.coerceIn(0f, 1f), false,
                    style = Stroke(3.dp.toPx(), cap = StrokeCap.Round),
                )
            }
        }
    }
}

/** Film-window card: color stripe like a canister label + edge-code name. */
@Composable
fun FilmCard(name: String, iso: Int?, note: String, color: Color, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier
            .width(148.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(if (selected) Emu.GraphiteHi else Emu.Graphite)
            .border(1.dp, if (selected) Emu.Bone else Emu.Line, RoundedCornerShape(6.dp))
            .clickable(onClick = onClick),
    ) {
        Box(Modifier.fillMaxWidth().height(6.dp).background(color))
        Column(Modifier.padding(10.dp)) {
            Text(name.uppercase(), style = Emu.Title, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (iso != null) Text("ISO $iso", style = Emu.Mono.copy(color = Emu.Muted))
            Spacer(Modifier.height(4.dp))
            Text(note, style = Emu.Body, maxLines = 3, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** Small pill chip (aperture shapes, bundled effects, categories). */
@Composable
fun Chip(text: String, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier, accent: Color = Emu.Bone) {
    Box(
        modifier
            .clip(RoundedCornerShape(50))
            .background(if (selected) accent.copy(alpha = 0.16f) else Color.Transparent)
            .border(1.dp, if (selected) accent else Emu.Line, RoundedCornerShape(50))
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 6.dp),
    ) {
        Text(text.uppercase(), style = Emu.Label.copy(color = if (selected) accent else Emu.Muted))
    }
}

/**
 * Horizontal detent wheel: drag to change, haptic tick per step, double-tap
 * to reset. Tick marks scroll like a lens scale.
 */
@Composable
fun DetentWheel(
    label: String,
    value: Float,
    min: Float,
    max: Float,
    step: Float,
    default: Float,
    format: (Float) -> String,
    onChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
) {
    val haptics = LocalHapticFeedback.current
    val current by rememberUpdatedState(value)
    var acc by remember { mutableFloatStateOf(0f) }
    val pxPerStep = 18f
    Column(modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(label.uppercase(), style = Emu.Label)
            Spacer(Modifier.weight(1f))
            Text(format(value), style = Emu.Mono)
        }
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(28.dp)
                .pointerInput(min, max, step) {
                    detectTapGestures(onDoubleTap = { onChange(default) })
                }
                .pointerInput(min, max, step) {
                    detectHorizontalDragGestures(onDragEnd = { acc = 0f }) { _, dx ->
                        acc -= dx
                        val steps = (acc / pxPerStep).toInt()
                        if (steps != 0) {
                            acc -= steps * pxPerStep
                            val nv = (current + steps * step).coerceIn(min, max)
                            if (nv != current) {
                                haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                onChange((nv / step).roundToInt() * step)
                            }
                        }
                    }
                },
        ) {
            val cx = size.width / 2
            val stepPx = pxPerStep.dp.toPx() / 2.2f
            val offset = ((value - min) / step) * stepPx
            val n = ((max - min) / step).roundToInt()
            for (i in 0..n) {
                val x = cx + i * stepPx - offset
                if (x < 0 || x > size.width) continue
                val major = i % 5 == 0
                val h = if (major) size.height * 0.6f else size.height * 0.35f
                val fade = 1f - abs(x - cx) / cx
                drawLine(Emu.Muted.copy(alpha = 0.3f + 0.6f * fade), Offset(x, size.height - h), Offset(x, size.height), 1.dp.toPx())
            }
            drawLine(Emu.Signal, Offset(cx, 0f), Offset(cx, size.height), 2.dp.toPx())
        }
    }
}

/** 2D character pad (x: cool↔warm, y: faded↔punchy), à la Photographic Styles. */
@Composable
fun XYPad(x: Float, y: Float, onChange: (Float, Float) -> Unit, modifier: Modifier = Modifier) {
    val cb by rememberUpdatedState(onChange)
    Box(
        modifier
            .size(148.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(Emu.Graphite)
            .border(1.dp, Emu.Line, RoundedCornerShape(8.dp))
            .pointerInput(Unit) {
                detectTapGestures(onDoubleTap = { cb(0f, 0f) })
            }
            .pointerInput(Unit) {
                detectDragGestures { change, _ ->
                    val nx = (change.position.x / size.width * 2 - 1).coerceIn(-1f, 1f)
                    val ny = -(change.position.y / size.height * 2 - 1).coerceIn(-1f, 1f)
                    cb(nx, ny)
                }
            },
    ) {
        Canvas(Modifier.matchParentSizeCompat()) {
            val c = Offset(size.width / 2, size.height / 2)
            drawLine(Emu.Line, Offset(0f, c.y), Offset(size.width, c.y))
            drawLine(Emu.Line, Offset(c.x, 0f), Offset(c.x, size.height))
            val p = Offset((x + 1) / 2 * size.width, (1 - (y + 1) / 2) * size.height)
            drawCircle(Emu.Bone, 7.dp.toPx(), p)
            drawCircle(Emu.Ink, 3.dp.toPx(), p)
        }
        Text("WARM", style = Emu.Label, modifier = Modifier.align(Alignment.CenterEnd).padding(4.dp))
        Text("COOL", style = Emu.Label, modifier = Modifier.align(Alignment.CenterStart).padding(4.dp))
        Text("PUNCH", style = Emu.Label, modifier = Modifier.align(Alignment.TopCenter).padding(4.dp))
        Text("FADE", style = Emu.Label, modifier = Modifier.align(Alignment.BottomCenter).padding(4.dp))
    }
}

private fun Modifier.matchParentSizeCompat() = this.then(Modifier.size(148.dp))

/** Round icon-ish button with a short text glyph (keeps the app asset-free). */
@Composable
fun GlyphButton(glyph: String, onClick: () -> Unit, modifier: Modifier = Modifier, active: Boolean = false) {
    Box(
        modifier
            .size(44.dp)
            .clip(CircleShape)
            .background(if (active) Emu.Bone.copy(alpha = 0.15f) else Emu.Graphite.copy(alpha = 0.7f))
            .border(1.dp, if (active) Emu.Bone else Emu.Line, CircleShape)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(glyph, style = Emu.Mono.copy(color = if (active) Emu.Bone else Emu.Muted))
    }
}

/**
 * Labelled round action (share, delete…): a 52dp glyph button over an
 * edge-code label, so the action is readable without guessing icons.
 */
@Composable
fun ActionButton(
    glyph: String,
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    accent: Color = Emu.Bone,
) {
    val tint = if (enabled) accent else Emu.Muted.copy(alpha = 0.4f)
    Column(
        modifier
            .clip(RoundedCornerShape(10.dp))
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 6.dp, vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier
                .size(52.dp)
                .clip(CircleShape)
                .background(Emu.Graphite)
                .border(1.dp, if (enabled) accent.copy(alpha = 0.5f) else Emu.Line, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(glyph, style = Emu.Title.copy(color = tint))
        }
        Spacer(Modifier.height(6.dp))
        Text(label.uppercase(), style = Emu.Label.copy(color = tint), maxLines = 1)
    }
}

/** Full-width primary action (one per surface), filled with [accent]. */
@Composable
fun PrimaryButton(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, accent: Color = Emu.Amber, enabled: Boolean = true) {
    Box(
        modifier
            .fillMaxWidth()
            .height(52.dp)
            .clip(RoundedCornerShape(26.dp))
            .background(if (enabled) accent else Emu.GraphiteHi)
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(text.uppercase(), style = Emu.Title.copy(color = if (enabled) Emu.Ink else Emu.Muted))
    }
}

/** Thin darkroom progress bar (amber on graphite); null = indeterminate-looking empty track. */
@Composable
fun DevelopBar(progress: Float?, modifier: Modifier = Modifier, color: Color = Emu.Amber) {
    Canvas(modifier.fillMaxWidth().height(3.dp)) {
        drawRect(Emu.Line)
        if (progress != null) drawRect(color, size = size.copy(width = size.width * progress.coerceIn(0f, 1f)))
    }
}

/** [deg] snapped to the nearest of 0/90/180/270 when within 4°, normalized to 0..<360; else null. */
fun snapAngle(deg: Float): Float? {
    val n = ((deg % 360f) + 360f) % 360f
    return listOf(0f, 90f, 180f, 270f, 360f).firstOrNull { abs(it - n) < SNAP_DEG }?.rem(360f)
}

private const val SNAP_DEG = 4f

/**
 * Where around the frame something sits (OMNI wand, light leak, …): a ring
 * like the filter ring on the lens, with the photo frame drawn inside for
 * orientation. Tap or drag anywhere to swing the knob to that direction.
 * [deg] is image-space (0 right, 90 bottom, 180 left, 270 top).
 */
@Composable
fun PositionDial(deg: Float, onChange: (Float) -> Unit, modifier: Modifier = Modifier, label: String? = null, dialSize: Dp = 72.dp) {
    val cb by rememberUpdatedState(onChange)
    val haptics = LocalHapticFeedback.current
    var lastSnap by remember { mutableFloatStateOf(-1f) }
    fun pick(p: Offset, w: Float, h: Float) {
        val a = Math.toDegrees(kotlin.math.atan2((p.y - h / 2).toDouble(), (p.x - w / 2).toDouble())).toFloat()
        val n = ((a % 360f) + 360f) % 360f
        val snapped = snapAngle(n)
        // one tick when entering a snap point, not on every drag event inside it
        if (snapped != null && snapped != lastSnap) haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
        lastSnap = snapped ?: -1f
        cb(snapped ?: n)
    }
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            Modifier
                .size(dialSize)
                .clip(CircleShape)
                .background(Emu.Graphite)
                .pointerInput(Unit) { detectTapGestures { pick(it, size.width.toFloat(), size.height.toFloat()) } }
                .pointerInput(Unit) {
                    detectDragGestures { change, _ -> pick(change.position, size.width.toFloat(), size.height.toFloat()) }
                },
        ) {
            Canvas(Modifier.size(dialSize)) {
                val c = Offset(size.width / 2, size.height / 2)
                val ringR = size.minDimension / 2 - 7.dp.toPx()
                drawCircle(Emu.Line, ringR, c, style = Stroke(2.dp.toPx()))
                // the photo frame (portrait 3:4) inside the ring
                val fh = ringR * 1.1f
                val fw = fh * 0.75f
                drawRect(Emu.Muted, Offset(c.x - fw / 2, c.y - fh / 2), androidx.compose.ui.geometry.Size(fw, fh), style = Stroke(1.dp.toPx()))
                val r = Math.toRadians(deg.toDouble())
                val dir = Offset(kotlin.math.cos(r).toFloat(), kotlin.math.sin(r).toFloat())
                // footprint on the frame side it affects, then the knob on the ring
                drawLine(Emu.Signal.copy(alpha = 0.6f), c, c + dir * ringR, 1.5.dp.toPx())
                drawCircle(Emu.Signal.copy(alpha = 0.25f), 12.dp.toPx(), c + dir * ringR)
                drawCircle(Emu.Bone, 6.dp.toPx(), c + dir * ringR)
                drawCircle(Emu.Ink, 2.5.dp.toPx(), c + dir * ringR)
            }
        }
        if (label != null) {
            Spacer(Modifier.height(4.dp))
            Text(label, style = Emu.Label)
        }
    }
}

/**
 * Composition guide over the viewfinder: [segments] are (x0, y0, x1, y1)
 * normalized 0..1. Drawn twice (dark under light) so it reads on any scene.
 */
@Composable
fun GuideOverlay(segments: List<FloatArray>, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val (w, h) = size.width to size.height
        for (pass in 0..1) {
            val color = if (pass == 0) Color.Black.copy(alpha = 0.35f) else Emu.Bone.copy(alpha = 0.7f)
            val stroke = if (pass == 0) 2.5.dp.toPx() else 1.dp.toPx()
            segments.forEach { s ->
                drawLine(color, Offset(s[0] * w, s[1] * h), Offset(s[2] * w, s[3] * h), stroke, cap = StrokeCap.Round)
            }
        }
    }
}
