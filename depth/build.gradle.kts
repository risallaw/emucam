import java.net.URI
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "com.emucam.depth"
    compileSdk = 36
    defaultConfig { minSdk = 29 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    // models are memory-mapped/read whole; compressing them only costs time
    androidResources { noCompress += "tflite" }
    // DepthEstimator logs via android.util.Log; JVM tests get no-op stubs
    testOptions { unitTests.isReturnDefaultValues = true }
}

kotlin { jvmToolchain(17) }

dependencies {
    implementation(libs.litert)
    implementation(libs.litert.gpu)
    implementation(libs.kotlinx.coroutines.android)
    testImplementation(libs.junit)
}

/**
 * Depth networks (not committed: 120 MB). Downloaded once into the Gradle
 * user-independent cache under the root `.gradle/`, verified by SHA-256, and
 * added to this module's assets.
 *   - Depth Anything 3 Small (Apache-2.0) — capture develop
 *   - MiDaS v2.1 small (MIT)             — live preview
 *   - PIDNet-S Cityscapes (MIT)         — vegetation mask for infrared films
 */
val depthModels = mapOf(
    "da3_small_gpu_fp16.tflite" to (
        "https://huggingface.co/litert-community/Depth-Anything-3-Small/resolve/5cd25d936e3fde2edef68f53b4123401454ac9c8/da3_small_gpu_fp16.tflite" to
            "e170369a72ba1bba7486a4d2de555639fccd0595a9bb5b5349f7733ed4aebd1f"
        ),
    "midas_small_256_fp16.tflite" to (
        "https://huggingface.co/litert-community/MiDaS-small/resolve/60029dd423757afaed8f7a38fb4042b105ca1f6e/midas_small_256_fp16.tflite" to
            "bec9bce704789e504ec306196fcb0aabe90fd25c2b9d7db382339741950890ca"
        ),
    "pidnet_s.tflite" to (
        "https://huggingface.co/litert-community/PIDNet-S-Cityscapes-LiteRT/resolve/3d79fd7a49278c81649fbe180091f96feead56fd/pidnet_s.tflite" to
            "9f11e66b223eefb9a35ca70bf1451b6a0104623e4c178e422f7cf3eb72eace26"
        ),
)

abstract class FetchModels : DefaultTask() {
    @get:Input abstract val models: MapProperty<String, String>   // name -> "url sha256"
    @get:Internal abstract val cacheDir: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun fetch() {
        val cache = cacheDir.get().asFile.apply { mkdirs() }
        val out = outputDir.get().asFile.apply { mkdirs() }
        for ((name, spec) in models.get()) {
            val (url, sha) = spec.split(' ')
            val cached = File(cache, name)
            if (!cached.exists() || sha256(cached) != sha) {
                logger.lifecycle("emucam: downloading $name")
                val tmp = File(cache, "$name.part")
                try {
                    val conn = URI(url).toURL().openConnection().apply {
                        connectTimeout = 30_000
                        readTimeout = 60_000 // a stalled download fails instead of hanging the build
                    }
                    conn.getInputStream().use { input -> tmp.outputStream().use { input.copyTo(it) } }
                    val got = sha256(tmp)
                    if (got != sha) throw GradleException("$name: SHA-256 mismatch (expected $sha, got $got)")
                    // replace a stale/corrupt cached copy (File.renameTo fails silently on some platforms)
                    Files.move(tmp.toPath(), cached.toPath(), StandardCopyOption.REPLACE_EXISTING)
                } finally {
                    tmp.delete()
                }
            }
            cached.copyTo(File(out, name), overwrite = true)
        }
    }

    private fun sha256(f: File): String {
        val md = MessageDigest.getInstance("SHA-256")
        f.inputStream().use { s ->
            val buf = ByteArray(1 shl 16)
            while (true) {
                val n = s.read(buf)
                if (n < 0) break
                md.update(buf, 0, n)
            }
        }
        return md.digest().joinToString("") { "%02x".format(it) }
    }
}

val fetchModels = tasks.register<FetchModels>("fetchDepthModels") {
    models.set(depthModels.mapValues { (_, v) -> "${v.first} ${v.second}" })
    cacheDir.set(rootProject.layout.projectDirectory.dir(".gradle/emucam-models"))
    outputDir.set(layout.buildDirectory.dir("generated/depthModels"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.assets?.addGeneratedSourceDirectory(fetchModels, FetchModels::outputDir)
    }
}
