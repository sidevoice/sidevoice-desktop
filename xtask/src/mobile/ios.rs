//! What the iOS app declares to Apple, checked on the app as built: `mobile check-ios APP` (the `.app` directory
//! `mobile build ios` leaves), and the last step of `mobile build ios` itself.
//!
//! - **Export compliance.** The app's `Info.plist` says whether it uses encryption beyond the system's
//!   (`ITSAppUsesNonExemptEncryption`). Without the key App Store Connect asks the question again for every build, and
//!   holds it from TestFlight until someone answers. What the key says is decided in mobile/tauri/Info.ios.plist.
//! - **Background modes.** `UIBackgroundModes` has `audio` (the call goes on with the screen locked) and `voip` (CallKit
//!   refuses every call of an app without it, as unentitled, and the call then has no call UI on the lock screen).
//! - **Privacy manifest.** `PrivacyInfo.xcprivacy` is at the root of the bundle (mobile/tauri/PrivacyInfo.xcprivacy, which
//!   `mobile generate ios` puts in the project), says it does not track, and declares the reason for each category of
//!   required-reason API the executable calls. Apple refuses an upload that calls one without a declared reason
//!   (<https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api>).
//!
//! `check_ipa` is the same check on the archive `mobile testflight` is about to upload, and what only an archive can
//! show: that it is signed, that it is the app and the build number it was asked to be, and that it carries the symbols
//! App Store Connect symbolicates TestFlight's crash reports with.
//!
//! The calls are found in the executable's strings. A C function the app imports is a symbol (`_stat`); a Foundation
//! API a Swift or Objective-C file uses leaves its name in a symbol or a selector (`…FileAttributeKeyV12creationDate…`,
//! `systemUptime`). The first is matched whole, the second as a part of a string, without regard to case. This sees what
//! the executable calls, wherever it was compiled from (the Rust libraries are linked into it), and is made to err on
//! the side of more: a name that only looks like an API asks for a declaration, which a person then reads.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::util::ensure;
use crate::Result;

/// How a required-reason API shows in an executable's strings.
#[derive(Clone, Copy)]
enum Shows {
    /// A C function the executable imports: the whole string `_name`.
    Function,
    /// A Foundation or UIKit name: a part of a symbol or of a selector, whatever its case.
    Name,
}

const FILE_TIMESTAMP: &str = "NSPrivacyAccessedAPICategoryFileTimestamp";
const SYSTEM_BOOT_TIME: &str = "NSPrivacyAccessedAPICategorySystemBootTime";
const DISK_SPACE: &str = "NSPrivacyAccessedAPICategoryDiskSpace";
const ACTIVE_KEYBOARDS: &str = "NSPrivacyAccessedAPICategoryActiveKeyboards";
const USER_DEFAULTS: &str = "NSPrivacyAccessedAPICategoryUserDefaults";

/// The required-reason APIs, and the categories Apple lists each under. `getattrlist` is listed under two: the
/// executable does not say which attribute it asks for, so a declaration of either satisfies the check.
const APIS: &[(Shows, &str, &[&str])] = &[
    (Shows::Function, "stat", &[FILE_TIMESTAMP]),
    (Shows::Function, "fstat", &[FILE_TIMESTAMP]),
    (Shows::Function, "fstatat", &[FILE_TIMESTAMP]),
    (Shows::Function, "lstat", &[FILE_TIMESTAMP]),
    (Shows::Function, "getattrlist", &[FILE_TIMESTAMP, DISK_SPACE]),
    (Shows::Function, "getattrlistbulk", &[FILE_TIMESTAMP]),
    (Shows::Function, "fgetattrlist", &[FILE_TIMESTAMP, DISK_SPACE]),
    (Shows::Function, "getattrlistat", &[FILE_TIMESTAMP, DISK_SPACE]),
    // FileAttributeKey.creationDate and .modificationDate, UIDocument.fileModificationDate, URLResourceKey
    // .creationDateKey and .contentModificationDateKey, and the NSFile… and NSURL… constants they are.
    (Shows::Name, "creationdate", &[FILE_TIMESTAMP]),
    (Shows::Name, "modificationdate", &[FILE_TIMESTAMP]),
    (Shows::Function, "mach_absolute_time", &[SYSTEM_BOOT_TIME]),
    (Shows::Name, "systemuptime", &[SYSTEM_BOOT_TIME]),
    (Shows::Function, "statfs", &[DISK_SPACE]),
    (Shows::Function, "statvfs", &[DISK_SPACE]),
    (Shows::Function, "fstatfs", &[DISK_SPACE]),
    (Shows::Function, "fstatvfs", &[DISK_SPACE]),
    // URLResourceKey.volumeAvailableCapacity…Key and .volumeTotalCapacityKey, FileAttributeKey.systemFreeSize and
    // .systemSize, and the NSURLVolume… and NSFileSystem… constants they are.
    (Shows::Name, "volumeavailablecapacity", &[DISK_SPACE]),
    (Shows::Name, "volumetotalcapacity", &[DISK_SPACE]),
    (Shows::Name, "systemfreesize", &[DISK_SPACE]),
    (Shows::Name, "nsfilesystemsize", &[DISK_SPACE]),
    (Shows::Name, "activeinputmodes", &[ACTIVE_KEYBOARDS]),
    (Shows::Name, "userdefaults", &[USER_DEFAULTS]),
];

