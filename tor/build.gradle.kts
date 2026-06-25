import gobley.gradle.GobleyHost
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.kotlinMultiplatform)
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.atomicfu)
    alias(libs.plugins.gobley.cargo)
    alias(libs.plugins.gobley.uniffi)
    alias(libs.plugins.vanniktech.mavenPublish)
}

// Maven Central namespace (io.github.<user> is auto-verified via the GitHub repo).
// The Kotlin package stays `com.yet.tor`; group and package need not match.
group = "io.github.yet300"
version = "0.1.0"

// The Rust crate lives outside this Gradle module.
cargo {
    packageDirectory = rootProject.layout.projectDirectory.dir("rust/arti-kmp-ffi")
}

uniffi {
    // proc-macro (setup_scaffolding!) crate: extract metadata from the built library.
    generateFromLibrary {
        namespace = "arti_kmp_ffi"
        packageName = "com.yet.tor.ffi"
    }
}

kotlin {
    androidTarget {
        compilerOptions {
            jvmTarget = JvmTarget.JVM_17
        }
    }
    jvmToolchain(21)

    // Required Apple targets. Built only on macOS hosts.
    if (GobleyHost.Platform.MacOS.isCurrent) {
        iosArm64()
        iosSimulatorArm64()
    }

    // Desktop / additional targets — scaffold. Arti has no raw-TCP path on the
    // web, so wasm is intentionally unsupported. To enable a desktop target,
    // declare it here and add the matching Rust target via rustup; Gobley wires
    // the rest. Kept off by default to keep the required matrix fast to build.
    //
    // jvm()
    // if (GobleyHost.Platform.MacOS.isCurrent) { macosArm64(); macosX64() }
    // if (GobleyHost.Platform.Linux.isCurrent) { linuxX64() }
    // if (GobleyHost.Platform.Windows.isCurrent) { mingwX64() }

    sourceSets {
        commonMain.dependencies {
            implementation(libs.kotlinx.coroutines.core)
        }
        commonTest.dependencies {
            implementation(libs.kotlin.test)
            implementation(libs.kotlinx.coroutines.core)
        }

        // On-device E2E proof (runs via :tor:connectedDebugAndroidTest).
        val androidInstrumentedTest by getting {
            dependencies {
                implementation(libs.kotlinx.coroutines.android)
                // okhttp 5.x needs compileSdk 36 (AGP 8.7 caps at 35); 4.x is fine for the test.
                implementation("com.squareup.okhttp3:okhttp:4.12.0")
                implementation("androidx.test:runner:1.6.2")
                implementation("androidx.test:core:1.6.1")
                implementation("androidx.test.ext:junit:1.2.1")
            }
        }
    }
}

android {
    namespace = "com.yet.tor"
    compileSdk = libs.versions.android.compileSdk.get().toInt()
    ndkVersion = "28.2.13676358"

    defaultConfig {
        minSdk = libs.versions.android.minSdk.get().toInt()
        // Full required ABI matrix. .so are bundled into the AAR (jniLibs) and
        // AGP merges them into the consumer APK automatically.
        ndk.abiFilters.addAll(listOf("arm64-v8a", "armeabi-v7a", "x86", "x86_64"))
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

java {
    toolchain {
        languageVersion = JavaLanguageVersion.of(21)
    }
}

mavenPublishing {
    // New Central Portal (central.sonatype.com tokens), not legacy OSSRH staging.
    publishToMavenCentral(com.vanniktech.maven.publish.SonatypeHost.CENTRAL_PORTAL)
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
