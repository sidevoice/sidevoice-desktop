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
    text.split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

/// The newest iPhone Pro Max device type and the newest available iOS runtime, from `simctl list ... -j`.
fn pick(types: &str, runtimes: &str) -> Result<(String, String)> {
    pick_model(types, runtimes, " Pro Max")
}

/// The newest `iPhone <n><model>` device type (`model` " Pro" or " Pro Max") and the newest available iOS runtime.
fn pick_model(types: &str, runtimes: &str, model: &str) -> Result<(String, String)> {
    let types: serde_json::Value = serde_json::from_str(types).map_err(|e| format!("device types: {e}"))?;
    let runtimes: serde_json::Value = serde_json::from_str(runtimes).map_err(|e| format!("runtimes: {e}"))?;
    let number = |name: &str| -> Option<u32> {
        let rest = name.strip_prefix("iPhone ")?.strip_suffix(model)?;
        rest.parse().ok()
    };
    let device = types["devicetypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some((number(t["name"].as_str()?)?, t["identifier"].as_str()?)))
        .max_by_key(|(n, _)| *n)
        .map(|(_, id)| id.to_owned())
        .ok_or_else(|| format!("no iPhone{model} device type on this Mac"))?;
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
        let running = simctl().args(["spawn", udid, "launchctl", "list"]).output()?;
        ensure(
            running.contains(&format!("UIKitApplication:{bundle}")),
            format!("the app is not running after launch ({mode})"),
        )?;
        simctl().args(["io", udid, "screenshot"]).arg(out.join(format!("screen-{mode}.png"))).run()?;
        let _ = simctl().args(["terminate", udid, bundle]).attempt()?;
    }
    Ok(())
}

/// The Live Activity harness's scheme, and the Xcode project XcodeGen makes of its spec.
const HARNESS_SCHEME: &str = "LiveActivityHarness";

/// The harness's UI tests built for the device `udid` (booted here) and run in light, then dark; fails if either failed.
fn run_harness(udid: &str, harness: &Path, work: &Path, out: &Path, only: Option<&str>) -> Result<()> {
    simctl().args(["boot", udid]).run()?;
    simctl().args(["bootstatus", udid, "-b"]).run()?;
    // The status bar as Apple's own screenshots have it, so that two runs' shots differ only by what they show.
    simctl()
        .args(["status_bar", udid, "override", "--time", "9:41", "--dataNetwork", "wifi", "--wifiMode", "active"])
        .args(["--wifiBars", "3", "--cellularMode", "active", "--cellularBars", "4", "--batteryState", "charged"])
        .args(["--batteryLevel", "100"])
        .run()?;
    let project = harness.join(format!("{HARNESS_SCHEME}.xcodeproj"));
    let xcodebuild = |action: &str| {
        Cmd::new("xcodebuild")
            .arg(action)
            .arg("-project")
            .arg(&project)
            .args(["-scheme", HARNESS_SCHEME, "-configuration", "Debug", "-destination"])
            .arg(format!("platform=iOS Simulator,id={udid}"))
            .arg("-derivedDataPath")
            .arg(work)
    };
    xcodebuild("build-for-testing").run()?;
    let mut failed = Vec::new();
    for mode in ["light", "dark"] {
        simctl().args(["ui", udid, "appearance", mode]).run()?;
        // The test process gets the variables prefixed `TEST_RUNNER_`, without the prefix.
        let mut test =
            xcodebuild("test-without-building").env("TEST_RUNNER_LA_OUT", out).env("TEST_RUNNER_LA_APPEARANCE", mode);
        if let Some(only) = only {
            test = test.env("TEST_RUNNER_LA_ONLY", only);
        }
        if let Err(error) = test.run() {
            failed.push(format!("{mode}: {error}"));
        }
    }
    ensure(failed.is_empty(), format!("the Live Activity harness failed: {}", failed.join("; ")))
}

impl Os for Native {
    fn live_activity(harness: &Path, out: &Path, only: Option<&str>) -> Result<()> {
        mkdir(out)?;
        Cmd::new("xcodegen").args(["generate", "--spec", "project.yml"]).dir(harness).run()?;
        let (device, runtime) = pick_model(
            &simctl().args(["list", "devicetypes", "-j"]).stdout()?,
            &simctl().args(["list", "runtimes", "available", "-j"]).stdout()?,
            " Pro",
        )?;
        fs::write(out.join("device.txt"), format!("device type {device}, runtime {runtime}\n"))
            .map_err(|e| e.to_string())?;
        let udid =
            simctl().args(["create", "sidevoice-la", device.as_str(), runtime.as_str()]).stdout()?.trim().to_owned();
        let work = std::env::temp_dir().join(format!("sidevoice-la-{}", std::process::id()));
        let ran = run_harness(&udid, harness, &work, out, only);
        // The system log of the harness and its widget extension, whether or not the run went well: it says why not.
        let system = simctl()
            .args(["spawn", udid.as_str(), "log", "show", "--last", "60m", "--info", "--style", "compact"])
            .args(["--predicate", "process BEGINSWITH \"LiveActivityHarness\" OR subsystem == \"ai.sidevoice\""])
            .attempt()?
            .1;
        fs::write(out.join("system-log.txt"), system).map_err(|e| e.to_string())?;
        let _ = simctl().args(["shutdown", udid.as_str()]).attempt()?;
        let _ = simctl().args(["delete", udid.as_str()]).attempt()?;
        let _ = fs::remove_dir_all(&work);
        ran
    }

