//! Evidence for the packaging layout migration; no native build or signing is needed.

use serde_json::Value;

use std::path::Path;

use crate::util::{channel_config, ensure, read_bytes, repo, write, Channel, Cmd};
use crate::Result;

fn moved(path: &str) -> String {
    match path {
        "Entitlements.plist" | "Info.plist" => format!("packaging/macos/{path}"),
        "icons/icon.icns" | "icons/macos-1024.png" => path.replace("icons/", "packaging/macos/"),
        "icons/icon.ico" => "packaging/windows/icon.ico".into(),
        _ if path.starts_with("icons/tray/") => path.replacen("icons/tray/", "packaging/shared/tray/", 1),
        _ if path.starts_with("icons/") => path.replacen("icons/", "packaging/linux/icons/", 1),
        _ if path.starts_with("dmg/") => format!("packaging/macos/{path}"),
        _ => path.into(),
    }
}

fn json(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

/// `packaging files DIR OUTPUT`: sorted relative paths of the produced bundles, including staged package contents.
/// Keep this beside CI's artifacts so before/after runs can be compared without rebuilding either revision.
pub fn files(dir: &str, output: &str) -> Result<()> {
    fn visit(root: &Path, dir: &Path, found: &mut Vec<String>) -> Result<()> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let meta = path.symlink_metadata().map_err(|e| e.to_string())?;
            found.push(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            if meta.is_dir() {
                visit(root, &path, found)?;
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    visit(Path::new(dir), Path::new(dir), &mut found)?;
    found.sort();
    ensure(!found.is_empty(), format!("no bundle files in {dir}"))?;
    let list = found.join("\n") + "\n";
    print!("{list}");
    write(Path::new(output), list)
}

/// `packaging compare REF`: compare the pre-split config and every moved asset with this checkout. Only resource
/// paths and fields ignored by the selected OS are normalized. Channel overrides are applied last, like the CLI. `REF`
/// predates `desktop/`: its app lives in `src-tauri/`.
pub fn compare(before: &str) -> Result<()> {
    let git = |args: &[&str]| Cmd::new("git").args(args).stdout();
    let original = git(&["show", &format!("{before}:src-tauri/tauri.conf.json")])?;
    let original = json(original.as_bytes())?;
    let base = json(&read_bytes(&repo().join("desktop/tauri.conf.json"))?)?;
    for (os, targets) in [("macos", vec!["app", "dmg"]), ("windows", vec!["nsis"]), ("linux", vec!["deb", "appimage"])]
    {
        let mut current = base.clone();
        json_patch::merge(&mut current, &json(&read_bytes(&repo().join(format!("desktop/tauri.{os}.conf.json")))?)?);
        let mut previous = original.clone();
        // Tauri filters the old combined list to the formats supported on the selected host.
        let supported: Vec<Value> = previous["bundle"]["targets"]
            .as_array()
            .ok_or("before config has no target array")?
            .iter()
            .filter(|v| targets.contains(&v.as_str().unwrap_or_default()))
            .cloned()
            .collect();
        previous["bundle"]["targets"] = supported.into();
        for icon in previous["bundle"]["icon"].as_array_mut().ok_or("before config has no icons")? {
            *icon = moved(icon.as_str().ok_or("icon is not a path")?).into();
        }
        for field in ["entitlements", "infoPlist"] {
            let value = previous["bundle"]["macOS"][field].as_str().ok_or("missing macOS plist")?;
            previous["bundle"]["macOS"][field] = moved(value).into();
        }
        if os != "macos" {
            previous["bundle"].as_object_mut().unwrap().remove("macOS");
        }
        for channel in [Channel::Dev, Channel::Release, Channel::Nightly] {
            let mut previous = previous.clone();
            let mut current = current.clone();
            if let Some(patch) = channel_config(channel, "0.2.2-nightly.17") {
                json_patch::merge(&mut previous, &json(patch.as_bytes())?);
                json_patch::merge(&mut current, &json(patch.as_bytes())?);
            }
            ensure(
                previous == current,
                format!("{os} {} config changed:\nbefore={previous}\nafter={current}", channel.name()),
            )?;
            println!("{os}/{}: unchanged {current}", channel.name());
        }
    }
    let files = git(&[
        "ls-tree",
        "-r",
        "--name-only",
        before,
        "--",
        "src-tauri/icons",
        "src-tauri/dmg",
        "src-tauri/Info.plist",
        "src-tauri/Entitlements.plist",
    ])?;
    let mut count = 0;
    for old in files.lines() {
        let new = format!("desktop/{}", moved(old.strip_prefix("src-tauri/").unwrap()));
        let old_hash = git(&["rev-parse", &format!("{before}:{old}")])?;
        let new_hash = git(&["hash-object", &new])?;
        ensure(old_hash == new_hash, format!("asset bytes changed: {old} -> {new}"))?;
        count += 1;
    }
    ensure(count > 0, "no assets compared")?;
    println!("{count} relocated assets: identical Git blob hashes (before {before})");
    Ok(())
}
