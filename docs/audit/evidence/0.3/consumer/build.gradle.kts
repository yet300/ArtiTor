plugins {
    kotlin("multiplatform") version "2.4.20"
    id("com.android.kotlin.multiplatform.library") version "9.3.1"
    id("com.android.application") version "9.3.1" apply false
}
kotlin {
    jvmToolchain(21)
    android { namespace = "consumer.artitor03"; compileSdk = 37; minSdk = 26 }
    iosArm64()
    iosSimulatorArm64 {
        binaries.framework { baseName = "ArtiTor03Consumer" }
    }
    sourceSets {
        commonMain.dependencies { implementation("io.github.yet300:tor:0.3.0") }
    }
}
