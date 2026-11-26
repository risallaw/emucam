package com.emucam.camera

import android.content.Context
import android.util.Log
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.FocusMeteringAction
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.Preview
import androidx.camera.core.UseCaseGroup
import androidx.camera.core.resolutionselector.AspectRatioStrategy
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleOwner
import com.emucam.gl.EmuPreviewEffect
import com.emucam.gl.EmuSurfaceProcessor
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.suspendCancellableCoroutine
import java.io.File
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

data class CameraState(
    val bound: Boolean = false,
    val lensFacing: Int = CameraSelector.LENS_FACING_BACK,
    /** Live film/lens preview failed on this device → plain preview fallback. */
    val effectError: String? = null,
    val error: String? = null,
)

/**
 * Owns CameraX: Preview (through the emucam GL effect) + full-resolution
 * ImageCapture sharing the same ViewPort, so what you frame is what develops.
 */
class CameraController(private val context: Context) {
    private val _state = MutableStateFlow(CameraState())
    val state: StateFlow<CameraState> = _state.asStateFlow()

    val processor = EmuSurfaceProcessor(context.assets) { t -> onEffectError(t) }
    private var provider: ProcessCameraProvider? = null
    private var camera: Camera? = null
    private var capture: ImageCapture? = null
    private var owner: LifecycleOwner? = null
    private var view: PreviewView? = null
    private var useEffect = true

    fun bind(lifecycleOwner: LifecycleOwner, previewView: PreviewView) {
        owner = lifecycleOwner
        view = previewView
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            provider = future.get()
            previewView.post { rebind() }
        }, ContextCompat.getMainExecutor(context))
    }

    fun switchCamera() {
        val next = if (_state.value.lensFacing == CameraSelector.LENS_FACING_BACK) CameraSelector.LENS_FACING_FRONT else CameraSelector.LENS_FACING_BACK
        _state.value = _state.value.copy(lensFacing = next)
        rebind()
    }

    private fun rebind() {
        val p = provider ?: return
        val o = owner ?: return
        val v = view ?: return
        val viewPort = v.viewPort ?: run { v.post { rebind() }; return }
        val selector = ResolutionSelector.Builder()
            .setAspectRatioStrategy(AspectRatioStrategy.RATIO_4_3_FALLBACK_AUTO_STRATEGY)
            .build()
        val preview = Preview.Builder().setResolutionSelector(selector).build().also {
            it.surfaceProvider = v.surfaceProvider
        }
        val cap = ImageCapture.Builder()
            .setResolutionSelector(selector)
            .setCaptureMode(ImageCapture.CAPTURE_MODE_ZERO_SHUTTER_LAG)
            .setJpegQuality(95)
            .build()
        val group = UseCaseGroup.Builder()
            .setViewPort(viewPort)
            .addUseCase(preview)
            .addUseCase(cap)
        if (useEffect) group.addEffect(EmuPreviewEffect(processor) { onEffectError(it) })
        val cameraSelector = CameraSelector.Builder().requireLensFacing(_state.value.lensFacing).build()
        try {
            p.unbindAll()
            camera = p.bindToLifecycle(o, cameraSelector, group.build())
            capture = cap
            _state.value = _state.value.copy(bound = true, error = null)
        } catch (t: Throwable) {
            Log.e(TAG, "bind failed", t)
            if (useEffect) {
                onEffectError(t)
            } else {
                _state.value = _state.value.copy(bound = false, error = t.message ?: "camera bind failed")
            }
        }
    }

    private fun onEffectError(t: Throwable) {
        Log.e(TAG, "preview effect error → plain preview", t)
        ContextCompat.getMainExecutor(context).execute {
            if (!useEffect) return@execute
            useEffect = false
            _state.value = _state.value.copy(effectError = t.message ?: t.javaClass.simpleName)
            rebind()
        }
    }

    /** Tap-to-focus at a normalized point of the preview (0..1). */
    fun focusAt(nx: Float, ny: Float) {
        val v = view ?: return
        val cam = camera ?: return
        val point = v.meteringPointFactory.createPoint(nx * v.width, ny * v.height)
        cam.cameraControl.startFocusAndMetering(FocusMeteringAction.Builder(point).build())
    }

    /** Capture a full-resolution JPEG to [file]. EXIF orientation is set by CameraX. */
    suspend fun takePicture(file: File): File = suspendCancellableCoroutine { cont ->
        val cap = capture ?: return@suspendCancellableCoroutine cont.resumeWithException(IllegalStateException("camera not ready"))
        val opts = ImageCapture.OutputFileOptions.Builder(file).apply {
            if (_state.value.lensFacing == CameraSelector.LENS_FACING_FRONT) {
                setMetadata(ImageCapture.Metadata().apply { isReversedHorizontal = true })
            }
        }.build()
        cap.takePicture(opts, ContextCompat.getMainExecutor(context), object : ImageCapture.OnImageSavedCallback {
            override fun onImageSaved(output: ImageCapture.OutputFileResults) = cont.resume(file)
            override fun onError(exception: ImageCaptureException) = cont.resumeWithException(exception)
        })
    }

    fun release() {
        provider?.unbindAll()
        processor.release()
    }

    companion object {
        private const val TAG = "CameraController"
    }
}
