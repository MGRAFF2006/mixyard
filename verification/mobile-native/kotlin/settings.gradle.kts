pluginManagement { repositories { mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositories { mavenCentral() } }
rootProject.name = "MixyardNativeControlQualification"
include(":shipping-http")
project(":shipping-http").projectDir = file("../../../apps/mobile/android/http")