    fn renders(package: &Path, out: &Path) -> Result<()> {
        mkdir(out)?;
        let (device, runtime) = pick(
            &simctl().args(["list", "devicetypes", "-j"]).stdout()?,
            &simctl().args(["list", "runtimes", "available", "-j"]).stdout()?,
        )?;
        let udid = simctl()
            .args(["create", "sidevoice-renders", device.as_str(), runtime.as_str()])
            .stdout()?
            .trim()
            .to_owned();
        let work = std::env::temp_dir().join(format!("sidevoice-renders-{}", std::process::id()));
        // A package with more than one product has the scheme `<name>-Package`, which builds and tests them all. The
        // test process gets the variables prefixed `TEST_RUNNER_`, without the prefix.
        let tested = Cmd::new("xcodebuild")
            .args(["test", "-scheme", "CallActivity-Package", "-destination"])
            .arg(format!("platform=iOS Simulator,id={udid}"))
            .arg("-derivedDataPath")
            .arg(&work)
            .env("TEST_RUNNER_CALL_ACTIVITY_RENDERS", out)
            .dir(package)
            .run();
        let _ = simctl().args(["shutdown", udid.as_str()]).attempt()?;
        let _ = simctl().args(["delete", udid.as_str()]).attempt()?;
        let _ = fs::remove_dir_all(&work);
        tested
    }

    fn screenshots(zip: &Path, bundle: &str, out: &Path) -> Result<()> {
        mkdir(out)?;
        let work = std::env::temp_dir().join(format!("sidevoice-simulator-{}", std::process::id()));
        mkdir(&work)?;
        Cmd::new("unzip").arg("-q").arg(zip).arg("-d").arg(&work).run()?;
        let app = fs::read_dir(&work)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| format!("no .app in {}", zip.display()))?;

        let (device, runtime) = pick(
            &simctl().args(["list", "devicetypes", "-j"]).stdout()?,
            &simctl().args(["list", "runtimes", "available", "-j"]).stdout()?,
        )?;
        fs::write(out.join("device.txt"), format!("device type {device}, runtime {runtime}\n"))
            .map_err(|e| e.to_string())?;
        let udid =
            simctl().args(["create", "sidevoice-shot", device.as_str(), runtime.as_str()]).stdout()?.trim().to_owned();

        let ran = run_app(&udid, &app, bundle, out);

        // The logs are kept whether or not the run went well: they say why it did not. The app's own log is where the app
        // writes it, below its data container (Tauri's log plugin, in the app's data directory).
        let data =
            simctl().args(["get_app_container", udid.as_str(), bundle, "data"]).stdout().map(|p| p.trim().to_owned());
        if let Ok(data) = data {
            let logs = Path::new(&data).join("Library/Application Support").join(bundle).join("logs");
            let _ = Cmd::new("cp").arg("-R").arg(&logs).arg(out.join("logs")).attempt()?;
        }
        let system = simctl()
            .args(["spawn", udid.as_str(), "log", "show", "--last", "10m", "--info", "--debug", "--style", "compact"])
            .args(["--predicate", "process == \"Sidevoice\""])
            .attempt()?
            .1;
        fs::write(out.join("system-log.txt"), system).map_err(|e| e.to_string())?;
        let _ = simctl().args(["shutdown", udid.as_str()]).attempt()?;
        let _ = simctl().args(["delete", udid.as_str()]).attempt()?;
        let _ = fs::remove_dir_all(&work);
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
        assert_eq!(pick(types, runtimes).unwrap(), ("t17".to_owned(), "r2610".to_owned()));
    }

    #[test]
    fn the_newest_pro_is_not_a_pro_max() {
        let types = r#"{"devicetypes":[
            {"name":"iPhone 16 Pro","identifier":"t16p"},
            {"name":"iPhone 17 Pro Max","identifier":"t17"},
            {"name":"iPhone 17 Pro","identifier":"t17p"}]}"#;
        let runtimes = r#"{"runtimes":[{"platform":"iOS","version":"26.0","identifier":"r26","isAvailable":true}]}"#;
        assert_eq!(pick_model(types, runtimes, " Pro").unwrap(), ("t17p".to_owned(), "r26".to_owned()));
    }

    #[test]
    fn no_pro_max_is_an_error() {
        assert!(pick(r#"{"devicetypes":[]}"#, r#"{"runtimes":[]}"#).is_err());
    }
}
