pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositories {
    exclusiveContent { forRepository { mavenLocal() }; filter { includeGroup("io.github.yet300") } }
    google(); mavenCentral()
} }
rootProject.name = "ArtiTorHardwareGate"
include(":app")
