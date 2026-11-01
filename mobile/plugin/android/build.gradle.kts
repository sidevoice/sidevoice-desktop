plugins {
    id("com.android.library")
}

android {
    namespace = "ai.sidevoice.mobile.plugin"
    // The Tauri template's (gen/android/app/build.gradle.kts).
    compileSdk = 37

    defaultConfig {
        minSdk = 26
    }
}
