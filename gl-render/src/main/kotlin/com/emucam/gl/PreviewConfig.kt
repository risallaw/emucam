package com.emucam.gl

import java.nio.ByteBuffer

/**
 * Everything the preview renderer needs for one recipe — produced by Rust
 * (`EmuNative.previewParamsJson` + `bakeLuts`). Immutable; swapped atomically.
 */
class PreviewConfig(
    /** uniform name → 1..4 floats. Uploaded generically by name. */
    val uniforms: Map<String, FloatArray>,
    /** shader `#define`s (selects shader variant). */
    val defines: List<String>,
    val programKey: String,
    val lutSize: Int,
    /** RGBA16F 3D LUTs (lutSize³ × 8 bytes), direct buffers. */
    val lutDevelop: ByteBuffer,
    val lutPrint: ByteBuffer,
    /** Increments with every new recipe (LUT re-upload trigger). */
    val version: Long,
) {
    companion object {
        val NEUTRAL_DEFINES = emptyList<String>()
    }
}

/** Live, gesture-driven overrides that must not wait for a LUT re-bake. */
data class LiveParams(
    val focusCenter: FloatArray? = null,
    val focusAngle: Float? = null,
)

/** Edge of the square live depth map / readback frame (MiDaS small input). */
const val LIVE_DEPTH_SIZE = 256

/**
 * Live nearness map (0 far … 1 near as bytes 0..255), [size]×[size], rows in
 * GL order (bottom row first) to match the renderer's readback.
 */
class DepthFrame(val near: ByteArray, val size: Int, val version: Long) {
    /**
     * Nearness at the focus point (image coords, y down): median of a small
     * window, like `depth::focus_nearness` in Rust.
     */
    fun focusNear(center: FloatArray): Float {
        val r = maxOf(size / 60, 1)
        val cx = (center[0].coerceIn(0f, 1f) * size).toInt()
        val cy = size - 1 - (center[1].coerceIn(0f, 1f) * size).toInt()
        // primitive array: this runs on the GL thread every frame (no boxing garbage)
        val v = IntArray((2 * r + 1) * (2 * r + 1))
        var i = 0
        for (y in cy - r..cy + r) for (x in cx - r..cx + r) {
            v[i++] = near[y.coerceIn(0, size - 1) * size + x.coerceIn(0, size - 1)].toInt() and 0xFF
        }
        v.sort()
        return v[v.size / 2] / 255f
    }
}

/** Edge of the square live vegetation map (PIDNet-S output). */
const val LIVE_VEG_SIZE = 128

/**
 * Live vegetation probability (0..1 as bytes 0..255), [size]×[size], rows in
 * GL order (bottom row first) of the screen image, like [DepthFrame]. Gates
 * the infrared preview's foliage masks the way `infrared::gate_by_seg` does.
 */
class VegFrame(val prob: ByteArray, val size: Int, val version: Long) {
    /** Whether this frame matches the renderer's [LIVE_VEG_SIZE]² texture. */
    val valid: Boolean get() = size == LIVE_VEG_SIZE && prob.size == size * size
}
