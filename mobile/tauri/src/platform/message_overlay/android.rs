//! Android: the quick settings and the volume panel choose the output.
use super::Os;

pub struct Native;

impl Os for Native {
    const TEXTS: [(&'static str, &'static str); 11] = [
        ("en", include_str!("../../../i18n/android/en.json")),
        ("de", include_str!("../../../i18n/android/de.json")),
        ("es", include_str!("../../../i18n/android/es.json")),
        ("fr", include_str!("../../../i18n/android/fr.json")),
        ("it", include_str!("../../../i18n/android/it.json")),
        ("ja", include_str!("../../../i18n/android/ja.json")),
        ("pl", include_str!("../../../i18n/android/pl.json")),
        (
            "zh-Hans",
            include_str!("../../../i18n/android/zh-Hans.json"),
        ),
        ("pt-BR", include_str!("../../../i18n/android/pt-BR.json")),
        ("ko", include_str!("../../../i18n/android/ko.json")),
        ("hi", include_str!("../../../i18n/android/hi.json")),
    ];
}
