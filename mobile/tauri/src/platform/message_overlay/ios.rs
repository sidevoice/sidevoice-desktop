//! iOS: Control Center chooses the output.
use super::Os;

pub struct Native;

impl Os for Native {
    const TEXTS: [(&'static str, &'static str); 11] = [
        ("en", include_str!("../../../i18n/ios/en.json")),
        ("de", include_str!("../../../i18n/ios/de.json")),
        ("es", include_str!("../../../i18n/ios/es.json")),
        ("fr", include_str!("../../../i18n/ios/fr.json")),
        ("it", include_str!("../../../i18n/ios/it.json")),
        ("ja", include_str!("../../../i18n/ios/ja.json")),
        ("pl", include_str!("../../../i18n/ios/pl.json")),
        ("zh-Hans", include_str!("../../../i18n/ios/zh-Hans.json")),
        ("pt-BR", include_str!("../../../i18n/ios/pt-BR.json")),
        ("ko", include_str!("../../../i18n/ios/ko.json")),
        ("hi", include_str!("../../../i18n/ios/hi.json")),
    ];
}
