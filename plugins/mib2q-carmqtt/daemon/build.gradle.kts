plugins {
    base
}

val qnxImage = providers.gradleProperty("qnx.rust.image")
    .orElse(providers.environmentVariable("QNX_RUST_IMAGE"))
    .orElse("qnx65-armv7-toolchain:8.5-rust")
val mibpkg = providers.gradleProperty("mibsdk.mibpkg")

tasks.register<Exec>("buildDaemon") {
    group = "native"
    description = "Cross-build carmqtt for QNX"
    workingDir(layout.projectDirectory)
    commandLine("sh", "scripts/build.sh")
    environment("QNX_IMAGE", qnxImage.get())
}

tasks.register<Exec>("packageDaemon") {
    group = "native"
    description = "Build and package the carmqtt mibpkg package"
    dependsOn("buildDaemon")
    workingDir(layout.projectDirectory)
    val out = layout.buildDirectory.dir("mpk").get().asFile
    commandLine("sh", "scripts/package.sh", out.absolutePath)
    doFirst {
        val path = mibpkg.orNull
            ?: throw GradleException("mibsdk.mibpkg is not set: pass -Pmibsdk.mibpkg=/path/to/mibpkg")
        environment("MIBPKG", path)
        out.mkdirs()
    }
}
