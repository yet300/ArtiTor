plugins { id("com.android.application") }
android {
    namespace = "consumer.artitor03.app"
    compileSdk = 37
    defaultConfig {
        applicationId = "consumer.artitor03.app"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "1"
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
        }
    }
}
dependencies { implementation("io.github.yet300:tor:0.3.0") }
