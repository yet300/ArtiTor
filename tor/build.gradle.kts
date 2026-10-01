import ch.ubique.uniffi.plugin.model.RustHost
import ch.ubique.uniffi.plugin.tasks.CargoBuildTask
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.kotlinMultiplatform)
    alias(libs.plugins.android.kotlin.multiplatform.library)
    alias(libs.plugins.kotlin.atomicfu)
    alias(libs.plugins.ubique.uniffi)
    alias(libs.plugins.vanniktech.mavenPublish)
}

// Maven Central namespace (io.github.<user> is auto-verified via the GitHub repo).
// The Kotlin package stays `com.yet.tor`; group and package need not match.
group = "io.github.yet300"
version = "0.3.0"

// The Rust crate lives outside this Gradle module.
cargo {
    packageDirectory = rootProject.layout.projectDirectory.dir("rust/arti-kmp-ffi")
    ndkVersion = "28.2.13676358"
}

// The bundled libsqlite3-sys exports the full sqlite3 API as global symbols; in a static
// Apple link they shadow any other SQLite the consumer links (e.g. SQLCipher), silently
// leaving the consumer's "encrypted" database plaintext. Prelink the staticlib and demote
// _sqlite3* to local symbols right after cargo produces it, before cinterop packs the klib.
tasks.withType<CargoBuildTask>().configureEach {
    doLast {
        val target = rustTarget.orNull ?: return@doLast
        if (!target.rustTriple.contains("apple")) return@doLast
        val archive = staticLibraryFile.orNull?.asFile ?: return@doLast
        if (!archive.exists()) return@doLast
        val script = rootProject.file("rust/hide-sqlite3-symbols.sh").absolutePath
        val process = ProcessBuilder(script, archive.absolutePath)
            .redirectErrorStream(true)
            .start()
        process.inputStream.copyTo(System.out)
        check(process.waitFor() == 0) { "hide-sqlite3-symbols.sh failed for $archive" }
    }
}

uniffi {
    // proc-macro (setup_scaffolding!) crate: extract metadata from the built library.
    // Package name comes from rust/arti-kmp-ffi/uniffi.toml (com.yet.tor.ffi).
    generateFromLibrary()
}

kotlin {
    jvmToolchain(21)

    android {
        namespace = "com.yet.tor"
        compileSdk = libs.versions.android.compileSdk.get().toInt()
        minSdk = libs.versions.android.minSdk.get().toInt()

        compilerOptions {
            jvmTarget.set(JvmTarget.JVM_17)
        }

        // Consumer R8 rules, published inside the AAR and applied automatically
        // to apps. The Ubique/UniFFI Android backend calls into the native
        // library via JNA and receives native->Kotlin callbacks (StatusListener),
        // both of which rely on reflection and must survive minification.
        @Suppress("UnstableApiUsage")
        optimization {
            consumerKeepRules.apply {
                publish = true
                file("consumer-rules.pro")
            }
        }

        withDeviceTest {
            instrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        }
    }

    // Required Apple targets. Built only on macOS hosts.
    if (RustHost.Platform.MacOS.isCurrent) {
        iosArm64()
        iosSimulatorArm64()
    }

    // Desktop / additional targets — scaffold. Arti has no raw-TCP path on the
    // web, so wasm is intentionally unsupported. To enable a desktop target,
    // declare it here and add the matching Rust target via rustup; Ubique wires
    // the rest. Kept off by default to keep the required matrix fast to build.
    //
    // jvm()
    // if (RustHost.Platform.MacOS.isCurrent) { macosArm64() }
    // if (RustHost.Platform.Linux.isCurrent) { linuxX64() }
    // if (RustHost.Platform.Windows.isCurrent) { mingwX64() }

    sourceSets {
        commonMain.dependencies {
            api(libs.kotlinx.coroutines.core)
        }
        // Compile the actual documentation sample against every test target.
        getByName("commonTest").kotlin.srcDir(rootProject.file("docs/examples"))
        commonTest.dependencies {
            implementation(libs.kotlin.test)
            implementation(libs.kotlinx.coroutines.core)
        }

        // On-device E2E proof (runs via :tor:connectedAndroidDeviceTest).
        val androidDeviceTest by getting {
            dependencies {
                implementation(libs.kotlinx.coroutines.android)
                // okhttp 5.x needs compileSdk 36 (now satisfied); 4.x is fine for the test.
                implementation("com.squareup.okhttp3:okhttp:4.12.0")
                implementation("androidx.test:runner:1.6.2")
                implementation("androidx.test:core:1.6.1")
                implementation("androidx.test.ext:junit:1.2.1")
            }
        }
    }
}

atomicfu {
    transformJvm = false
}

mavenPublishing {    // Central Portal (central.sonatype.com tokens), the only target in 0.33.x.
    publishToMavenCentral()
    // Sign only when a key is available (CI / release). Keeps publishToMavenLocal
    // and consumer integration via mavenLocal working without GPG configured.
    if (providers.gradleProperty("signingInMemoryKey").isPresent ||
        providers.gradleProperty("signing.keyId").isPresent ||
        providers.gradleProperty("signing.gnupg.keyName").isPresent
    ) {
        signAllPublications()
    }
    coordinates(group.toString(), "tor", version.toString())

    pom {
        name = "ArtiTor"
        description = "Kotlin Multiplatform wrapper over Arti (Tor in Rust) with first-class bootstrap status."
        inceptionYear = "2026"
        url = "https://github.com/yet300/ArtiTor"
        licenses {
            license {
                name = "The Apache License, Version 2.0"
                url = "https://www.apache.org/licenses/LICENSE-2.0.txt"
                distribution = "repo"
            }
        }
        developers {
            developer {
                id = "yet300"
                name = "yet300"
                url = "https://github.com/yet300"
            }
        }
        scm {
            url = "https://github.com/yet300/ArtiTor"
            connection = "scm:git:git://github.com/yet300/ArtiTor.git"
            developerConnection = "scm:git:ssh://git@github.com/yet300/ArtiTor.git"
        }
    }
}
