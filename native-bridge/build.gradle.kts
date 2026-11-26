import org.gradle.api.tasks.Exec

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "com.emucam.nativebridge"
    compileSdk = 36
    ndkVersion = "27.2.12479018"
    defaultConfig { minSdk = 29 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs"))
}

kotlin { jvmToolchain(17) }

// ---------------------------------------------------------------- Rust build
// Builds rust/emucore-ffi with cargo-ndk into build/rustJniLibs/<abi>/.
val rustDir = rootProject.layout.projectDirectory.dir("rust")
val abis = (findProperty("emucam.abis") as String? ?: "arm64-v8a").split(',').map { it.trim() }.filter { it.isNotEmpty() }
val rustRelease = (findProperty("emucam.rustRelease") as String?)?.toBoolean() ?: false

fun cargoBin(name: String): String {
    val home = System.getProperty("user.home")
    val f = file("$home/.cargo/bin/$name")
    return if (f.exists()) f.absolutePath else name
}

val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Build emucore-ffi for Android ABIs with cargo-ndk"
    val outDir = layout.buildDirectory.dir("rustJniLibs")
    inputs.dir(rustDir.dir("emucore/src"))
    inputs.dir(rustDir.dir("emucore/presets"))
    inputs.dir(rustDir.dir("emucore-ffi/src"))
    inputs.files(rustDir.file("Cargo.toml"), rustDir.file("emucore/Cargo.toml"), rustDir.file("emucore-ffi/Cargo.toml"))
    inputs.property("abis", abis)
    inputs.property("release", rustRelease)
    outputs.dir(outDir)
    workingDir = rustDir.asFile
    val sdkDir = androidComponents.sdkComponents.sdkDirectory.get().asFile
    val ndkDir = File(sdkDir, "ndk/${android.ndkVersion}")
    environment("ANDROID_NDK_HOME", ndkDir.absolutePath)
    environment("ANDROID_NDK_ROOT", ndkDir.absolutePath)
    environment("PATH", "${System.getProperty("user.home")}/.cargo/bin:" + System.getenv("PATH"))
    val args = mutableListOf(cargoBin("cargo"), "ndk")
    abis.forEach { args += listOf("-t", it) }
    args += listOf("-P", "29", "-o", outDir.get().asFile.absolutePath, "build", "-p", "emucore-ffi")
    args += if (rustRelease) listOf("--release") else listOf("--profile", "android-dev")
    commandLine(args)
}

tasks.matching { it.name.startsWith("merge") && it.name.endsWith("JniLibFolders") }.configureEach {
    dependsOn(cargoNdkBuild)
}

dependencies {
    testImplementation(libs.junit)
}
