plugins {
    base
}

val qnxImage = providers.gradleProperty("qnx.rust.image")
    .orElse(providers.environmentVariable("QNX_RUST_IMAGE"))
    .orElse("qnx65-armv7-toolchain:8.5-rust")
val mibpkg = providers.gradleProperty("mibsdk.mibpkg")
val pluginStage = project(":plugins:mib2q-carmqtt:plugin").layout.buildDirectory.dir("mibpkg-stage")

tasks.register<Exec>("buildDaemon") {
    group = "native"
    description = "Cross-build carmqtt for QNX"
    workingDir(layout.projectDirectory)
    commandLine("bash", "scripts/build.sh")
    environment("QNX_IMAGE", qnxImage.get())
    // Create the package output before the Docker build: the build scripts run
    // as root and may create build/ themselves, leaving the runner unable to
    // mkdir build/mpk afterwards.
    doFirst { layout.buildDirectory.dir("mpk").get().asFile.mkdirs() }
}

tasks.register<Exec>("packageDaemon") {
    group = "native"
    description = "Build and package the carmqtt mibpkg package"
    dependsOn("buildDaemon", ":plugins:mib2q-carmqtt:plugin:mibpkgPackageStage")
    workingDir(layout.projectDirectory)
    val out = layout.buildDirectory.dir("mpk").get().asFile
    commandLine("bash", "scripts/package.sh", out.absolutePath)
    doFirst {
        val path = mibpkg.orNull
            ?: throw GradleException("mibsdk.mibpkg is not set: pass -Pmibsdk.mibpkg=/path/to/mibpkg")
        environment("MIBPKG", path)
        environment("PLUGIN_STAGE", pluginStage.get().asFile.absolutePath)
        out.mkdirs()
    }
}
