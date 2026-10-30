plugins {
    id("org.jetbrains.kotlin.jvm")
    id("mibsdk.plugin")
}

dependencies {
    // This test drives the runtime and the fake car directly, beyond what the plugin-test harness exposes.
    testImplementation("dev.complex256.mibsdk:runtime:0.6.3")
    testImplementation("dev.complex256.mibsdk:adapter-fake:0.6.3")
}

// Tests run on the host JVM (the main source set is pinned to 1.8 by mibsdk.plugin).
tasks.named<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>("compileTestKotlin") {
    compilerOptions.jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
}
