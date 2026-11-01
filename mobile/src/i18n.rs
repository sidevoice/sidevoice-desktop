//! The mobile app's own texts, the ones the page shows but the app words (docs/MOBILE.md -> "The bridge"): how to allow a
//! blocked microphone on this phone, and the button that opens that settings page (D-HOST-OS: each app words its own
//! system's help). One table per language, English the fallback; adding a text means adding its key to English.

/// The languages the app words its texts in.
pub const LANGUAGES: [&str; 2] = ["en", "es"];

const EN: &[(&str, &str)] = &[
    ("microphone.help_ios", "Open Settings › Sidevoice and turn on Microphone. Then check again."),
    (
        "microphone.help_android",
        "Open the app’s settings › Permissions › Microphone and choose “Allow only while using the app”. Then check again.",
    ),
    ("microphone.open_settings_ios", "Open Settings"),
    ("microphone.open_settings_android", "Open app settings"),
];

const ES: &[(&str, &str)] = &[
    ("microphone.help_ios", "Abre Ajustes › Sidevoice y activa Micrófono. Después vuelve a comprobar."),
    (
        "microphone.help_android",
        "Abre los ajustes de la app › Permisos › Micrófono y elige «Permitir solo mientras se usa la app». Después vuelve a comprobar.",
    ),
    ("microphone.open_settings_ios", "Abrir Ajustes"),
    ("microphone.open_settings_android", "Abrir ajustes de la app"),
];

/// `key` in `language`, else in English; the key itself when English lacks it too (a test keeps that from happening).
pub fn t(language: &str, key: &'static str) -> &'static str {
    let table = match language {
        "es" => ES,
        _ => EN,
    };
    let find = |table: &[(&str, &'static str)]| {
        table.iter().find(|(k, _)| *k == key).map(|(_, text)| *text)
    };
    find(table).or_else(|| find(EN)).unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_english_key() {
        for (key, _) in EN {
            for language in LANGUAGES {
                let table = if language == "es" { ES } else { EN };
                assert!(
                    table.iter().any(|(k, _)| k == key),
                    "{language} lacks {key}"
                );
            }
        }
        assert_eq!(EN.len(), ES.len(), "a key only Spanish has");
    }

    #[test]
    fn an_unknown_language_is_english() {
        assert_eq!(t("de", "microphone.open_settings_ios"), "Open Settings");
        assert_eq!(t("es", "microphone.open_settings_ios"), "Abrir Ajustes");
    }
}
