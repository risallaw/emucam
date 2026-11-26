package com.emucam.develop

import com.emucam.nativebridge.EmuNative
import kotlinx.serialization.Serializable

/**
 * Container for developed photos. Heavy lens blur makes large smooth
 * gradients, which JPEG blocks and bands at lower qualities, so the default
 * is the highest JPEG setting (quality 100, no chroma subsampling).
 *
 * [nativeCode] / [quality] are the `format` / `quality` arguments of
 * `EmuNative.developStartOut`.
 */
@Serializable
enum class OutputFormat(
    val label: String,
    val detail: String,
    val nativeCode: Int,
    val quality: Int,
    val mimeType: String,
    val extension: String,
) {
    JPEG_MAX("JPEG Max", "Quality 100 · full colour (4:4:4). Smooth blur, larger files.", EmuNative.FORMAT_JPEG, 100, "image/jpeg", "jpg"),
    JPEG_HIGH("JPEG High", "Quality 95 · smallest files. May band in very soft blur.", EmuNative.FORMAT_JPEG, 95, "image/jpeg", "jpg"),
    PNG("PNG", "Lossless 8-bit · largest files, slower to save.", EmuNative.FORMAT_PNG, 100, "image/png", "png"),
    ;

    companion object {
        val DEFAULT = JPEG_MAX

        /** Persisted name → format; unknown or missing names give [DEFAULT]. */
        fun fromName(name: String?): OutputFormat = entries.firstOrNull { it.name == name } ?: DEFAULT
    }
}
