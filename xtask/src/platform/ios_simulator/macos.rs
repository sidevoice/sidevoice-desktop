//! macOS: `xcrun simctl`, Xcode's simulator control.

use std::fs;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use super::Os;
use crate::util::{ensure, mkdir, Cmd};
use crate::Result;

pub struct Native;

/// How long the app has to draw its first page before the screenshot.
const SETTLE: Duration = Duration::from_secs(25);

/// A dotted version as numbers, to order versions (`26.5` after `26.10` is wrong as text).
fn version(text: &str) -> Vec<u32> {
    text.split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// The newest iPhone Pro Max device type and the newest available iOS runtime, from `simctl list ... -j`.
fn pick(types: &str, runtimes: &str) -> Result<(String, String)> {
    let types: serde_json::Value =
        serde_json::from_str(types).map_err(|e| format!("device types: {e}"))?;
    let runtimes: serde_json::Value =
        serde_json::from_str(runtimes).map_err(|e| format!("runtimes: {e}"))?;
    let number = |name: &str| -> Option<u32> {
        let rest = name.strip_prefix("iPhone ")?.strip_suffix(" Pro Max")?;
        rest.parse().ok()
    };
    let device = types["devicetypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some((number(t["name"].as_str()?)?, t["identifier"].as_str()?)))
        .max_by_key(|(n, _)| *n)
        .map(|(_, id)| id.to_owned())
        .ok_or("no iPhone Pro Max device type on this Mac")?;
    let runtime = runtimes["runtimes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["platform"] == "iOS" && r["isAvailable"] == true)
        .filter_map(|r| Some((version(r["version"].as_str()?), r["identifier"].as_str()?)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, id)| id.to_owned())
        .ok_or("no available iOS runtime on this Mac")?;
    Ok((device, runtime))
}

fn simctl() -> Cmd {
    Cmd::new("xcrun").arg("simctl")
}

/// The app run in both appearances on the device `udid`, the screenshots and logs into `out`.
fn run_app(udid: &str, app: &Path, bundle: &str, out: &Path) -> Result<()> {
    simctl().args(["boot", udid]).run()?;
    simctl().args(["bootstatus", udid, "-b"]).run()?;
    simctl().args(["install", udid]).arg(app).run()?;
    for mode in ["light", "dark"] {
        simctl().args(["ui", udid, "appearance", mode]).run()?;
        let _ = simctl().args(["terminate", udid, bundle]).attempt()?;
        simctl().args(["launch", udid, bundle]).run()?;
        sleep(SETTLE);
        let running = simctl()
            .args(["spawn", udid, "launchctl", "list"])
            .output()?;
        ensure(
            running.contains(&format!("UIKitApplication:{bundle}")),
            format!("the app is not running after launch ({mode})"),
        )?;
        simctl()
            .args(["io", udid, "screenshot"])
            .arg(out.join(format!("screen-{mode}.png")))
            .run()?;
        let _ = simctl().args(["terminate", udid, bundle]).attempt()?;
    }
    Ok(())
}

impl Os for Native {
    fn screenshots(zip: &Path, bundle: &str, out: &Path) -> Result<()> {
        mkdir(out)?;
        let work = tempfile::tempdir().map_err(|e| e.to_string())?;
        Cmd::new("unzip")
            .arg("-q")
            .arg(zip)
            .arg("-d")
            .arg(work.path())
            .run()?;
        let app = fs::read_dir(work.path())
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| format!("no .app in {}", zip.display()))?;

        let (device, runtime) = pick(
            &simctl().args(["list", "devicetypes", "-j"]).stdout()?,
            &simctl()
                .args(["list", "runtimes", "available", "-j"])
                .stdout()?,
        )?;
        fs::write(
            out.join("device.txt"),
            format!("device type {device}, runtime {runtime}\n"),
        )
        .map_err(|e| e.to_string())?;
        let udid = simctl()
            .args([
                "create",
                "sidevoice-shot",
                device.as_str(),
                runtime.as_str(),
            ])
            .stdout()?
            .trim()
            .to_owned();

        let ran = run_app(&udid, &app, bundle, out);

        // The logs are kept whether or not the run went well: they say why it did not. The app's own log is where the app
        // writes it, below its data container (Tauri's log plugin, in the app's data directory).
        let data = simctl()
            .args(["get_app_container", udid.as_str(), bundle, "data"])
            .stdout()
            .map(|p| p.trim().to_owned());
        if let Ok(data) = data {
            let logs = Path::new(&data)
                .join("Library/Application Support")
                .join(bundle)
                .join("logs");
            let _ = Cmd::new("cp")
                .arg("-R")
                .arg(&logs)
                .arg(out.join("logs"))
                .attempt()?;
        }
        let system = simctl()
            .args([
                "spawn",
                udid.as_str(),
                "log",
                "show",
                "--last",
                "10m",
                "--info",
                "--debug",
                "--style",
                "compact",
            ])
            .args(["--predicate", "process == \"Sidevoice\""])
            .attempt()?
            .1;
        fs::write(out.join("system-log.txt"), system).map_err(|e| e.to_string())?;
        let _ = simctl().args(["shutdown", udid.as_str()]).attempt()?;
        let _ = simctl().args(["delete", udid.as_str()]).attempt()?;
        ran
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_pro_max_and_the_newest_ios_are_picked() {
        let types = r#"{"devicetypes":[
            {"name":"iPhone 16 Pro Max","identifier":"t16"},
            {"name":"iPhone 17 Pro Max","identifier":"t17"},
            {"name":"iPhone 17 Pro","identifier":"t17p"},
            {"name":"iPad Pro","identifier":"pad"}]}"#;
        let runtimes = r#"{"runtimes":[
            {"platform":"iOS","version":"26.5","identifier":"r265","isAvailable":true},
            {"platform":"iOS","version":"26.10","identifier":"r2610","isAvailable":true},
            {"platform":"iOS","version":"27.0","identifier":"r270","isAvailable":false},
            {"platform":"watchOS","version":"30.0","identifier":"w","isAvailable":true}]}"#;
        assert_eq!(
            pick(types, runtimes).unwrap(),
            ("t17".to_owned(), "r2610".to_owned())
        );
    }

    #[test]
    fn no_pro_max_is_an_error() {
        assert!(pick(r#"{"devicetypes":[]}"#, r#"{"runtimes":[]}"#).is_err());
    }
}
