//! The app's native texts (tray menu, call controls), keyed, one bundle per language, English the fallback for a
//! missing key or a language without a bundle (AGENTS.md). The language is the system's when there is a bundle for
//! it; the shell reads the system's preferred languages once at start and picks with [`language`].

/// The languages with a bundle.
pub const LANGUAGES: [&str; 2] = ["en", "es"];

/// The first of `preferred` (BCP 47 tags, the person's order) whose primary subtag has a bundle; English otherwise.
pub fn language<S: AsRef<str>>(preferred: &[S]) -> &'static str {
    preferred
        .iter()
        .filter_map(|tag| {
            let primary = tag.as_ref().split(['-', '_']).next()?.to_ascii_lowercase();
            LANGUAGES.iter().copied().find(|code| *code == primary)
        })
        .next()
        .unwrap_or("en")
}

/// The text for `key` in `language`, falling back to English, then to the key itself.
pub fn t(language: &str, key: &str) -> &'static str {
    let bundle: &[(&str, &'static str)] = match language {
        "es" => ES,
        _ => EN,
    };
    lookup(bundle, key).or_else(|| lookup(EN, key)).unwrap_or_else(|| leak(key))
}

fn lookup(bundle: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    bundle.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// An unknown key is a bug that shows the key; never a panic in a menu.
fn leak(key: &str) -> &'static str {
    Box::leak(key.to_string().into_boxed_str())
}

const EN: &[(&str, &str)] = &[
    ("tray.not_loaded", "Sidevoice · room not loaded"),
    ("tray.no_call", "No call"),
    ("tray.reconnecting", "Reconnecting…"),
    ("tray.in_call", "In a call"),
    ("tray.in_call_muted", "In a call · muted"),
    ("tray.mute", "Mute microphone"),
    ("tray.unmute", "Unmute microphone"),
    ("tray.hang_up", "Hang up"),
    ("tray.hide_call_controls", "Hide call controls"),
    ("tray.show_call_controls", "Show call controls"),
    ("tray.show", "Show Sidevoice"),
    ("tray.settings", "Settings…"),
    ("tray.quit", "Quit Sidevoice"),
];

const ES: &[(&str, &str)] = &[
    ("tray.not_loaded", "Sidevoice · sala sin cargar"),
    ("tray.no_call", "Sin llamada"),
    ("tray.reconnecting", "Reconectando…"),
    ("tray.in_call", "En llamada"),
    ("tray.in_call_muted", "En llamada · silenciado"),
    ("tray.mute", "Silenciar micrófono"),
    ("tray.unmute", "Activar micrófono"),
    ("tray.hang_up", "Colgar"),
    ("tray.hide_call_controls", "Esconder controles de llamada"),
    ("tray.show_call_controls", "Mostrar controles de llamada"),
    ("tray.show", "Mostrar Sidevoice"),
    ("tray.settings", "Ajustes…"),
    ("tray.quit", "Salir de Sidevoice"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_s_first_language_with_a_bundle_wins() {
        assert_eq!(language(&["es-ES", "en-US"]), "es");
        assert_eq!(language(&["de-DE", "es_419"]), "es");
        assert_eq!(language(&["de-DE", "fr"]), "en", "English when nothing has a bundle");
        assert_eq!(language::<&str>(&[]), "en");
    }

    #[test]
    fn english_is_the_fallback() {
        assert_eq!(t("es", "tray.hang_up"), "Colgar");
        assert_eq!(t("de", "tray.hang_up"), "Hang up");
        assert_eq!(t("es", "no.such.key"), "no.such.key");
    }
}
