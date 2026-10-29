// Where Gradle finds the mibsdk build plugin and the SDK artifacts. mibsdk.repo (gradle.properties) is a Maven
// repository holding them: an SDK checkout's build/repo (file: URL) or a hosted copy.
pluginManagement {
    repositories {
        val sdkRepo = providers.gradleProperty("mibsdk.repo").orNull?.takeIf { it.isNotBlank() }
            ?: throw GradleException("mibsdk.repo is not set: put the URL of the mibsdk Maven repository in gradle.properties")
        maven { url = uri(sdkRepo) }
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        val sdkRepo = providers.gradleProperty("mibsdk.repo").orNull?.takeIf { it.isNotBlank() }
            ?: throw GradleException("mibsdk.repo is not set: put the URL of the mibsdk Maven repository in gradle.properties")
        maven { url = uri(sdkRepo) }
        mavenCentral()
    }
}

plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "0.8.0"
}

rootProject.name = "mib2q-plugins"
include("plugins:bike-rack", "plugins:hello-menu", "plugins:inputs-demo",
    "plugins:tailscale-status", "plugins:carmqtt-status")
