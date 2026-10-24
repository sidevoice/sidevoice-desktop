//! Native destructive-action confirmation. The reset transaction remains in `data`.

use std::future::Future;
use tauri::{AppHandle, Runtime};

#[cfg(target_os = "macos")]
#[path = "confirmation/macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "confirmation/windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "confirmation/linux.rs"]
mod imp;

pub use imp::NativeConfirmation;

/// The existing reset and uninstall prompts, with their original translations.
pub enum ConfirmationKind {
    DeleteAllData,
    Uninstall,
}

/// OS-owned confirmation, available independently of whether reset is offered on this host.
pub trait Confirmation {
    /// Asks using the system language (English fallback). Cancel is the default; only an explicit destructive
    /// button response accepts. Escape, closing, and a failure to display or receive a response cancel.
    fn confirm<R: Runtime>(app: &AppHandle<R>, kind: ConfirmationKind) -> impl Future<Output = bool> + Send;
}

impl ConfirmationKind {
    fn words(self, locale: &str) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Self::DeleteAllData => native_reset_words(locale),
            Self::Uninstall => native_uninstall_words(locale),
        }
    }
}

// Kept verbatim from data.rs: do not merge these eight languages into the tray's smaller catalogue.
fn native_reset_words(locale: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match locale.split(['-', '_']).next().unwrap_or("en") {
        "de" => ("Alle Sidevoice-Daten löschen?", "Der Sidevoice-Dienst wird deinstalliert; lokale App-Daten, Modelle und gespeicherte Schlüssel werden gelöscht. Eigene Profile, ein separat installierter Core, andere Geräte, Backups und diese .app bleiben erhalten. Dies kann nicht rückgängig gemacht werden.", "Abbrechen", "Alle Sidevoice-Daten löschen"),
        "es" => ("¿Borrar todos los datos de Sidevoice?", "Se desinstalará el servicio Sidevoice y se borrarán los datos locales de la app, los modelos y las claves guardadas. Permanecen los perfiles personalizados, un núcleo instalado por separado, otros dispositivos, las copias de seguridad y esta .app. No se puede deshacer.", "Cancelar", "Borrar todos los datos de Sidevoice"),
        "fr" => ("Supprimer toutes les données Sidevoice ?", "Le service Sidevoice sera désinstallé et les données locales de l’app, les modèles et les clés enregistrées seront supprimés. Les profils personnalisés, un cœur installé séparément, les autres appareils, les sauvegardes et cette .app restent. Cette action est irréversible.", "Annuler", "Supprimer toutes les données Sidevoice"),
        "it" => ("Eliminare tutti i dati di Sidevoice?", "Il servizio Sidevoice verrà disinstallato; i dati locali dell’app, i modelli e le chiavi salvate verranno eliminati. Restano i profili personalizzati, un core installato separatamente, gli altri dispositivi, i backup e questa .app. L’azione non può essere annullata.", "Annulla", "Elimina tutti i dati di Sidevoice"),
        "ja" => ("Sidevoice の全データを削除しますか？", "Sidevoice サービスをアンインストールし、アプリ内のデータ、モデル、保存したキーを削除します。カスタムプロファイル、別途インストールしたコア、ほかの端末、バックアップ、この .app は残ります。この操作は取り消せません。", "キャンセル", "Sidevoice の全データを削除"),
        "pl" => ("Usunąć wszystkie dane Sidevoice?", "Usługa Sidevoice zostanie odinstalowana, a lokalne dane aplikacji, modele i zapisane klucze usunięte. Niestandardowe profile, osobno zainstalowany rdzeń, inne urządzenia, kopie zapasowe i ta aplikacja .app pozostaną. Nie można tego cofnąć.", "Anuluj", "Usuń wszystkie dane Sidevoice"),
        "zh" => ("删除所有 Sidevoice 数据？", "将卸载 Sidevoice 服务，并删除本应用的本地数据、模型和保存的密钥。自定义配置、单独安装的核心、其他设备、备份和此 .app 应用程序包会保留。此操作无法撤销。", "取消", "删除所有 Sidevoice 数据"),
        _ => ("Delete all Sidevoice data?", "This uninstalls the Sidevoice service and deletes this app’s local data, models and saved keys. Custom profiles, a separately installed core, other devices, backups and this .app bundle remain. This cannot be undone.", "Cancel", "Delete all Sidevoice data"),
    }
}