/// The longest string taken for a name: a symbol or a selector, not a message or a path.
const LONGEST_NAME: usize = 200;

/// The required-reason APIs an executable calls: each string that names one, with the categories it is listed under.
fn required_reason_apis(executable: &[u8]) -> BTreeMap<String, &'static [&'static str]> {
    let mut found = BTreeMap::new();
    for string in executable.split(|byte| *byte == 0) {
        if string.len() < 3 || string.len() > LONGEST_NAME || !string.iter().all(|b| b.is_ascii_graphic()) {
            continue;
        }
        let string = String::from_utf8_lossy(string);
        let lower = string.to_ascii_lowercase();
        for (shows, name, categories) in APIS {
            let is_it = match shows {
                Shows::Function => string.strip_prefix('_') == Some(*name),
                Shows::Name => lower.contains(name),
            };
            if is_it {
                found.insert(string.clone().into_owned(), *categories);
            }
        }
    }
    found
}

/// What a built app has, read: its `Info.plist`, its privacy manifest (if any) and its executable.
pub struct Bundle {
    pub info: plist::Dictionary,
    pub manifest: Option<plist::Dictionary>,
    pub executable: Vec<u8>,
}

fn dictionary(bytes: &[u8], what: &str) -> Result<plist::Dictionary> {
    plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| format!("{what}: {e}"))?
        .into_dictionary()
        .ok_or_else(|| format!("{what}: not a dictionary"))
}

impl Bundle {
    /// A bundle whose files `read` gives by their path in it (`None` for a file that is not there).
    fn read(mut read: impl FnMut(&str) -> Result<Option<Vec<u8>>>) -> Result<Bundle> {
        let info = read("Info.plist")?.ok_or("no Info.plist")?;
        let info = dictionary(&info, "Info.plist")?;
        let manifest = read(MANIFEST)?.map(|bytes| dictionary(&bytes, MANIFEST)).transpose()?;
        let name = info.get("CFBundleExecutable").and_then(plist::Value::as_string);
        let name = name.ok_or("Info.plist: no CFBundleExecutable")?;
        let executable = read(name)?.ok_or_else(|| format!("no executable {name}"))?;
        Ok(Bundle { info, manifest, executable })
    }

