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

dependencies {
    implementation("androidx.core:core-ktx:1.16.0")
    // The template's own version (gen/android/app/build.gradle.kts): the app is visible or it is not.
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    implementation(project(":tauri-android"))
}