fn native_uninstall_words(locale: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match locale.split(['-', '_']).next().unwrap_or("en") {
        "de" => ("Sidevoice deinstallieren?", "Alle Sidevoice-Daten werden gelöscht: der Sidevoice-Dienst, lokale App-Daten, Modelle und gespeicherte Schlüssel. Danach wird Sidevoice beendet und in den Papierkorb gelegt. Eigene Profile, ein separat installierter Core, andere Geräte und Backups bleiben erhalten. Dies kann nicht rückgängig gemacht werden.", "Abbrechen", "Sidevoice deinstallieren"),
        "es" => ("¿Desinstalar Sidevoice?", "Se borrarán todos los datos de Sidevoice: el servicio Sidevoice, los datos locales de la app, los modelos y las claves guardadas. Después Sidevoice se cerrará y se moverá a la Papelera. Permanecen los perfiles personalizados, un núcleo instalado por separado, otros dispositivos y las copias de seguridad. No se puede deshacer.", "Cancelar", "Desinstalar Sidevoice"),
        "fr" => ("Désinstaller Sidevoice ?", "Toutes les données Sidevoice seront supprimées : le service Sidevoice, les données locales de l’app, les modèles et les clés enregistrées. Sidevoice se fermera ensuite et sera placé dans la Corbeille. Les profils personnalisés, un cœur installé séparément, les autres appareils et les sauvegardes restent. Cette action est irréversible.", "Annuler", "Désinstaller Sidevoice"),
        "it" => ("Disinstallare Sidevoice?", "Verranno eliminati tutti i dati di Sidevoice: il servizio Sidevoice, i dati locali dell’app, i modelli e le chiavi salvate. Poi Sidevoice si chiuderà e verrà spostato nel Cestino. Restano i profili personalizzati, un core installato separatamente, gli altri dispositivi e i backup. L’azione non può essere annullata.", "Annulla", "Disinstalla Sidevoice"),
        "ja" => ("Sidevoice をアンインストールしますか？", "Sidevoice のすべてのデータ（Sidevoice サービス、アプリ内のデータ、モデル、保存したキー）を削除します。その後 Sidevoice は終了し、ゴミ箱に移動されます。カスタムプロファイル、別途インストールしたコア、ほかの端末、バックアップは残ります。この操作は取り消せません。", "キャンセル", "Sidevoice をアンインストール"),
        "pl" => ("Odinstalować Sidevoice?", "Wszystkie dane Sidevoice zostaną usunięte: usługa Sidevoice, lokalne dane aplikacji, modele i zapisane klucze. Następnie Sidevoice zamknie się i zostanie przeniesiony do Kosza. Niestandardowe profile, osobno zainstalowany rdzeń, inne urządzenia i kopie zapasowe pozostaną. Nie można tego cofnąć.", "Anuluj", "Odinstaluj Sidevoice"),
        "zh" => ("卸载 Sidevoice？", "将删除所有 Sidevoice 数据：Sidevoice 服务、本应用的本地数据、模型和保存的密钥。随后 Sidevoice 会退出并移到废纸篓。自定义配置、单独安装的核心、其他设备和备份会保留。此操作无法撤销。", "取消", "卸载 Sidevoice"),
        _ => ("Uninstall Sidevoice?", "This deletes all Sidevoice data: the Sidevoice service, this app’s local data, models and saved keys. Sidevoice then quits and moves itself to the Trash. Custom profiles, a separately installed core, other devices and backups remain. This cannot be undone.", "Cancel", "Uninstall Sidevoice"),
    }
}

#[cfg(test)]
mod tests {
    use super::ConfirmationKind;

    #[test]
    fn reset_confirmation_preserves_all_supported_languages() {
        for locale in ["en", "es", "de", "fr", "it", "ja", "pl", "zh-Hans"] {
            let (title, detail, cancel, delete) = ConfirmationKind::DeleteAllData.words(locale);
            assert!(!title.is_empty() && !detail.is_empty() && !cancel.is_empty() && !delete.is_empty());
            assert!(detail.contains("Sidevoice") && !detail.contains("connector"));
            assert_ne!(cancel, delete);
        }
        assert_eq!(ConfirmationKind::DeleteAllData.words("xx"), ConfirmationKind::DeleteAllData.words("en"));
    }

    #[test]
    fn native_uninstall_confirmation_names_the_trash_in_all_supported_languages() {
        for locale in ["en", "es", "de", "fr", "it", "ja", "pl", "zh-Hans"] {
            let (title, detail, cancel, uninstall) = ConfirmationKind::Uninstall.words(locale);
            assert!(!title.is_empty() && !detail.is_empty() && !cancel.is_empty() && !uninstall.is_empty());
            assert!(detail.contains("Sidevoice") && !detail.contains("connector"));
            assert_eq!(cancel, ConfirmationKind::DeleteAllData.words(locale).2);
            assert_ne!(title, ConfirmationKind::DeleteAllData.words(locale).0);
            assert_ne!(cancel, uninstall);
        }
        assert_eq!(ConfirmationKind::Uninstall.words("xx"), ConfirmationKind::Uninstall.words("en"));
        assert!(ConfirmationKind::Uninstall.words("en").1.contains("Trash"));
    }
}
