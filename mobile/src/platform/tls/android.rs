//! Android: the verifier is the Kotlin component `rustls-platform-verifier-android` ships (the Android library of
//! `mobile/plugin` adds it to the app's Gradle build), called through JNI with the application's context and class loader.
//! The JVM and the context are the ones Tauri's runtime (tao) puts in `ndk_context` when the activity starts, before the
//! app's own code runs: the application, not the activity, so they outlive any activity.
use super::Os;
use jni::{objects::JObject, JavaVM};

pub struct Native;

impl Os for Native {
    fn init() -> Result<(), String> {
        let android = ndk_context::android_context();
        // SAFETY: the pointers are the process's JVM and a global reference to the application, which tao made at the
        // activity's creation and keeps for the process's life (ndk_context's contract).
        let vm = unsafe { JavaVM::from_raw(android.vm().cast()) }
            .map_err(|e| format!("the JVM: {e}"))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("attaching to the JVM: {e}"))?;
        // SAFETY: as above; `init_with_env` takes its own global references and does not keep this local one.
        let context = unsafe { JObject::from_raw(android.context().cast()) };
        rustls_platform_verifier::android::init_with_env(&mut env, context)
            .map_err(|e| format!("the verifier: {e}"))
    }
}
