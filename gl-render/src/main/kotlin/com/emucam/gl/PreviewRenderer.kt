package com.emucam.gl

import android.content.res.AssetManager
import android.opengl.GLES11Ext
import android.opengl.GLES30
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Multi-pass preview pipeline (GL thread only):
 *  1 linearize (½ res)  camera OES → linear, lateral CA, highlight expand
 *  2 gather    (½ res)  bokeh, per-pixel kernel (LENS_BLUR only)
 *  3 down+blur (⅛ res)  wide blur for glow / halation / IR masks
 *  4 final     (full)   composite + film (LUTs, IR, halation, grain) → output
 */
internal class PreviewRenderer(assets: AssetManager) {
    private val shaders = ShaderLibrary(assets)
    private val quad = Quad()
    private val halfFloat: Boolean
    private var lin: Fbo? = null
    private var lens: Fbo? = null
    private var blurA: Fbo? = null
    private var blurB: Fbo? = null
    private var lutTex = IntArray(2)
    private var lutVersion = -1L
    private var frame = 0f

    /** Number of bokeh taps (lowered under thermal pressure). */
    @Volatile var taps = 48

    // ---- live depth
    private val depthTex = IntArray(1)
    private var depthVersion = -1L
    private var depthOn = false
    private var focusNear = 0f
    private var depthSrc: Fbo? = null
    private val readback: ByteBuffer = ByteBuffer.allocateDirect(DEPTH_SIZE * DEPTH_SIZE * 4).order(ByteOrder.nativeOrder())

    /**
     * Set (on the GL thread, by EmuSurfaceProcessor) when a live worker (depth,
     * segmentation) asked for a frame; the next render copies a [DEPTH_SIZE]²
     * sRGB RGBA image (GL row order) to [depthSink], which must not block (it
     * only trySends).
     */
    var wantDepthFrame = false
    var depthSink: ((ByteArray) -> Unit)? = null

    // ---- live vegetation (segmentation gate of the infrared masks)
    private val vegTex = IntArray(1)
    private var vegVersion = -1L
    private var vegOn = false

