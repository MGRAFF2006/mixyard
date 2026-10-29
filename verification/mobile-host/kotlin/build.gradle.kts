plugins { kotlin("jvm") version "2.1.20"; application }
kotlin { jvmToolchain(17) }
dependencies { implementation("org.json:json:20240303") }
application { mainClass.set("org.mixyard.mobile.HostQualificationKt") }
kotlin.sourceSets.main {
    kotlin.srcDir("build/production")
    kotlin.srcDir("../../../apps/mobile/native/kotlin")
}
