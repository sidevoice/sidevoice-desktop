import groovy.json.JsonSlurper
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "ai.sidevoice.mobile.plugin"
    // The Tauri template's (gen/android/app/build.gradle.kts).
    compileSdk = 37

    defaultConfig {
        minSdk = 26
        // Merged into the app's R8 rules: the verifier's Kotlin is reached only through JNI, which R8 cannot see.
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

/**
 * The Kotlin component of rustls-platform-verifier, the certificate verifier the engine's HTTPS uses on Android
 * (mobile/tauri/src/platform/tls/android.rs; the crate's README, "Android"). It is not published to a Maven repository: it ships
 * as one inside the crate `rustls-platform-verifier-android`, whose directory `cargo metadata` names for the version
 * Cargo.lock resolves, so the Kotlin and the Rust are always of the same release. That repository holds one version, its
 * `release`.
 */
val rustlsPlatformVerifierMaven: File = run {
    val metadata = providers.exec {
        workingDir = projectDir
        commandLine(
            "cargo", "metadata", "--format-version", "1", "--filter-platform", "aarch64-linux-android",
            "--manifest-path", file("../../Cargo.toml").path
        )
    }.standardOutput.asText.get()
    @Suppress("UNCHECKED_CAST")
    val packages = (JsonSlurper().parseText(metadata) as Map<String, Any>)["packages"] as List<Map<String, Any>>
    val manifest = packages.first { it["name"] == "rustls-platform-verifier-android" }["manifest_path"] as String
    File(File(manifest).parentFile, "maven")
}
val rustlsPlatformVerifierVersion: String = run {
    val index = File(rustlsPlatformVerifierMaven, "rustls/rustls-platform-verifier/maven-metadata-local.xml").readText()
    Regex("<release>([^<]+)</release>").find(index)?.groupValues?.get(1)
        ?: throw GradleException("no release in rustls-platform-verifier's repository: $rustlsPlatformVerifierMaven")
}

// The app resolves this library's dependencies with its own repositories, and its project is generated (gen/android,
// never edited): the repository is added to every project from here.
rootProject.allprojects {
    repositories {
        maven {
            url = uri(rustlsPlatformVerifierMaven)
            metadataSources { mavenPom() }
        }
    }
}

dependencies {
    implementation("rustls:rustls-platform-verifier:$rustlsPlatformVerifierVersion")
    implementation("androidx.core:core-ktx:1.16.0")
    // The call's media session is the shared plugin's (sidevoice-shared-tauri's Android library, `MediaControls`), and the
    // service's notification shows it, through androidx.media's MediaStyle.
    implementation(project(":sidevoice-shared-tauri"))
    implementation("androidx.media:media:1.7.0")
    // The template's own version (gen/android/app/build.gradle.kts): the hosting activity's lifecycle is read from it.
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation(project(":tauri-android"))
    testImplementation("junit:junit:4.13.2")
}