    init {
        val ext = GLES30.glGetString(GLES30.GL_EXTENSIONS) ?: ""
        halfFloat = ext.contains("GL_EXT_color_buffer_half_float") || ext.contains("GL_EXT_color_buffer_float")
        GLES30.glGenTextures(1, depthTex, 0)
        GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, depthTex[0])
        GLES30.glTexStorage2D(GLES30.GL_TEXTURE_2D, 1, GLES30.GL_R8, DEPTH_SIZE, DEPTH_SIZE)
        for ((k, v) in listOf(
            GLES30.GL_TEXTURE_MIN_FILTER to GLES30.GL_LINEAR, GLES30.GL_TEXTURE_MAG_FILTER to GLES30.GL_LINEAR,
            GLES30.GL_TEXTURE_WRAP_S to GLES30.GL_CLAMP_TO_EDGE, GLES30.GL_TEXTURE_WRAP_T to GLES30.GL_CLAMP_TO_EDGE,
        )) GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, k, v)
        GLES30.glGenTextures(1, vegTex, 0)
        GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, vegTex[0])
        GLES30.glTexStorage2D(GLES30.GL_TEXTURE_2D, 1, GLES30.GL_R8, LIVE_VEG_SIZE, LIVE_VEG_SIZE)
        for ((k, v) in listOf(
            GLES30.GL_TEXTURE_MIN_FILTER to GLES30.GL_LINEAR, GLES30.GL_TEXTURE_MAG_FILTER to GLES30.GL_LINEAR,
            GLES30.GL_TEXTURE_WRAP_S to GLES30.GL_CLAMP_TO_EDGE, GLES30.GL_TEXTURE_WRAP_T to GLES30.GL_CLAMP_TO_EDGE,
        )) GLES30.glTexParameteri(GLES30.GL_TEXTURE_2D, k, v)
        GLES30.glGenTextures(2, lutTex, 0)
        for (t in lutTex) {
            GLES30.glBindTexture(GLES30.GL_TEXTURE_3D, t)
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_3D, GLES30.GL_TEXTURE_MIN_FILTER, GLES30.GL_LINEAR)
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_3D, GLES30.GL_TEXTURE_MAG_FILTER, GLES30.GL_LINEAR)
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_3D, GLES30.GL_TEXTURE_WRAP_S, GLES30.GL_CLAMP_TO_EDGE)
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_3D, GLES30.GL_TEXTURE_WRAP_T, GLES30.GL_CLAMP_TO_EDGE)
            GLES30.glTexParameteri(GLES30.GL_TEXTURE_3D, GLES30.GL_TEXTURE_WRAP_R, GLES30.GL_CLAMP_TO_EDGE)
        }
    }

    private fun ensureTargets(w: Int, h: Int) {
        val hw = (w / 2).coerceAtLeast(1)
        val hh = (h / 2).coerceAtLeast(1)
        if (lin?.width == hw && lin?.height == hh) return
        listOfNotNull(lin, lens, blurA, blurB).forEach { it.release() }
        lin = Fbo(hw, hh, halfFloat, mipmapped = true)
        lens = Fbo(hw, hh, halfFloat)
        val bw = (w / 8).coerceAtLeast(1)
        val bh = (h / 8).coerceAtLeast(1)
        blurA = Fbo(bw, bh, halfFloat)
        blurB = Fbo(bw, bh, halfFloat)
    }

    private fun uploadLuts(cfg: PreviewConfig) {
        if (cfg.version == lutVersion) return
        val n = cfg.lutSize
        for ((i, buf) in listOf(cfg.lutDevelop, cfg.lutPrint).withIndex()) {
            buf.position(0)
            GLES30.glBindTexture(GLES30.GL_TEXTURE_3D, lutTex[i])
            GLES30.glTexImage3D(GLES30.GL_TEXTURE_3D, 0, GLES30.GL_RGBA16F, n, n, n, 0, GLES30.GL_RGBA, GLES30.GL_HALF_FLOAT, buf)
        }
        GlUtil.checkError("upload LUT")
        lutVersion = cfg.version
    }

    private fun Program.common(cfg: PreviewConfig, live: LiveParams, w: Int, h: Int, texMatrix: FloatArray) {
        use()
        for ((k, v) in cfg.uniforms) set(k, v)
        live.focusCenter?.let { set("uFocusCenter", it) }
        live.focusAngle?.let { setf("uFocusAngle", it) }
        setf("uOutSize", w.toFloat(), h.toFloat())
        setInt("uDepth", DEPTH_UNIT)
        setf("uDepthOn", if (depthOn) 1f else 0f)
        setf("uFocusNear", focusNear)
        set("uTexMatrix", texMatrix)
        setf("uTime", frame)
    }

    private fun bindTex(unit: Int, target: Int, tex: Int) {
        GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + unit)
        GLES30.glBindTexture(target, tex)
    }

    /**
     * Render one frame into the currently bound window surface of size [w]×[h].
     * [texMatrix] maps output uv → camera texture uv (from SurfaceOutput).
     */
    fun render(
        oesTex: Int, texMatrix: FloatArray, w: Int, h: Int, cfg: PreviewConfig, live: LiveParams,
        depth: DepthFrame?, veg: VegFrame? = null,
    ) {
        ensureTargets(w, h)
        uploadLuts(cfg)
        uploadDepth(depth, cfg, live.focusCenter ?: cfg.uniforms["uFocusCenter"])
        bindTex(DEPTH_UNIT, GLES30.GL_TEXTURE_2D, depthTex[0])
        uploadVeg(veg, cfg)
        frame += 1f
        val defs = cfg.defines
        val lensBlur = "LENS_BLUR" in defs
        val lin = lin!!
        val lens = lens!!
        val blurA = blurA!!
        val blurB = blurB!!

        // 1. linearize
        lin.bind()
        shaders.program("linearize.frag", defs).apply {
            common(cfg, live, w, h, texMatrix)
            bindTex(0, GLES11Ext.GL_TEXTURE_EXTERNAL_OES, oesTex); setInt("uCamera", 0)
        }
        quad.draw()

        // prefiltered levels: sparse bokeh taps read a pre-blurred source (no speckle)
        lin.generateMips()
        if (wantDepthFrame) captureDepthFrame(cfg, live, w, h, texMatrix)

        // 2. gather (bokeh)
        if (lensBlur) {
            lens.bind()
            shaders.program("gather.frag", defs).apply {
                common(cfg, live, w, h, texMatrix)
                bindTex(0, GLES30.GL_TEXTURE_2D, lin.texture); setInt("uSrc", 0)
                setf("uTaps", taps.toFloat())
            }
            quad.draw()
        }

        // 3. down + blur (two separable iterations → wide, smooth)
        blurA.bind()
        shaders.program("down.frag", defs).apply {
            common(cfg, live, w, h, texMatrix)
            bindTex(0, GLES30.GL_TEXTURE_2D, lin.texture); setInt("uSrc", 0)
            setf("uSrcTexel", 1f / lin.width, 1f / lin.height)
        }
        quad.draw()
        val blur = shaders.program("blur.frag", defs)
        repeat(2) {
            blurB.bind()
            blur.apply {
                common(cfg, live, w, h, texMatrix)
                bindTex(0, GLES30.GL_TEXTURE_2D, blurA.texture); setInt("uSrc", 0)
                setf("uDir", 1f / blurA.width, 0f)
            }
            quad.draw()
            blurA.bind()
            blur.apply {
                bindTex(0, GLES30.GL_TEXTURE_2D, blurB.texture); setInt("uSrc", 0)
                setf("uDir", 0f, 1f / blurB.height)
            }
            quad.draw()
        }

        // 4. final → output surface
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0)
        GLES30.glViewport(0, 0, w, h)
        shaders.program("final.frag", defs).apply {
            common(cfg, live, w, h, texMatrix)
            bindTex(0, GLES11Ext.GL_TEXTURE_EXTERNAL_OES, oesTex); setInt("uCamera", 0)
            bindTex(1, GLES30.GL_TEXTURE_2D, lens.texture); setInt("uLens", 1)
            bindTex(2, GLES30.GL_TEXTURE_2D, blurA.texture); setInt("uBlur", 2)
            if ("FX_OMNI" in defs) { bindTex(LIN_UNIT, GLES30.GL_TEXTURE_2D, lin.texture); setInt("uLin", LIN_UNIT) }
            bindTex(3, GLES30.GL_TEXTURE_3D, lutTex[0]); setInt("uLutDevelop", 3)
            bindTex(4, GLES30.GL_TEXTURE_3D, lutTex[1]); setInt("uLutPrint", 4)
            // always bound so the sampler never aliases unit 0 (OES) even when unused
            bindTex(VEG_UNIT, GLES30.GL_TEXTURE_2D, vegTex[0]); setInt("uVeg", VEG_UNIT)
            setf("uVegOn", if (vegOn) 1f else 0f)
            setf("uLutSize", cfg.lutSize.toFloat())
        }
        quad.draw()
        GlUtil.checkError("render")
    }

    private fun uploadDepth(d: DepthFrame?, cfg: PreviewConfig, focusCenter: FloatArray?) {
        val wanted = (cfg.uniforms["uDepthParams"]?.getOrNull(2) ?: 0f) > 0.5f
        depthOn = wanted && d != null && d.size == DEPTH_SIZE && d.near.size == DEPTH_SIZE * DEPTH_SIZE
        if (!depthOn || d == null) return
        focusNear = d.focusNear(focusCenter?.takeIf { it.size >= 2 } ?: floatArrayOf(0.5f, 0.5f))
        if (d.version == depthVersion) return
        GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, depthTex[0])
        GLES30.glPixelStorei(GLES30.GL_UNPACK_ALIGNMENT, 1)
        GLES30.glTexSubImage2D(
            GLES30.GL_TEXTURE_2D, 0, 0, 0, DEPTH_SIZE, DEPTH_SIZE,
            GLES30.GL_RED, GLES30.GL_UNSIGNED_BYTE, ByteBuffer.wrap(d.near),
        )
        depthVersion = d.version
    }

    /** Upload a new vegetation map; the gate is on only for IR films with a valid map. */
    private fun uploadVeg(v: VegFrame?, cfg: PreviewConfig) {
        vegOn = "IR" in cfg.defines && v != null && v.valid
        if (!vegOn || v == null || v.version == vegVersion) return
        GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, vegTex[0])
        GLES30.glPixelStorei(GLES30.GL_UNPACK_ALIGNMENT, 1)
        GLES30.glTexSubImage2D(
            GLES30.GL_TEXTURE_2D, 0, 0, 0, LIVE_VEG_SIZE, LIVE_VEG_SIZE,
            GLES30.GL_RED, GLES30.GL_UNSIGNED_BYTE, ByteBuffer.wrap(v.prob),
        )
        vegVersion = v.version
    }

    /** Downsample the linear scene to DEPTH_SIZE² sRGB and hand it to [depthSink]. */
    private fun captureDepthFrame(cfg: PreviewConfig, live: LiveParams, w: Int, h: Int, texMatrix: FloatArray) {
        val sink = depthSink ?: return
        val lin = lin ?: return
        val dst = depthSrc ?: Fbo(DEPTH_SIZE, DEPTH_SIZE, halfFloat = false).also { depthSrc = it }
        dst.bind()
        shaders.program("depthsrc.frag", emptyList()).apply {
            common(cfg, live, w, h, texMatrix)
            bindTex(0, GLES30.GL_TEXTURE_2D, lin.texture); setInt("uSrc", 0)
            // pick the mip whose texels match the output footprint (no aliasing)
            setf("uLod", kotlin.math.max(0f, kotlin.math.log2(lin.width.toFloat() / DEPTH_SIZE)))
        }
        quad.draw()
        readback.position(0)
        GLES30.glReadPixels(0, 0, DEPTH_SIZE, DEPTH_SIZE, GLES30.GL_RGBA, GLES30.GL_UNSIGNED_BYTE, readback)
        val bytes = ByteArray(readback.capacity())
        readback.position(0)
        readback.get(bytes)
        wantDepthFrame = false
        sink(bytes)
    }

    fun release() {
        depthSrc?.release()
        GLES30.glDeleteTextures(1, depthTex, 0)
        GLES30.glDeleteTextures(1, vegTex, 0)
        listOfNotNull(lin, lens, blurA, blurB).forEach { it.release() }
        GLES30.glDeleteTextures(2, lutTex, 0)
        shaders.release()
        quad.release()
    }

    companion object {
        const val DEPTH_SIZE = LIVE_DEPTH_SIZE
        private const val DEPTH_UNIT = 5
        /** Units 0–4: camera/lens/blur/LUTs in the final pass, 5: depth (all passes). */
        private const val VEG_UNIT = 6
        /** Mipmapped linear frame for the OMNI wand (final pass, FX_OMNI only). */
        private const val LIN_UNIT = 7
    }
}
