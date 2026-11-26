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
    /**
     * Like [developStart], with the raw output of a depth network for this
     * photo ([depth] in its portrait layout [depthW]×[depthH]; [depthKind]
     * 0 = depth, 1 = inverse depth). A bad map falls back to the lens field.
     */
    @JvmStatic external fun developStartDepth(
        jpeg: ByteArray, recipeJson: String, maxDim: Int, quality: Int,
        depth: FloatArray?, depthW: Int, depthH: Int, depthKind: Int,
    ): Long
    /**
     * Like [developStartDepth], plus the raw logits of a semantic-segmentation
     * network ([veg]: [vegClasses] planes of [vegW]×[vegH], class-major) used
     * to limit infrared glow to real vegetation. Either map may be null; a bad
     * map is ignored. Free the handle with [developFree].
     */
    @JvmStatic external fun developStartAux(
        jpeg: ByteArray, recipeJson: String, maxDim: Int, quality: Int,
        depth: FloatArray?, depthW: Int, depthH: Int, depthKind: Int,
        veg: FloatArray?, vegClasses: Int, vegW: Int, vegH: Int,
    ): Long
    @JvmStatic external fun developProgress(handle: Long): Float
    @JvmStatic external fun developStatus(handle: Long): Int
    @JvmStatic external fun developResult(handle: Long): ByteArray
    @JvmStatic external fun developError(handle: Long): String
    @JvmStatic external fun developCancel(handle: Long)
    @JvmStatic external fun developFree(handle: Long)

    /** Whether developing with this recipe would use a depth map. */
    @JvmStatic external fun wantsDepth(recipeJson: String): Boolean

    /** Whether developing with this recipe would use a vegetation mask (the film is infrared). */
    @JvmStatic external fun wantsVeg(recipeJson: String): Boolean

    /** Depth-network input tensor (ImageNet-normalised) for a JPEG; landscape shots are rotated to portrait. */
    @JvmStatic external fun depthInput(jpeg: ByteArray, width: Int, height: Int, nchw: Boolean): FloatArray

    /** Raw portrait depth output → nearness (0 far … 1 near), or null if unusable. */
    @JvmStatic external fun nearness(raw: FloatArray, width: Int, height: Int, kind: Int): FloatArray?

    /**
     * Raw segmentation logits ([classes] NCHW planes of [width]×[height]) →
     * per-pixel vegetation probability (0..1, [width]×[height], same layout as
     * the network's output). Null if the output is unusable (too few classes,
     * size mismatch, non-finite values); callers then fall back to colour-only.
     */
    @JvmStatic external fun vegetation(raw: FloatArray, classes: Int, width: Int, height: Int): FloatArray?

    fun lutBytes(size: Int): Int = size * size * size * 8
}
