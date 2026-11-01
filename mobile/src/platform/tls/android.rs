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
        // SAFETY: the pointer is tao's global reference to the application (above), valid for the process's life. It is
        // only borrowed here, to make a local reference of this thread's own from it: `JObject` deletes nothing when
        // dropped, so tao's global stays as it is.
        let application = unsafe { JObject::from_raw(android.context().cast()) };
        let context = env
            .new_local_ref(&application)
            .map_err(|e| format!("the application context: {e}"))?;
        // `init_with_env` takes global references of its own (the context and its class loader) and keeps no local one.
        rustls_platform_verifier::android::init_with_env(&mut env, context)
            .map_err(|e| format!("the verifier: {e}"))
    }
}
