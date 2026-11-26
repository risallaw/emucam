package com.emucam.gl

import android.content.res.AssetManager
import android.graphics.SurfaceTexture
import android.opengl.EGLSurface
import android.opengl.GLES11Ext
import android.opengl.GLES30
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface
import androidx.camera.core.CameraEffect
import androidx.camera.core.SurfaceOutput
import androidx.camera.core.SurfaceProcessor
import androidx.camera.core.SurfaceRequest
import java.util.concurrent.Executor
import java.util.concurrent.atomic.AtomicReference
import androidx.core.util.Consumer

/**
 * CameraX SurfaceProcessor running the film/lens preview shaders on a
 * dedicated GL thread. Only applied to PREVIEW (never ImageCapture, which
 * would cap resolution and break zero-shutter-lag).
 */
class EmuSurfaceProcessor(
    private val assets: AssetManager,
    private val onError: (Throwable) -> Unit,
) : SurfaceProcessor {
    private val thread = HandlerThread("emucam-gl").apply { start() }
    private val handler = Handler(thread.looper)
    val executor: Executor = Executor { handler.post(it) }

    private val config = AtomicReference<PreviewConfig?>(null)
    private val live = AtomicReference(LiveParams())

    private var egl: EglCore? = null
    private var renderer: PreviewRenderer? = null
    private var oesTex = 0
    private var surfaceTexture: SurfaceTexture? = null
    private var inputSurface: Surface? = null
    private val outputs = LinkedHashMap<SurfaceOutput, EGLSurface>()
    private val texMatrix = FloatArray(16)
    private val outMatrix = FloatArray(16)
    private var released = false
    @Volatile private var failed = false

    /** Average GPU-side frame time (ms) for the debug HUD. */
    @Volatile var frameMs = 0f
        private set

    init {
        handler.post {
            try {
                egl = EglCore()
                renderer = PreviewRenderer(assets)
            } catch (t: Throwable) {
                fail(t)
            }
        }
    }

    fun setConfig(cfg: PreviewConfig) = config.set(cfg)
    fun setLive(p: LiveParams) = live.set(p)
    fun setTaps(n: Int) { renderer?.taps = n }

    override fun onInputSurface(request: SurfaceRequest) {
        if (released || failed) {
            request.willNotProvideSurface(); return
        }
        val e = egl ?: run { request.willNotProvideSurface(); return }
        e.makeCurrentPbuffer()
        if (oesTex == 0) {
            val ids = IntArray(1)
            GLES30.glGenTextures(1, ids, 0)
            oesTex = ids[0]
            GLES30.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, oesTex)
            GLES30.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES30.GL_TEXTURE_MIN_FILTER, GLES30.GL_LINEAR)
            GLES30.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES30.GL_TEXTURE_MAG_FILTER, GLES30.GL_LINEAR)
            GLES30.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES30.GL_TEXTURE_WRAP_S, GLES30.GL_CLAMP_TO_EDGE)
            GLES30.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES30.GL_TEXTURE_WRAP_T, GLES30.GL_CLAMP_TO_EDGE)
        }
        val st = SurfaceTexture(oesTex)
        st.setDefaultBufferSize(request.resolution.width, request.resolution.height)
        val surface = Surface(st)
        surfaceTexture?.release()
        surfaceTexture = st
        inputSurface = surface
        st.setOnFrameAvailableListener({ drawFrame(it) }, handler)
        request.provideSurface(surface, executor) {
            st.setOnFrameAvailableListener(null)
            surface.release()
            st.release()
            if (surfaceTexture === st) surfaceTexture = null
        }
    }

    override fun onOutputSurface(output: SurfaceOutput) {
        if (released) {
            output.close(); return
        }
        val e = egl ?: run { output.close(); return }
        val surface = output.getSurface(executor, Consumer { _: SurfaceOutput.Event ->
            outputs.remove(output)?.let { e.releaseSurface(it) }
            output.close()
        })
        outputs[output] = e.createWindowSurface(surface)
    }

    private fun drawFrame(st: SurfaceTexture) {
        if (released || failed) return
        val e = egl ?: return
        val r = renderer ?: return
        try {
            e.makeCurrentPbuffer()
            st.updateTexImage()
            st.getTransformMatrix(texMatrix)
            val cfg = config.get() ?: return
            val lp = live.get()
            for ((out, eglSurface) in outputs) {
                e.makeCurrent(eglSurface)
                out.updateTransformMatrix(outMatrix, texMatrix)
                val t0 = System.nanoTime()
                r.render(oesTex, outMatrix, out.size.width, out.size.height, cfg, lp)
                e.swap(eglSurface, st.timestamp)
                frameMs = frameMs * 0.9f + (System.nanoTime() - t0) / 1e6f * 0.1f
            }
        } catch (t: Throwable) {
            fail(t)
        }
    }

    private fun fail(t: Throwable) {
        if (failed) return
        failed = true
        Log.e(TAG, "preview pipeline failed", t)
        onError(t)
    }

    /** Release GL resources and the thread. Call after unbinding the effect. */
    fun release() {
        handler.post {
            released = true
            try {
                egl?.makeCurrentPbuffer()
                renderer?.release()
                outputs.forEach { (o, s) -> egl?.releaseSurface(s); o.close() }
                outputs.clear()
                if (oesTex != 0) GLES30.glDeleteTextures(1, intArrayOf(oesTex), 0)
                egl?.release()
            } catch (t: Throwable) {
                Log.w(TAG, "release", t)
            }
            thread.quitSafely()
        }
    }

    companion object {
        private const val TAG = "EmuSurfaceProcessor"
    }
}

/** CameraEffect's constructor is protected — expose it for PREVIEW. */
class EmuPreviewEffect(processor: EmuSurfaceProcessor, onError: (Throwable) -> Unit) :
    CameraEffect(PREVIEW, processor.executor, processor, Consumer<Throwable> { onError(it) })
