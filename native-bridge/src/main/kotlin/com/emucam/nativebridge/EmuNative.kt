package com.emucam.nativebridge

import java.nio.ByteBuffer

/**
 * JNI surface of `rust/emucore-ffi`. Control plane uses JSON strings so new
 * presets / parameters never require binding changes. Methods throw
 * RuntimeException on Rust errors (and on Rust panics, which are caught).
 */
object EmuNative {
    init {
        System.loadLibrary("emucore_ffi")
    }

    const val STATUS_RUNNING = 0
    const val STATUS_DONE = 1
    const val STATUS_FAILED = 2
    const val STATUS_CANCELLED = 3

    @JvmStatic external fun version(): String

    /** `{films:[...], lenses:[...], bodies:[...], shapes:[...], effect_kinds:[...]}` */
    @JvmStatic external fun catalogJson(): String

    /** Load an external preset pack directory (films/, lenses/, bodies/ with *.toml). */
    @JvmStatic external fun loadPack(path: String)

    /** Validate + migrate a recipe, returning normalized JSON. */
    @JvmStatic external fun normalizeRecipe(recipeJson: String): String

    /** `{uniforms:{name:[floats]}, defines:[...], program_key, lut_size}` for the GL preview. */
    @JvmStatic external fun previewParamsJson(recipeJson: String): String

    /**
     * Bake develop + print 3D LUTs as RGBA16F into direct buffers of at least
     * `lutBytes(size)` bytes. Returns the LUT edge size.
     */
    @JvmStatic external fun bakeLuts(recipeJson: String, develop: ByteBuffer, print: ByteBuffer): Int

    /** Start a background develop job. Returns an opaque handle (free with [developFree]). */
    @JvmStatic external fun developStart(jpeg: ByteArray, recipeJson: String, maxDim: Int, quality: Int): Long
    @JvmStatic external fun developProgress(handle: Long): Float
    @JvmStatic external fun developStatus(handle: Long): Int
    @JvmStatic external fun developResult(handle: Long): ByteArray
    @JvmStatic external fun developError(handle: Long): String
    @JvmStatic external fun developCancel(handle: Long)
    @JvmStatic external fun developFree(handle: Long)

    fun lutBytes(size: Int): Int = size * size * size * 8
}
