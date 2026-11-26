package com.emucam.depth

import android.content.Context
import android.util.Log
import com.google.ai.edge.litert.Accelerator
import com.google.ai.edge.litert.CompiledModel
import com.google.ai.edge.litert.TensorBuffer
import kotlinx.coroutines.cancel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.Closeable

/**
 * An on-device network. The depth models are monocular depth; [kind] tells the
 * Rust engine how to read their output (0 = depth, larger is farther;
 * 1 = inverse depth, larger is nearer). [kind] is unused for [SEGMENT].
 * Input tensors are [width]×[height] (portrait layout for depth models).
 * The output holds [classes] planes of [outW]×[outH]; depth models have one
 * plane the same size as the input, [SEGMENT] has 19 class logits at 128×128.
 */
enum class DepthModel(
    val asset: String,
    val width: Int,
    val height: Int,
    val nchw: Boolean,
    val kind: Int = 0,
    val classes: Int = 1,
    val outW: Int = width,
    val outH: Int = height,
) {
    /** Depth Anything 3 Small: detailed, ~250 ms on a flagship GPU. Used when developing. */
    CAPTURE("da3_small_gpu_fp16.tflite", 504, 896, nchw = true, kind = 0),

    /** MiDaS v2.1 small: coarse but a few ms. Used for the live viewfinder. */
    PREVIEW("midas_small_256_fp16.tflite", 256, 256, nchw = false, kind = 1),

    /** PIDNet-S Cityscapes: per-pixel class logits (19 classes, 128×128). Used to find real vegetation. */
    SEGMENT("pidnet_s.tflite", 1024, 1024, nchw = true, classes = 19, outW = 128, outH = 128),
    ;

    val inputSize get() = width * height * 3
    val outputSize get() = classes * outW * outH
}

/** One loaded network on one accelerator. Only used from the estimator's thread. */
internal interface DepthSession : Closeable {
    /** Raw output for [input]; throws if inference fails. */
    fun run(input: FloatArray): FloatArray
}

/** LiteRT [CompiledModel] with its I/O buffers. */
private class LiteRtSession(context: Context, model: DepthModel, acc: Accelerator) : DepthSession {
    private val compiled = CompiledModel.create(context.assets, model.asset, CompiledModel.Options(acc), null)
    private val inputs: List<TensorBuffer>
    private val outputs: List<TensorBuffer>

    init {
        try {
            inputs = compiled.createInputBuffers()
            outputs = compiled.createOutputBuffers()
        } catch (t: Throwable) {
            compiled.close()
            throw t
        }
    }

    override fun run(input: FloatArray): FloatArray {
        inputs[0].writeFloat(input)
        compiled.run(inputs, outputs)
        return outputs[0].readFloat()
    }

    override fun close() {
        inputs.forEach { it.close() }
        outputs.forEach { it.close() }
        compiled.close()
    }
}

/**
 * Runs one [DepthModel] with LiteRT. All LiteRT calls happen on a single
 * thread (CompiledModel is not documented as thread-safe). GPU is tried
 * first, then CPU; an accelerator that fails (to load or mid-inference) is
 * dropped for the rest of this estimator's life. The model is released after
 * [idleMillis] without use, since the capture network holds ~100 MB.
 *
 * [run] never throws for inference problems: it returns null, so callers
 * (the develop queue, the live viewfinder loop) degrade to no depth.
 */
class DepthEstimator internal constructor(
    val model: DepthModel,
    private val idleMillis: Long,
    private val open: (Accelerator) -> DepthSession,
) : Closeable {
    constructor(context: Context, model: DepthModel, idleMillis: Long = 60_000) :
        this(model, idleMillis, { acc -> LiteRtSession(context, model, acc) })

    /**
     * Serialises every LiteRT call. Invariant: code between suspension points
     * on this dispatcher never interleaves, so [run]'s body, the idle release
     * and [close]'s release can't overlap.
     */
    @OptIn(ExperimentalCoroutinesApi::class)
    private val thread = Dispatchers.Default.limitedParallelism(1)
    private val scope = CoroutineScope(SupervisorJob() + thread)
    private var session: DepthSession? = null
    private val accelerators = ArrayDeque(listOf(Accelerator.GPU, Accelerator.CPU))
    private var idleJob: Job? = null
    @Volatile private var closed = false

    /** Set when no accelerator can run the model; further calls fail fast. */
    @Volatile var unavailable: String? = null
        private set

    /** Run the network. Returns the raw output, or null if it can't run (now or ever: see [unavailable]). */
    suspend fun run(input: FloatArray): FloatArray? = withContext(thread) {
        require(input.size == model.inputSize) { "input ${input.size} != ${model.inputSize}" }
        // checked on the estimator thread: after close() nothing may be (re)loaded
        if (closed || unavailable != null) return@withContext null
        idleJob?.cancel()
        val s = session ?: load() ?: return@withContext null
        try {
            s.run(input).takeIf { it.size == model.outputSize }
        } catch (t: Throwable) {
            // e.g. GPU delegate failing at runtime: fall back to the next accelerator next time
            Log.w(TAG, "${model.name} on ${accelerators.firstOrNull()} failed; dropping it", t)
            release()
            accelerators.removeFirstOrNull()
            if (accelerators.isEmpty()) unavailable = "${model.asset}: inference failed on every accelerator"
            null
        } finally {
            if (session != null) idleJob = scope.launch {
                delay(idleMillis)
                release()
            }
        }
    }

    private fun load(): DepthSession? {
        while (accelerators.isNotEmpty()) {
            val acc = accelerators.first()
            try {
                val t0 = System.nanoTime()
                val s = open(acc)
                session = s
                Log.i(TAG, "${model.name} on $acc in ${(System.nanoTime() - t0) / 1_000_000} ms")
                return s
            } catch (t: Throwable) {
                Log.w(TAG, "${model.name}: $acc unavailable", t)
                accelerators.removeFirst()
            }
        }
        unavailable = "no accelerator could load ${model.asset}"
        return null
    }

    private fun release() {
        session?.close()
        session = null
    }

    /** Releases the model; later [run] calls return null. Safe to call more than once. */
    override fun close() {
        closed = true
        // release on the estimator thread (after any in-flight run), then stop the idle timer
        scope.launch { release() }.invokeOnCompletion { scope.cancel() }
    }

    companion object {
        private const val TAG = "DepthEstimator"
    }
}
