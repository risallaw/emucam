package com.emucam.depth

/**
 * Pixel bookkeeping for the live depth network. The GL preview hands us a
 * square RGBA readback in GL row order (bottom row first) of the image as it
 * appears on the portrait screen. The network wants the scene *upright*, so
 * when the phone is held sideways the image is rotated by the device
 * rotation first, and the result rotated back afterwards.
 *
 * Rotations use Surface.ROTATION_* values (0..3) and the same convention as
 * `UserParams.forCaptureRotation`: rotation 1 = top of the phone points left,
 * so the screen's right edge is the scene's top.
 */
object PreviewTensor {
    internal val MEAN = floatArrayOf(0.485f, 0.456f, 0.406f)
    internal val STD = floatArrayOf(0.229f, 0.224f, 0.225f)

    /** Upright tensor pixel (u, v) → screen pixel (x, y), both y-down, n×n. */
    internal fun screenOf(u: Int, v: Int, n: Int, rotation: Int): Pair<Int, Int> = when (rotation and 3) {
        1 -> (n - 1 - v) to u
        2 -> (n - 1 - u) to (n - 1 - v)
        3 -> v to (n - 1 - u)
        else -> u to v
    }

    /** RGBA readback → NHWC ImageNet-normalised float tensor (upright). */
    fun toTensor(rgba: ByteArray, n: Int, rotation: Int): FloatArray {
        require(rgba.size == n * n * 4) { "rgba ${rgba.size} != ${n * n * 4}" }
        val out = FloatArray(n * n * 3)
        for (v in 0 until n) for (u in 0 until n) {
            val (x, y) = screenOf(u, v, n, rotation)
            val src = ((n - 1 - y) * n + x) * 4 // GL rows are bottom-up
            val dst = (v * n + u) * 3
            for (c in 0..2) {
                val s = (rgba[src + c].toInt() and 0xFF) / 255f
                out[dst + c] = (s - MEAN[c]) / STD[c]
            }
        }
        return out
    }

    /**
     * Upright map (0..1, n×n: nearness, vegetation probability, …) → bytes in GL
     * row order of the screen image. [n] is the map's own size, which need not
     * be the readback's: the screen is covered proportionally either way.
     */
    fun toGlBytes(near: FloatArray, n: Int, rotation: Int): ByteArray {
        require(near.size == n * n) { "map ${near.size} != ${n * n}" }
        val out = ByteArray(n * n)
        for (v in 0 until n) for (u in 0 until n) {
            val (x, y) = screenOf(u, v, n, rotation)
            out[(n - 1 - y) * n + x] = (near[v * n + u].coerceIn(0f, 1f) * 255f + 0.5f).toInt().toByte()
        }
        return out
    }
}

/**
 * Builds the segmentation network's input from the live readback: the n×n
 * RGBA frame (GL rows, screen orientation) is turned upright for the device
 * rotation (same mapping as [PreviewTensor.toTensor]), ImageNet-normalised and
 * bilinearly upscaled to an m×m NCHW tensor (RGB planes).
 *
 * Holds its buffers (m² × 3 floats ≈ 12 MB at 1024²) and reuses them across
 * calls, so one instance must only be used from one coroutine at a time and
 * the returned array is only valid until the next [tensor] call.
 */
class UprightUpscaler(val n: Int, val m: Int) {
    init {
        require(n > 0 && m > 0) { "bad sizes $n → $m" }
    }

    private val planes = FloatArray(3 * n * n)
    private val out = FloatArray(3 * m * m)

    // separable bilinear taps (half-pixel centres, edges clamped), same for both axes
    private val i0 = IntArray(m)
    private val i1 = IntArray(m)
    private val wt = FloatArray(m)

    init {
        for (i in 0 until m) {
            val s = (i + 0.5f) * n / m - 0.5f
            val f = kotlin.math.floor(s)
            i0[i] = f.toInt().coerceIn(0, n - 1)
            i1[i] = (f.toInt() + 1).coerceIn(0, n - 1)
            wt[i] = s - f
        }
    }

    fun tensor(rgba: ByteArray, rotation: Int): FloatArray {
        require(rgba.size == n * n * 4) { "rgba ${rgba.size} != ${n * n * 4}" }
        val nn = n * n
        for (v in 0 until n) for (u in 0 until n) {
            val (x, y) = PreviewTensor.screenOf(u, v, n, rotation)
            val src = ((n - 1 - y) * n + x) * 4 // GL rows are bottom-up
            for (c in 0..2) {
                val s = (rgba[src + c].toInt() and 0xFF) / 255f
                planes[c * nn + v * n + u] = (s - PreviewTensor.MEAN[c]) / PreviewTensor.STD[c]
            }
        }
        val mm = m * m
        for (c in 0..2) {
            val p = c * nn
            val o = c * mm
            for (v in 0 until m) {
                val r0 = p + i0[v] * n
                val r1 = p + i1[v] * n
                val wy = wt[v]
                for (u in 0 until m) {
                    val a = i0[u]
                    val b = i1[u]
                    val wx = wt[u]
                    val top = planes[r0 + a] + (planes[r0 + b] - planes[r0 + a]) * wx
                    val bot = planes[r1 + a] + (planes[r1 + b] - planes[r1 + a]) * wx
                    out[o + v * m + u] = top + (bot - top) * wy
                }
            }
        }
        return out
    }
}
