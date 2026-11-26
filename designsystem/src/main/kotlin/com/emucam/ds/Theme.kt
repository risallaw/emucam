package com.emucam.ds

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/**
 * emucam design language: Leica/Hasselblad restraint. Ink black, graphite
 * surfaces, bone text, ONE accent (signal red) for the shutter/active state,
 * amber reserved for the "darkroom" (developing).
 */
object Emu {
    val Ink = Color(0xFF0B0B0C)
    val Graphite = Color(0xFF161618)
    val GraphiteHi = Color(0xFF222225)
    val Line = Color(0xFF2E2E32)
    val Bone = Color(0xFFEDEAE3)
    val Muted = Color(0xFF8A8780)
    val Signal = Color(0xFFE2231A)
    val Amber = Color(0xFFF2A900)

    /** Exposure readouts: monospace, tabular. */
    val Mono = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 12.sp, letterSpacing = 0.04.em, color = Bone)

    /** Edge-code style labels (film rebate). */
    val Label = TextStyle(
        fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.Medium,
        fontSize = 11.sp, letterSpacing = 0.12.em, color = Muted,
    )

    val Title = TextStyle(
        fontFamily = FontFamily.SansSerif, fontWeight = FontWeight.SemiBold,
        fontSize = 15.sp, letterSpacing = 0.06.em, color = Bone,
    )

    val Body = TextStyle(fontFamily = FontFamily.SansSerif, fontSize = 12.sp, lineHeight = 16.sp, color = Muted)
}

@Composable
fun EmuTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Emu.Bone,
            onPrimary = Emu.Ink,
            secondary = Emu.Signal,
            tertiary = Emu.Amber,
            background = Emu.Ink,
            surface = Emu.Graphite,
            surfaceVariant = Emu.GraphiteHi,
            onBackground = Emu.Bone,
            onSurface = Emu.Bone,
            onSurfaceVariant = Emu.Muted,
            outline = Emu.Line,
        ),
        content = content,
    )
}

fun parseHex(hex: String, fallback: Color = Emu.Muted): Color = runCatching {
    val h = hex.removePrefix("#")
    Color(("FF" + h).toLong(16))
}.getOrDefault(fallback)