    /// An app directory (`Sidevoice.app`).
    pub fn from_app(app: &Path) -> Result<Bundle> {
        Bundle::read(|name| match fs::read(app.join(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", app.join(name).display())),
        })
    }
}

/// The privacy manifest's name, at the root of the bundle.
pub const MANIFEST: &str = "PrivacyInfo.xcprivacy";

/// The categories a privacy manifest declares a reason for.
fn declared_categories(manifest: &plist::Dictionary) -> Result<BTreeSet<String>> {
    let mut declared = BTreeSet::new();
    let entries = manifest.get("NSPrivacyAccessedAPITypes").and_then(plist::Value::as_array);
    for entry in entries.ok_or("the privacy manifest has no NSPrivacyAccessedAPITypes array")? {
        let entry = entry.as_dictionary().ok_or("NSPrivacyAccessedAPITypes: an entry is not a dictionary")?;
        let category = entry
            .get("NSPrivacyAccessedAPIType")
            .and_then(plist::Value::as_string)
            .ok_or("NSPrivacyAccessedAPITypes: an entry has no NSPrivacyAccessedAPIType")?;
        let reasons = entry.get("NSPrivacyAccessedAPITypeReasons").and_then(plist::Value::as_array);
        ensure(reasons.is_some_and(|r| !r.is_empty()), format!("{category} is declared with no reason"))?;
        declared.insert(category.to_owned());
    }
    Ok(declared)
}

/// The background modes the call needs: `audio` keeps the app running while the call records and plays with the screen
/// locked, and `voip` is what CallKit asks of an app before it takes its calls (without it every start is refused as
/// unentitled, and the call has no lock-screen call UI).
const BACKGROUND_MODES: [&str; 2] = ["audio", "voip"];

/// The checks above, on a bundle already read.
pub fn check_bundle(bundle: &Bundle) -> Result<()> {
    ensure(
        bundle.info.get("ITSAppUsesNonExemptEncryption").is_some_and(|v| v.as_boolean().is_some()),
        "Info.plist does not say whether the app uses encryption beyond the system's (ITSAppUsesNonExemptEncryption, set in mobile/tauri/Info.ios.plist)",
    )?;
    let modes = bundle.info.get("UIBackgroundModes").and_then(plist::Value::as_array);
    let modes: Vec<&str> = modes.into_iter().flatten().filter_map(plist::Value::as_string).collect();
    for mode in BACKGROUND_MODES {
        ensure(
            modes.contains(&mode),
            format!("Info.plist's UIBackgroundModes has no `{mode}` (set in mobile/tauri/Info.ios.plist)"),
        )?;
    }
    let manifest = bundle.manifest.as_ref().ok_or(format!("{MANIFEST} is not at the root of the bundle"))?;
    ensure(
        manifest.get("NSPrivacyTracking").is_some_and(|v| v.as_boolean().is_some()),
        format!("{MANIFEST} does not say whether the app tracks (NSPrivacyTracking)"),
    )?;
    let declared = declared_categories(manifest)?;
    let called = required_reason_apis(&bundle.executable);
    let missing: Vec<String> = called
        .iter()
        .filter(|(_, categories)| !categories.iter().any(|c| declared.contains(*c)))
        .map(|(name, categories)| format!("  {name}: {}", categories.join(" or ")))
        .collect();
    ensure(
        missing.is_empty(),
        format!(
            "the executable calls required-reason APIs {MANIFEST} gives no reason for (mobile/{MANIFEST}):\n{}",
            missing.join("\n")
        ),
    )?;
    for category in &declared {
        if !called.values().any(|categories| categories.contains(&category.as_str())) {
            println!("note: {category} is declared and the executable shows no call of its APIs");
        }
    }
    println!(
        "{} required-reason API names in the executable, every category declared ({}); the encryption and tracking declarations are there",
        called.len(),
        declared.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

/// `mobile check-ios APP`.
pub fn check_app(app: &Path) -> Result<()> {
    ensure(app.is_dir(), format!("{}: no such app", app.display()))?;
    check_bundle(&Bundle::from_app(app)?)?;
    check_intent_metadata(|name| match fs::read(app.join(name)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", app.join(name).display())),
    })
}

/// The call's Live Activity's widget extension, inside the app (mobile/tauri/ios-project.yml, target SidevoiceActivity).
const EXTENSION: &str = "PlugIns/SidevoiceActivity.appex";

/// The Live Activity's buttons (`LiveActivityIntent`s, mobile/tauri/apple/Intents, compiled into the app and the extension).
const INTENTS: [&str; 5] = ["SetMicOff", "SkipReply", "CancelSend", "HangUp", "ToggleRoute"];

/// The module of the Live Activity's Swift package (mobile/tauri/apple/CallActivity), linked statically: no intent may be
/// declared there.
const PACKAGE_MODULE: &str = "CallActivity";

/// The App Intents metadata the system reads to run a button (`Metadata.appintents/extract.actionsdata`, in the app and
/// in the extension): every intent of [`INTENTS`] is listed, once, and is declared in the bundle's own code. Either way it
/// goes wrong, a tap on the locked phone does nothing, silently: an intent missing, listed twice (a double
/// `AppIntentsPackage`), or one of a statically linked package, whose metadata Xcode copies into the bundle with an
/// `includes` of the package in `extract.packagedata`, which the device's `linkd` cannot resolve (no framework holds the
/// package's metadata): it registers nothing for the app (`aggregateMetadataIsEmpty`) and every tap fails with "There is
/// no metadata for SetMicOff" (TestFlight build 26, iOS 26). The actions are read as an object keyed by their identifier or
/// an array of objects with an `identifier`; an identifier is the intent's name, alone or after its module
/// (`CallActivity.SetMicOff`); the module of its type is its `fullyQualifiedTypeName`'s.
fn check_intents(actions_data: &[u8], what: &str) -> Result<()> {
    let json: serde_json::Value = serde_json::from_slice(actions_data).map_err(|e| format!("{what}: {e}"))?;
    let actions: Vec<(String, Option<String>)> = match json.get("actions") {
        Some(serde_json::Value::Object(map)) => {
            map.iter().map(|(id, action)| (id.clone(), type_name(action))).collect()
        }
        Some(serde_json::Value::Array(list)) => {
            list.iter().filter_map(|a| Some((a.get("identifier")?.as_str()?.to_owned(), type_name(a)))).collect()
        }
        _ => return Err(format!("{what}: no actions in the App Intents metadata")),
    };
    for intent in INTENTS {
        let found: Vec<&(String, Option<String>)> =
            actions.iter().filter(|(id, _)| *id == intent || id.ends_with(&format!(".{intent}"))).collect();
        ensure(
            !found.is_empty(),
            format!("{what}: the App Intents metadata lacks {intent}: its button would do nothing"),
        )?;
        ensure(found.len() == 1, format!("{what}: the App Intents metadata lists {intent} {} times", found.len()))?;
        let package = format!("{PACKAGE_MODULE}.");
        let (id, type_name) = found[0];
        ensure(
            !id.starts_with(&package) && !type_name.as_deref().is_some_and(|t| t.starts_with(&package)),
            format!(
                "{what}: {intent} is declared in the statically linked {PACKAGE_MODULE} package: the device registers no \
                 metadata for the app (linkd: aggregateMetadataIsEmpty) and its button does nothing"
            ),
        )?;
    }
    Ok(())
}

/// The type an action of the metadata names (`fullyQualifiedTypeName`, `Module.Type`).
fn type_name(action: &serde_json::Value) -> Option<String> {
    action.get("fullyQualifiedTypeName").and_then(serde_json::Value::as_str).map(str::to_owned)
}

/// The App Intents packages a bundle's metadata includes (`extract.packagedata`): none may be, since the one package
/// there is linked statically and the device cannot resolve an include of it ([`check_intents`]).
fn check_packages(package_data: &[u8], what: &str) -> Result<()> {
    let json: serde_json::Value = serde_json::from_slice(package_data).map_err(|e| format!("{what}: {e}"))?;
    let includes = json.get("includes").and_then(serde_json::Value::as_array).map_or(0, Vec::len);
    ensure(
        includes == 0,
        format!(
            "{what}: the App Intents metadata includes {includes} package(s) ({}): a statically linked package's metadata \
             is not found on the device (linkd: aggregateMetadataIsEmpty), and no button runs",
            json.get("includes").map(ToString::to_string).unwrap_or_default()
        ),
    )
}

/// The buttons' App Intents metadata, in the app and in the extension, as `read` gives their files by their path in
/// the app (`None` for one that is not there).
fn check_intent_metadata(mut read: impl FnMut(&str) -> Result<Option<Vec<u8>>>) -> Result<()> {
    for bundle in ["", EXTENSION] {
        let metadata =
            if bundle.is_empty() { "Metadata.appintents".to_owned() } else { format!("{bundle}/Metadata.appintents") };
        let actions = format!("{metadata}/extract.actionsdata");
        check_intents(&read(&actions)?.ok_or_else(|| format!("no {actions}"))?, &actions)?;
        let packages = format!("{metadata}/extract.packagedata");
        if let Some(bytes) = read(&packages)? {
            check_packages(&bytes, &packages)?;
        }
    }
    Ok(())
}

/// An archive for App Store Connect (`.ipa`): its app is the bundle `identifier` at `build_number`, it is signed
/// (a provisioning profile embedded, a code signature sealed), the Live Activity's extension
/// is there at the same build number, it carries its symbols (`Symbols/*.symbols`, which the export writes from the
/// archive's dSYMs, `uploadSymbols` being on by default), and the app passes [`check_bundle`].
pub fn check_ipa(ipa: &Path, identifier: &str, build_number: u32) -> Result<()> {
    let file = fs::File::open(ipa).map_err(|e| format!("{}: {e}", ipa.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", ipa.display()))?;
    let names: Vec<String> =
        archive.file_names().filter_map(|name| name.ok().map(std::borrow::Cow::into_owned)).collect();
    let root = names
        .iter()
        .filter_map(|name| name.strip_prefix("Payload/")?.split('/').next())
        .find(|app| app.ends_with(".app"))
        .map(|app| format!("Payload/{app}/"))
        .ok_or_else(|| format!("{}: no Payload/<name>.app", ipa.display()))?;
    let bundle = Bundle::read(|name| {
        let path = format!("{root}{name}");
        let Ok(mut entry) = archive.by_name(&path) else { return Ok(None) };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| format!("{path}: {e}"))?;
        Ok(Some(bytes))
    })?;
    let text = |key: &str| bundle.info.get(key).and_then(plist::Value::as_string).unwrap_or_default().to_owned();
    ensure(
        text("CFBundleIdentifier") == identifier,
        format!("the app is {}, not {identifier}", text("CFBundleIdentifier")),
    )?;
    ensure(
        text("CFBundleVersion") == build_number.to_string(),
        format!("the build number is {}, not {build_number}", text("CFBundleVersion")),
    )?;
    // The Live Activity's extension, at the app's build number (App Store Connect refuses an extension whose version
    // differs from its app's).
    let appex = format!("{root}{EXTENSION}/Info.plist");
    let appex_info = {
        let mut entry = archive.by_name(&appex).map_err(|_| format!("{}: no {EXTENSION}", ipa.display()))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| format!("{appex}: {e}"))?;
        dictionary(&bytes, &appex)?
    };
    let appex_version = appex_info.get("CFBundleVersion").and_then(plist::Value::as_string).unwrap_or_default();
    ensure(
        appex_version == build_number.to_string(),
        format!("{EXTENSION}'s build number is {appex_version}, not {build_number}"),
    )?;
    let short = |info: &plist::Dictionary| {
        info.get("CFBundleShortVersionString").and_then(plist::Value::as_string).unwrap_or_default().to_owned()
    };
    ensure(
        short(&appex_info) == short(&bundle.info),
        format!("{EXTENSION}'s version is {:?}, not the app's {:?}", short(&appex_info), short(&bundle.info)),
    )?;
    for sealed in ["embedded.mobileprovision", "_CodeSignature/CodeResources"] {
        ensure(names.contains(&format!("{root}{sealed}")), format!("{}: not signed: no {sealed}", ipa.display()))?;
    }
    ensure(
        names.iter().any(|name| name.starts_with("Symbols/") && name.ends_with(".symbols")),
        format!("{}: no Symbols/*.symbols: TestFlight's crash reports would not be symbolicated", ipa.display()),
    )?;
    check_bundle(&bundle)?;
    check_intent_metadata(|name| {
        let path = format!("{root}{name}");
        let Ok(mut entry) = archive.by_name(&path) else { return Ok(None) };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| format!("{path}: {e}"))?;
        Ok(Some(bytes))
    })
    .map_err(|e| format!("{}: {e}", ipa.display()))?;
    println!("{}: {identifier} build {build_number}, signed", ipa.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(strings: &[&str]) -> Vec<u8> {
        strings.iter().flat_map(|s| s.bytes().chain([0])).collect()
    }

    fn dict(xml: &str) -> plist::Dictionary {
        dictionary(format!("<plist version=\"1.0\">{xml}</plist>").as_bytes(), "test").unwrap()
    }

    fn manifest(categories: &[&str]) -> plist::Dictionary {
        let entries: String = categories
            .iter()
            .map(|c| {
                format!(
                    "<dict><key>NSPrivacyAccessedAPIType</key><string>{c}</string>\
                     <key>NSPrivacyAccessedAPITypeReasons</key><array><string>C617.1</string></array></dict>"
                )
            })
            .collect();
        dict(&format!(
            "<dict><key>NSPrivacyTracking</key><false/><key>NSPrivacyAccessedAPITypes</key><array>{entries}</array></dict>"
        ))
    }

    /// The background modes a built app declares (Info.ios.plist's).
    const MODES: &str = "<key>UIBackgroundModes</key><array><string>audio</string><string>voip</string></array>";

    fn info(declared: bool) -> plist::Dictionary {
        match declared {
            true => dict(&format!("<dict><key>ITSAppUsesNonExemptEncryption</key><false/>{MODES}</dict>")),
            false => dict("<dict/>"),
        }
    }

    fn bundle(strings: &[&str], categories: &[&str]) -> Bundle {
        Bundle { info: info(true), manifest: Some(manifest(categories)), executable: executable(strings) }
    }

    #[test]
    fn a_function_is_found_by_its_whole_symbol() {
        let found = required_reason_apis(&executable(&[
            "_stat",
            "_fstat",
            "_state",
            "stat",
            "_stat_value",
            "_mach_absolute_time",
        ]));
        assert_eq!(found.keys().collect::<Vec<_>>(), ["_fstat", "_mach_absolute_time", "_stat"]);
    }

    #[test]
    fn a_foundation_name_is_found_inside_a_symbol_or_a_selector_in_any_case() {
        let found = required_reason_apis(&executable(&[
            "$s10Foundation16FileAttributeKeyV12creationDateACvgZ",
            "systemUptime",
            "NSFileModificationDate",
            "_OBJC_CLASS_$_NSUserDefaults",
            "a message with systemUptime in it",
        ]));
        assert_eq!(found.len(), 4, "{found:?}");
        assert_eq!(found["NSFileModificationDate"], [FILE_TIMESTAMP]);
        assert_eq!(found["_OBJC_CLASS_$_NSUserDefaults"], [USER_DEFAULTS]);
    }

    #[test]
    fn a_declared_category_passes_and_an_undeclared_one_names_what_called_it() {
        check_bundle(&bundle(&["_stat", "_fstat"], &[FILE_TIMESTAMP])).unwrap();
        let error = check_bundle(&bundle(&["_stat", "_mach_absolute_time"], &[FILE_TIMESTAMP])).unwrap_err();
        assert!(error.contains("_mach_absolute_time: NSPrivacyAccessedAPICategorySystemBootTime"), "{error}");
        assert!(!error.contains("_stat:"), "{error}");
    }

    #[test]
    fn a_call_listed_under_two_categories_needs_one_of_them() {
        check_bundle(&bundle(&["_getattrlist"], &[DISK_SPACE])).unwrap();
        check_bundle(&bundle(&["_getattrlist"], &[FILE_TIMESTAMP])).unwrap();
        assert!(check_bundle(&bundle(&["_getattrlist"], &[USER_DEFAULTS])).unwrap_err().contains(" or "));
    }

    #[test]
    fn no_calls_and_no_declarations_pass() {
        check_bundle(&bundle(&["_malloc", "main"], &[])).unwrap();
    }

    #[test]
    fn the_declarations_must_be_there() {
        let mut app = bundle(&[], &[]);
        app.info = info(false);
        assert!(check_bundle(&app).unwrap_err().contains("ITSAppUsesNonExemptEncryption"));
        app = bundle(&[], &[]);
        app.manifest = None;
        assert!(check_bundle(&app).unwrap_err().contains("not at the root"));
        app.manifest = Some(dict("<dict><key>NSPrivacyAccessedAPITypes</key><array/></dict>"));
        assert!(check_bundle(&app).unwrap_err().contains("NSPrivacyTracking"));
    }

    #[test]
    fn the_call_s_background_modes_must_be_there() {
        let mut app = bundle(&[], &[]);
        app.info = dict(
            "<dict><key>ITSAppUsesNonExemptEncryption</key><false/>\
             <key>UIBackgroundModes</key><array><string>audio</string></array></dict>",
        );
        let error = check_bundle(&app).unwrap_err();
        assert!(error.contains("no `voip`"), "{error}");
        app.info = dict("<dict><key>ITSAppUsesNonExemptEncryption</key><false/></dict>");
        assert!(check_bundle(&app).unwrap_err().contains("no `audio`"));
    }

    /// An archive with the files of an app below `Payload/Sidevoice.app/`, and those whose name starts with `/` at its root.
    fn ipa(dir: &Path, files: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
        use std::io::Write;
        let path = dir.join("Sidevoice.ipa");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for (name, bytes) in files {
            let name = name.strip_prefix('/').map_or_else(|| format!("Payload/Sidevoice.app/{name}"), str::to_owned);
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn app_files(identifier: &str, version: &str) -> Vec<(&'static str, Vec<u8>)> {
        let info = format!(
            "<plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>Sidevoice</string>\
             <key>CFBundleIdentifier</key><string>{identifier}</string>\
             <key>CFBundleVersion</key><string>{version}</string>\
             <key>CFBundleShortVersionString</key><string>0.1.0</string>\
             <key>ITSAppUsesNonExemptEncryption</key><false/>{MODES}</dict></plist>"
        );
        let manifest = "<plist version=\"1.0\"><dict><key>NSPrivacyTracking</key><false/>\
             <key>NSPrivacyAccessedAPITypes</key><array/></dict></plist>";
        vec![
            ("Info.plist", info.into_bytes()),
            (MANIFEST, manifest.as_bytes().to_vec()),
            ("Sidevoice", executable(&["main", "_malloc"])),
            ("embedded.mobileprovision", b"profile".to_vec()),
            ("_CodeSignature/CodeResources", b"seal".to_vec()),
            ("PlugIns/SidevoiceActivity.appex/Info.plist", appex_info(version)),
            ("Metadata.appintents/extract.actionsdata", actions_data(&INTENTS)),
            ("PlugIns/SidevoiceActivity.appex/Metadata.appintents/extract.actionsdata", actions_data(&INTENTS)),
            (SYMBOLS, b"symbols".to_vec()),
        ]
    }

    /// The app's symbols for App Store Connect, at the archive's root.
    const SYMBOLS: &str = "/Symbols/BCA28F76-4E7B-3FFD-A975-E69ECF5C4130.symbols";

    fn appex_info(version: &str) -> Vec<u8> {
        format!(
            "<plist version=\"1.0\"><dict><key>CFBundleVersion</key><string>{version}</string>\
             <key>CFBundleShortVersionString</key><string>0.1.0</string></dict></plist>"
        )
        .into_bytes()
    }

    fn check(files: &[(&str, Vec<u8>)], identifier: &str, build: u32) -> Result<()> {
        let dir = tempfile::tempdir().unwrap();
        check_ipa(&ipa(dir.path(), files), identifier, build)
    }

    fn without<'a>(files: &[(&'a str, Vec<u8>)], name: &str) -> Vec<(&'a str, Vec<u8>)> {
        files.iter().filter(|(n, _)| *n != name).cloned().collect()
    }

    /// The App Intents metadata listing `names`, as Xcode writes it for intents declared in the bundle's own module.
    fn actions_data(names: &[&str]) -> Vec<u8> {
        actions_in("Sidevoice", names)
    }

    /// The App Intents metadata listing `names`, declared in `module`.
    fn actions_in(module: &str, names: &[&str]) -> Vec<u8> {
        let actions: serde_json::Map<String, serde_json::Value> = names
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    serde_json::json!({ "identifier": n, "fullyQualifiedTypeName": format!("{module}.{n}") }),
                )
            })
            .collect();
        serde_json::json!({ "version": 1, "actions": actions }).to_string().into_bytes()
    }

    #[test]
    fn intents_of_the_statically_linked_package_are_refused() {
        let files = app_files("ai.sidevoice.mobile", "42");
        let mut app = without(&files, "Metadata.appintents/extract.actionsdata");
        app.push(("Metadata.appintents/extract.actionsdata", actions_in("CallActivity", &INTENTS)));
        let error = check(&app, "ai.sidevoice.mobile", 42).unwrap_err();
        assert!(error.contains("statically linked CallActivity package"), "{error}");
        // As TestFlight build 26 had it: the package included.
        let mut included = files.clone();
        included.push((
            "PlugIns/SidevoiceActivity.appex/Metadata.appintents/extract.packagedata",
            br#"{"includes":["12CallActivity0aB7IntentsV"],"version":1}"#.to_vec(),
        ));
        let error = check(&included, "ai.sidevoice.mobile", 42).unwrap_err();
        assert!(error.contains("includes 1 package"), "{error}");
        let mut none: Vec<(&str, Vec<u8>)> = files.clone();
        none.push(("Metadata.appintents/extract.packagedata", br#"{"includes":[],"version":1}"#.to_vec()));
        check(&none, "ai.sidevoice.mobile", 42).unwrap();
    }

    #[test]
    fn the_buttons_intents_must_be_in_the_metadata_once() {
        let files = app_files("ai.sidevoice.mobile", "42");
        let lacking = actions_data(&INTENTS[1..]);
        let mut app = without(&files, "Metadata.appintents/extract.actionsdata");
        app.push(("Metadata.appintents/extract.actionsdata", lacking));
        let error = check(&app, "ai.sidevoice.mobile", 42).unwrap_err();
        assert!(error.contains("lacks SetMicOff"), "{error}");
        let no_metadata = without(&files, "PlugIns/SidevoiceActivity.appex/Metadata.appintents/extract.actionsdata");
        assert!(check(&no_metadata, "ai.sidevoice.mobile", 42).unwrap_err().contains("appex/Metadata.appintents"));
        let listed: Vec<u8> = serde_json::json!({ "actions": [
            {"identifier": "SetMicOff"}, {"identifier": "SetMicOff"}, {"identifier": "SkipReply"},
            {"identifier": "CancelSend"}, {"identifier": "HangUp"}, {"identifier": "ToggleRoute"} ] })
        .to_string()
        .into_bytes();
        assert!(check_intents(&listed, "x").unwrap_err().contains("SetMicOff 2 times"));
    }
    #[test]
    fn a_signed_archive_of_the_app_at_the_build_passes() {
        check(&app_files("ai.sidevoice.mobile", "42"), "ai.sidevoice.mobile", 42).unwrap();
    }

    #[test]
    fn an_archive_of_another_app_or_build_or_unsigned_is_refused() {
        let files = app_files("ai.sidevoice.mobile", "42");
        assert!(check(&files, "ai.sidevoice.other", 42).unwrap_err().contains("not ai.sidevoice.other"));
        assert!(check(&files, "ai.sidevoice.mobile", 43).unwrap_err().contains("build number is 42, not 43"));
        let unsigned = without(&files, "embedded.mobileprovision");
        assert!(check(&unsigned, "ai.sidevoice.mobile", 42).unwrap_err().contains("not signed"));
        let no_manifest = without(&files, MANIFEST);
        assert!(check(&no_manifest, "ai.sidevoice.mobile", 42).unwrap_err().contains("not at the root"));
        assert!(check(&[], "ai.sidevoice.mobile", 42).unwrap_err().contains("no Payload"));
        let no_extension = without(&files, "PlugIns/SidevoiceActivity.appex/Info.plist");
        assert!(check(&no_extension, "ai.sidevoice.mobile", 42)
            .unwrap_err()
            .contains("no PlugIns/SidevoiceActivity.appex"));
        let mut stale = without(&files, "PlugIns/SidevoiceActivity.appex/Info.plist");
        stale.push(("PlugIns/SidevoiceActivity.appex/Info.plist", appex_info("0.1.0")));
        assert!(check(&stale, "ai.sidevoice.mobile", 42).unwrap_err().contains("build number is 0.1.0, not 42"));
        let no_symbols = without(&files, SYMBOLS);
        assert!(check(&no_symbols, "ai.sidevoice.mobile", 42).unwrap_err().contains("no Symbols/*.symbols"));
    }

    #[test]
    fn a_category_without_a_reason_is_refused() {
        let mut app = bundle(&[], &[]);
        app.manifest = Some(dict(
            "<dict><key>NSPrivacyTracking</key><false/><key>NSPrivacyAccessedAPITypes</key><array><dict>\
             <key>NSPrivacyAccessedAPIType</key><string>NSPrivacyAccessedAPICategoryFileTimestamp</string>\
             <key>NSPrivacyAccessedAPITypeReasons</key><array/></dict></array></dict>",
        ));
        assert!(check_bundle(&app).unwrap_err().contains("with no reason"));
    }
}
