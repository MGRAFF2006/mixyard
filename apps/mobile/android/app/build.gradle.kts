plugins { id("com.android.application"); kotlin("android") }
android {
    namespace = "org.mixyard.mobile"
    compileSdk = 35
    defaultConfig { applicationId = "org.mixyard.mobile"; minSdk = 28; targetSdk = 35; versionCode = 1; versionName = "0.1.0" }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
}
dependencies { implementation(project(":core")); implementation(project(":http")); implementation("androidx.webkit:webkit:1.12.1") }
dependencyLocking { lockAllConfigurations() }
