package com.emucam.camera

import android.content.Context
import android.util.Log
import android.view.OrientationEventListener
import android.view.Surface
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
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
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
    /** Current / available zoom (CameraX switches physical lenses: ultra-wide < 1 < tele). */
    val zoom: Float = 1f,
    val minZoom: Float = 1f,
    val maxZoom: Float = 1f,
    /** 35 mm-equivalent focal length at zoom 1×. */
    val focal35: Float = CameraController.DEFAULT_FOCAL_35,
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
    /** Zoom the user / lens asked for; re-applied after every (re)bind. */
    private var wantedZoom: Float? = null
    private var wantedFocal: Int? = null

    /**
     * Physical device rotation (Surface.ROTATION_*). The activity is locked to
     * portrait, so we follow the sensor to save landscape shots upright.
     */
    @Volatile var deviceRotation: Int = Surface.ROTATION_0
        private set

    private val orientationListener = object : OrientationEventListener(context) {
        override fun onOrientationChanged(deg: Int) {
            if (deg == ORIENTATION_UNKNOWN) return
            val r = rotationFor(deg, deviceRotation)
            if (r != deviceRotation) {
                deviceRotation = r
                capture?.targetRotation = r
            }
        }
    }

    /** The accelerometer runs only while the UI is started, not for the ViewModel's whole life. */
    private val orientationGate = LifecycleEventObserver { _, event ->
        when (event) {
            Lifecycle.Event.ON_START -> if (orientationListener.canDetectOrientation()) orientationListener.enable()
            Lifecycle.Event.ON_STOP -> orientationListener.disable()
            else -> Unit
        }
    }

    fun bind(lifecycleOwner: LifecycleOwner, previewView: PreviewView) {
        owner?.lifecycle?.removeObserver(orientationGate)
        owner = lifecycleOwner
        view = previewView
        // replays ON_START if already started
        lifecycleOwner.lifecycle.addObserver(orientationGate)
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
        val preview = Preview.Builder()
            .setResolutionSelector(selector)
            // 24 fps viewfinder: the camera HAL's CPU cost scales with frame
            // rate; 30 fps kept it at ~110-170 % CPU (heat). CameraX picks the
            // closest range the device supports.
            .setTargetFrameRate(PREVIEW_FPS)
            .build().also {
            it.surfaceProvider = v.surfaceProvider
        }
        val cap = ImageCapture.Builder()
            .setResolutionSelector(selector)
            // not ZERO_SHUTTER_LAG: ZSL keeps a ring buffer of full 12 MP frames
            // streaming at 30 fps while the viewfinder is open (camera HAL at
            // ~226 % CPU, CPU ~88 °C after a few minutes on the Xiaomi 15 Ultra)
            .setCaptureMode(ImageCapture.CAPTURE_MODE_MINIMIZE_LATENCY)
            .setJpegQuality(95)
            .setTargetRotation(deviceRotation)
            .build()
        val group = UseCaseGroup.Builder()
            .setViewPort(viewPort)
            .addUseCase(preview)
            .addUseCase(cap)
        if (useEffect) group.addEffect(EmuPreviewEffect(processor) { onEffectError(it) })
        val cameraSelector = CameraSelector.Builder().requireLensFacing(_state.value.lensFacing).build()
        try {
            p.unbindAll()
            camera?.cameraInfo?.zoomState?.removeObservers(o)
            val cam = p.bindToLifecycle(o, cameraSelector, group.build())
            camera = cam
            capture = cap
            val z = cam.cameraInfo.zoomState.value
            _state.value = _state.value.copy(
                bound = true, error = null,
                focal35 = equivalentFocal(cam),
                zoom = z?.zoomRatio ?: 1f, minZoom = z?.minZoomRatio ?: 1f, maxZoom = z?.maxZoomRatio ?: 1f,
            )
            cam.cameraInfo.zoomState.observe(o) {
                val rangeKnown = _state.value.maxZoom > _state.value.minZoom
                _state.value = _state.value.copy(zoom = it.zoomRatio, minZoom = it.minZoomRatio, maxZoom = it.maxZoomRatio)
                if (!rangeKnown && it.maxZoomRatio > it.minZoomRatio) reapplyWanted()
            }
            Log.i(TAG, "bound: zoom ${z?.minZoomRatio}..${z?.maxZoomRatio}, 1× ≈ ${_state.value.focal35} mm")
            reapplyWanted()
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

    /**
     * Zoom ratio (clamped to what the camera offers). On multi-camera phones
     * CameraX moves between the physical lenses: < 1 is the ultra-wide, the
     * telephotos take over at their native ratios.
     */
    fun setZoom(ratio: Float) {
        wantedZoom = ratio
        wantedFocal = null
        val cam = camera ?: return
        val z = cam.cameraInfo.zoomState.value ?: return
        cam.cameraControl.setZoomRatio(applicableZoom(ratio, z.minZoomRatio, z.maxZoomRatio) ?: return)
    }

    private fun reapplyWanted() {
        val f = wantedFocal
        if (f != null) matchFocal(f) else wantedZoom?.let { setZoom(it) }
    }

    /** Frame like a [focalMm] (35 mm-equivalent) lens: the matching zoom. */
    fun matchFocal(focalMm: Int) {
        if (focalMm <= 0) return
        if (camera == null) {
            wantedFocal = focalMm
            return
        }
        setZoom(zoomForFocal(focalMm.toFloat(), _state.value.focal35))
        wantedFocal = focalMm
    }

    @androidx.annotation.OptIn(androidx.camera.camera2.interop.ExperimentalCamera2Interop::class)
    private fun equivalentFocal(cam: Camera): Float = runCatching {
        val info = androidx.camera.camera2.interop.Camera2CameraInfo.from(cam.cameraInfo)
        val focals = info.getCameraCharacteristic(android.hardware.camera2.CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)
        val size = info.getCameraCharacteristic(android.hardware.camera2.CameraCharacteristics.SENSOR_INFO_PHYSICAL_SIZE)
        if (focals == null || size == null) DEFAULT_FOCAL_35 else mainFocal35(focals, size.width, size.height)
    }.getOrDefault(DEFAULT_FOCAL_35)

    /** Tap-to-focus at a normalized point of the preview (0..1). */
    fun focusAt(nx: Float, ny: Float) {
        val v = view ?: return
        val cam = camera ?: return
        val point = v.meteringPointFactory.createPoint(nx * v.width, ny * v.height)
        cam.cameraControl.startFocusAndMetering(FocusMeteringAction.Builder(point).build())
    }

    /** Drop any tap-to-focus point: back to the camera's continuous autofocus. */
    fun resetFocus() {
        camera?.cameraControl?.cancelFocusAndMetering()
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
        owner?.lifecycle?.removeObserver(orientationGate)
        orientationListener.disable()
        provider?.unbindAll()
        processor.release()
    }

    companion object {
        private const val TAG = "CameraController"
        const val DEFAULT_FOCAL_35 = 24f

        /**
         * 35 mm-equivalent focal length of the 1× camera. A logical
         * multi-camera may list several focal lengths (one per physical
         * lens); 1× is the main camera, whose equivalent is the one nearest
         * the usual ~24 mm. Implausible results fall back to 24 mm.
         */
        fun mainFocal35(focalsMm: FloatArray, sensorW: Float, sensorH: Float): Float {
            val diag = kotlin.math.hypot(sensorW, sensorH)
            if (diag <= 0f || focalsMm.isEmpty()) return DEFAULT_FOCAL_35
            val eq = focalsMm.map { it * 43.27f / diag }.minBy { kotlin.math.abs(it - DEFAULT_FOCAL_35) }
            return if (eq in 15f..40f) eq else DEFAULT_FOCAL_35
        }

        /** [ratio] clamped into the camera's range, or null while that range is unknown or empty. */
        fun applicableZoom(ratio: Float, min: Float, max: Float): Float? =
            if (max > min && min > 0f) ratio.coerceIn(min, max) else null

        fun zoomForFocal(focalMm: Float, focal35: Float): Float = focalMm / focal35

        /**
         * Sensor degrees → Surface rotation, with ±10° hysteresis around the
         * 45° boundaries so the result doesn't flicker when held diagonally.
         */
        fun rotationFor(deg: Int, current: Int): Int {
            val centre = when (current) {
                Surface.ROTATION_270 -> 90
                Surface.ROTATION_180 -> 180
                Surface.ROTATION_90 -> 270
                else -> 0
            }
            val diff = ((deg - centre + 540) % 360) - 180
            if (kotlin.math.abs(diff) < 55) return current
            return when (deg) {
                in 45 until 135 -> Surface.ROTATION_270
                in 135 until 225 -> Surface.ROTATION_180
                in 225 until 315 -> Surface.ROTATION_90
                else -> Surface.ROTATION_0
            }
        }
    }
}

/** Viewfinder frame rate (see [CameraController.rebind]). */
private val PREVIEW_FPS = android.util.Range(24, 24)
