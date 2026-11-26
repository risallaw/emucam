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
