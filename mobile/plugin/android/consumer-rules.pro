# rustls-platform-verifier's Kotlin is called only from Rust, through JNI (its README, "Proguard").
-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }
