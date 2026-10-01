import java.util.Properties

plugins { id("com.android.application") }
android {
    namespace = "consumer.artitor03.app"
    compileSdk = 37
    defaultConfig {
        applicationId = "consumer.artitor03.hardwaregate"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "0.3-gate"
        ndk { abiFilters += "arm64-v8a" }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildTypes { release {
        isMinifyEnabled = true
        signingConfig = signingConfigs.getByName("debug")
        proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
    } }
}
dependencies { implementation("io.github.yet300:tor:0.3.0") }

// Release regression: inspect the final DEX, using annotations in the resolved
// original AARs as the inventory. No keep rules or dependency overrides here.
tasks.register<Exec>("verifyJnaMetadata") {
    dependsOn("assembleRelease")
    doFirst {
        val sdkProperties = Properties()
        rootProject.file("local.properties").takeIf { it.exists() }?.inputStream()?.use { sdkProperties.load(it) }
        val sdk = System.getenv("ANDROID_HOME") ?: System.getenv("ANDROID_SDK_ROOT")
            ?: sdkProperties.getProperty("sdk.dir") ?: error("Android SDK path is required")
        val dexdump = file("$sdk/build-tools").listFiles().orEmpty()
            .map { it.resolve(if (System.getProperty("os.name").startsWith("Windows")) "dexdump.exe" else "dexdump") }
            .filter { it.isFile }.sortedBy { it.parentFile.name }.lastOrNull()
            ?: error("Install Android build-tools (dexdump required)")
        val aars = configurations.getByName("releaseRuntimeClasspath").resolvedConfiguration.resolvedArtifacts
            .filter { it.moduleVersion.id.group in setOf("io.github.yet300", "ch.ubique.uniffi") && it.file.extension == "aar" }
            .map { it.file }
        check(aars.isNotEmpty()) { "Original ArtiTor and UniFFI AARs must be available" }
        val output = layout.buildDirectory.file("reports/jna-metadata.json").get().asFile
        output.parentFile.mkdirs()
        val verifier = providers.gradleProperty("jnaMetadataVerifier").orNull
            ?: rootProject.file("../../scripts/verify_android_jna_metadata.py").absolutePath
        commandLine(listOf("python3", verifier,
            "--apk", layout.buildDirectory.file("outputs/apk/release/app-release.apk").get().asFile.absolutePath,
            "--mapping", layout.buildDirectory.file("outputs/mapping/release/mapping.txt").get().asFile.absolutePath,
            "--dexdump", dexdump.absolutePath, "--output", output.absolutePath) +
            aars.flatMap { listOf("--aar", it.absolutePath) })
    }
}
