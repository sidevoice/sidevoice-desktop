//! Android: the name the person gave the phone (`Settings.Global` `device_name`, API 25+), else the Bluetooth name
//! (`Settings.Secure` `bluetooth_name`, when it can be read without a permission), else the model's human name
//! (`Build.MANUFACTURER` and `Build.MODEL`). No permission is asked. The JVM and the application are the ones Tauri's
//! runtime puts in `ndk_context` (as `platform/tls/android.rs` uses them).
use super::{model_name, Os};
use jni::{
    objects::{JObject, JString, JValue},
    JNIEnv, JavaVM,
};

pub struct Native;

/// A string setting of the given `Settings` class (`Global` or `Secure`), none when it is unset, blank or unreadable.
fn setting(env: &mut JNIEnv, resolver: &JObject, class: &str, key: &str) -> Option<String> {
    let key = env.new_string(key).ok()?;
    let value = env
        .call_static_method(
            class,
            "getString",
            "(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(resolver), JValue::Object(&key)],
        )
        .and_then(|value| value.l());
    string(env, value)
}

/// A static `String` field of `android.os.Build`.
fn build(env: &mut JNIEnv, field: &str) -> Option<String> {
    let value = env
        .get_static_field("android/os/Build", field, "Ljava/lang/String;")
        .and_then(|value| value.l());
    string(env, value)
}

/// The Rust string of a Java one; a Java exception (a restricted setting) is cleared and counts as none.
fn string(env: &mut JNIEnv, value: jni::errors::Result<JObject>) -> Option<String> {
    match value {
        Ok(object) if !object.is_null() => {
            env.get_string(&JString::from(object)).ok().map(Into::into)
        }
        _ => {
            let _ = env.exception_clear();
            None
        }
    }
}

impl Os for Native {
    fn device_name() -> Option<String> {
        let android = ndk_context::android_context();
        // SAFETY: the pointers are the process's JVM and a global reference to the application, which tao made at the
        // activity's creation and keeps for the process's life (ndk_context's contract).
        let vm = unsafe { JavaVM::from_raw(android.vm().cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        // SAFETY: tao's global reference to the application, only borrowed (`JObject` deletes nothing when dropped).
        let application = unsafe { JObject::from_raw(android.context().cast()) };
        let resolver = env
            .call_method(
                &application,
                "getContentResolver",
                "()Landroid/content/ContentResolver;",
                &[],
            )
            .and_then(|resolver| resolver.l())
            .ok()?;
        let given = |name: Option<String>| name.filter(|name| !name.trim().is_empty());
        given(setting(
            &mut env,
            &resolver,
            "android/provider/Settings$Global",
            "device_name",
        ))
        .or_else(|| {
            given(setting(
                &mut env,
                &resolver,
                "android/provider/Settings$Secure",
                "bluetooth_name",
            ))
        })
        .or_else(|| {
            let manufacturer = build(&mut env, "MANUFACTURER").unwrap_or_default();
            let model = build(&mut env, "MODEL").unwrap_or_default();
            model_name(&manufacturer, &model)
        })
    }
}
