plugins { kotlin("jvm") version "2.1.20"; application }
kotlin { jvmToolchain(17) }
dependencies { implementation("org.json:json:20240303") }
application { mainClass.set("org.mixyard.mobile.NativeQualificationKt") }
// The workflow copies the unchanged production PinnedControl.kt and Endpoint.kt
// into build-owned sources. No Android SDK or replacement control client is used.
kotlin.sourceSets.main { kotlin.srcDir("build/production") }
