//! The name the person gave this phone (`host.deviceName`, docs/BRIDGE.md), which the page proposes as the device's name
//! instead of naming it from the webview's user agent. Each phone asks its own system (D-HOST-OS).
#[cfg_attr(target_os = "android", path = "device_name/android.rs")]
#[cfg_attr(target_os = "ios", path = "device_name/ios.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// The name the system keeps for the device; `None` where it has none.
    fn device_name() -> Option<String>;
}

/// The name as the page gets it: trimmed, and none when nothing is left.
fn given(raw: &str) -> Option<String> {
    let name = raw.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A human model name from what the maker and the model say: the maker capitalised, and not repeated when the model
/// already starts with it (`samsung` + `SM-A600FN` -> `Samsung SM-A600FN`; `Google` + `Google Pixel 8` -> `Google Pixel 8`).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn model_name(manufacturer: &str, model: &str) -> Option<String> {
    let (manufacturer, model) = (manufacturer.trim(), model.trim());
    if model.is_empty() {
        return given(manufacturer);
    }
    if manufacturer.is_empty()
        || model
            .to_lowercase()
            .starts_with(&manufacturer.to_lowercase())
    {
        return Some(model.to_owned());
    }
    let mut letters = manufacturer.chars();
    let first = letters.next()?.to_uppercase().collect::<String>();
    Some(format!("{first}{} {model}", letters.as_str()))
}

/// The name the person gave this phone, if the system has one.
pub fn device_name() -> Option<String> {
    Native::device_name().as_deref().and_then(given)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_name_is_none() {
        assert_eq!(given("  Galaxy de Ana \n"), Some("Galaxy de Ana".into()));
        assert_eq!(given(" "), None);
    }

    #[test]
    fn the_model_name_capitalises_the_maker_and_does_not_repeat_it() {
        assert_eq!(
            model_name("samsung", "SM-A600FN").as_deref(),
            Some("Samsung SM-A600FN")
        );
        assert_eq!(
            model_name("Google", "Pixel 8").as_deref(),
            Some("Google Pixel 8")
        );
        assert_eq!(
            model_name("Google", "Google Pixel 8").as_deref(),
            Some("Google Pixel 8")
        );
        assert_eq!(
            model_name("HUAWEI", "huawei P30").as_deref(),
            Some("huawei P30")
        );
        assert_eq!(model_name("", "Pixel 8").as_deref(), Some("Pixel 8"));
        assert_eq!(model_name("samsung", " ").as_deref(), Some("samsung"));
        assert_eq!(model_name("", ""), None);
    }
}
