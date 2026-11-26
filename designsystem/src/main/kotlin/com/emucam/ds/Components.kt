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
