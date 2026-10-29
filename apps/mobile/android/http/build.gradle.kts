plugins { kotlin("jvm") }
kotlin { jvmToolchain(17) }
dependencies { implementation("com.squareup.okhttp3:okhttp:4.12.0") }
dependencyLocking { lockAllConfigurations() }
