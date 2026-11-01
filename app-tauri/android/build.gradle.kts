plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "ai.sidevoice.app"
    // The Tauri template's (an app's gen/android/app/build.gradle.kts).
    compileSdk = 37

    defaultConfig {
        minSdk = 26
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_1_8
    }
}

dependencies {
    // Tauri's Kotlin API (Plugin, PermissionState); the CLI copies it to .tauri/tauri-api.
    implementation(project(":tauri-android"))
}
