//! What the iOS app declares to Apple, checked on the app as built: `mobile check-ios APP` (the `.app` directory
//! `mobile build ios` leaves), and the last step of `mobile build ios` itself.
//!
//! - **Export compliance.** The app's `Info.plist` says whether it uses encryption beyond the system's
//!   (`ITSAppUsesNonExemptEncryption`). Without the key App Store Connect asks the question again for every build, and
//!   holds it from TestFlight until someone answers. What the key says is decided in mobile/Info.ios.plist.
//! - **Privacy manifest.** `PrivacyInfo.xcprivacy` is at the root of the bundle (mobile/PrivacyInfo.xcprivacy, which
//!   `mobile generate ios` puts in the project), says it does not track, and declares the reason for each category of
//!   required-reason API the executable calls. Apple refuses an upload that calls one without a declared reason
//!   (<https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api>).
//!
//! The calls are found in the executable's strings. A C function the app imports is a symbol (`_stat`); a Foundation
//! API a Swift or Objective-C file uses leaves its name in a symbol or a selector (`…FileAttributeKeyV12creationDate…`,
//! `systemUptime`). The first is matched whole, the second as a part of a string, without regard to case. This sees what
//! the executable calls, wherever it was compiled from (the Rust libraries are linked into it), and is made to err on
//! the side of more: a name that only looks like an API asks for a declaration, which a person then reads.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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
    /// An app directory (`Sidevoice.app`).
    pub fn from_app(app: &Path) -> Result<Bundle> {
        let read = |name: &str| fs::read(app.join(name)).map_err(|e| format!("{}: {e}", app.join(name).display()));
        let info = dictionary(&read("Info.plist")?, "Info.plist")?;
        let manifest = match app.join(MANIFEST).is_file() {
            true => Some(dictionary(&read(MANIFEST)?, MANIFEST)?),
            false => None,
        };
        let executable = info
            .get("CFBundleExecutable")
            .and_then(plist::Value::as_string)
            .ok_or("Info.plist: no CFBundleExecutable")?;
        Ok(Bundle { executable: read(executable)?, info, manifest })
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

/// The checks above, on a bundle already read.
pub fn check_bundle(bundle: &Bundle) -> Result<()> {
    ensure(
        bundle.info.get("ITSAppUsesNonExemptEncryption").is_some_and(|v| v.as_boolean().is_some()),
        "Info.plist does not say whether the app uses encryption beyond the system's (ITSAppUsesNonExemptEncryption, set in mobile/Info.ios.plist)",
    )?;
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

/// `mobile check-ios [APP]`.
pub fn check_app(app: &Path) -> Result<()> {
    ensure(app.is_dir(), format!("{}: no such app", app.display()))?;
    check_bundle(&Bundle::from_app(app)?)
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

    fn info(declared: bool) -> plist::Dictionary {
        match declared {
            true => dict("<dict><key>ITSAppUsesNonExemptEncryption</key><false/></dict>"),
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
