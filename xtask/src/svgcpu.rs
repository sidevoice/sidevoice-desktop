//! `macos svg-cpu` (desktop#149): what an animated SVG costs the macOS webview, and whether WebKit's layer-based SVG
//! engine (LBSE, a runtime feature flag, off by default) changes it. A measurement, not a gate: it prints a table and
//! fails only when the app never ran the page.
//!
//! The probe app shows `test/fixtures/svg-anim.html` (four figures whose inner groups animate `transform` and
//! `opacity` from CSS keyframes, as the creatures do) and the step reads, from `ps`, the CPU time the app and every
//! WebKit process spend over a stretch of it: off (WebKit as it ships), LBSE switched on before the webview exists (a
//! `WKWebViewConfiguration`, src-tauri/src/probe/webkit.rs), LBSE switched on late (the live webview, then a reload),
//! and the same page with every animation stopped (the control: what the rest costs).

use std::collections::BTreeMap;
use std::time::Instant;

use crate::macos::AppHome;
use crate::util::{ensure, grep, mkdir, read, repo, sleep, wait_for, work, write, Cmd};
use crate::Result;

/// How long each run is measured, after the page has been running a few seconds.
const SECONDS: u64 = 20;
/// The key WebKit has used for the layer-based SVG engine, when the run's own list does not show a better match.
const LBSE_KEY: &str = "LayerBasedSVGEngineEnabled";

/// One process's accumulated CPU time (user + system), from `ps -o time`: `[[dd-]hh:]mm:ss.cc`.
pub fn cpu_seconds(time: &str) -> Option<f64> {
    let (days, rest) = match time.split_once('-') {
        Some((d, rest)) => (d.parse::<f64>().ok()?, rest),
        None => (0.0, time),
    };
    let mut seconds = 0.0;
    for part in rest.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86400.0 + seconds)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    App,
    Gpu,
    WebContent,
    Network,
    OtherWebKit,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Gpu => "GPU",
            Kind::WebContent => "WebContent",
            Kind::Network => "Network",
            Kind::OtherWebKit => "other WebKit",
        }
    }
}

/// The processes that count, from `ps -A -o pid=,time=,command=`: the app (by pid) and every WebKit XPC service (by
/// path: they are launchd's children, not the app's; on a runner nothing else uses WebKit while a step runs, and a
/// process from before the run cancels out in the difference).
pub fn processes(ps: &str, app: u32) -> BTreeMap<u32, (Kind, f64)> {
    let mut found = BTreeMap::new();
    for line in ps.lines() {
        let mut words = line.split_whitespace();
        let (Some(pid), Some(time)) = (words.next().and_then(|p| p.parse::<u32>().ok()), words.next()) else {
            continue;
        };
        let Some(seconds) = cpu_seconds(time) else { continue };
        let command = line.trim_start().splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim_start();
        let kind = if pid == app {
            Kind::App
        } else if command.contains("com.apple.WebKit.GPU") {
            Kind::Gpu
        } else if command.contains("com.apple.WebKit.WebContent") {
            Kind::WebContent
        } else if command.contains("com.apple.WebKit.Networking") {
            Kind::Network
        } else if command.contains("com.apple.WebKit.") {
            Kind::OtherWebKit
        } else {
            continue;
        };
        found.insert(pid, (kind, seconds));
    }
    found
}

/// The CPU each kind spent between two samples, in seconds.
pub fn spent(before: &BTreeMap<u32, (Kind, f64)>, after: &BTreeMap<u32, (Kind, f64)>) -> BTreeMap<Kind, f64> {
    let mut total = BTreeMap::new();
    for (pid, (kind, now)) in after {
        let was = before.get(pid).map_or(0.0, |(_, s)| *s);
        *total.entry(*kind).or_insert(0.0) += (now - was).max(0.0);
    }
    total
}

/// The features worth reading for this question, from the `webkit-feature` lines the app logs.
pub fn relevant(log: &str) -> Vec<&str> {
    grep(log, "webkit-feature when=config|webkit-feature when=late-before")
        .into_iter()
        .filter(|line| {
            let lower = line.to_lowercase();
            ["svg", "composit", "accelerat", "gpu", "layer", "animation", "canvas", "metal", "render", "css"]
                .iter()
                .any(|word| lower.contains(word))
        })
        .collect()
}

/// The key of the layer-based SVG engine in the app's own list, else [`LBSE_KEY`].
pub fn lbse_key(log: &str) -> String {
    let key = crate::util::re(r"key=(\S*(?i:layer[a-z]*svg|lbse)\S*)");
    grep(log, "webkit-feature ")
        .iter()
        .find_map(|line| key.captures(line).map(|c| c[1].to_owned()))
        .unwrap_or_else(|| LBSE_KEY.to_owned())
}

struct Run {
    label: &'static str,
    page: &'static str,
    /// `SIDEVOICE_DEBUG_WEBKIT_ENABLE`, or none.
    enable: Option<String>,
    late: bool,
}

struct Result_ {
    label: &'static str,
    cpu: BTreeMap<Kind, f64>,
    seconds: f64,
    animations: String,
    live: Vec<String>,
}

fn sample(app: u32) -> BTreeMap<u32, (Kind, f64)> {
    let ps = Cmd::new("ps").args(["-A", "-o", "pid=,time=,command="]).stdout().unwrap_or_default();
    processes(&ps, app)
}

fn measure(home: &AppHome, run: &Run, dir: &std::path::Path, shots: &std::path::Path) -> Result<(Result_, String)> {
    let log = dir.join(format!("{}.log", run.label));
    let mut env: Vec<(&str, &str)> = vec![("SIDEVOICE_DEBUG_PAGE", run.page), ("SIDEVOICE_DEBUG_WEBKIT_FEATURES", "1")];
    if let Some(enable) = &run.enable {
        env.push(("SIDEVOICE_DEBUG_WEBKIT_ENABLE", enable));
    }
    if run.late {
        env.push(("SIDEVOICE_DEBUG_WEBKIT_LATE", "1"));
    }
    let mut app = home.app(&env).spawn(&log)?;
    let pid = app.pid();
    let up = wait_for(&log, "svg-anim running", 0, 90);
    let measured = (|| {
        ensure(up, format!("{}: the page never said it was running", run.label))?;
        // The page, the GPU process and the first frames settle.
        sleep(8.0);
        let before = sample(pid);
        let started = Instant::now();
        sleep(SECONDS as f64);
        let after = sample(pid);
        let seconds = started.elapsed().as_secs_f64();
        let _ = Cmd::new("screencapture").arg("-x").arg(shots.join(format!("{}.png", run.label))).attempt();
        Ok((before, after, seconds))
    })();
    app.stop();
    let text = read(&log);
    let (before, after, seconds) = match measured {
        Ok(m) => m,
        Err(error) => {
            eprintln!("--- the app's last lines\n{}", text.lines().rev().take(60).collect::<Vec<_>>().join("\n"));
            return Err(error);
        }
    };
    let animations = grep(&text, "svg-anim running").first().map(|l| l.to_string()).unwrap_or_default();
    let live = grep(&text, "webkit-enable |webkit-live ").into_iter().map(str::to_owned).collect();
    Ok((Result_ { label: run.label, cpu: spent(&before, &after), seconds, animations, live }, text))
}

fn table(results: &[Result_]) -> String {
    let kinds = [Kind::Gpu, Kind::WebContent, Kind::App, Kind::Network, Kind::OtherWebKit];
    let mut out = format!("{:<18}", "run");
    for kind in kinds {
        out += &format!(" {:>13}", kind.name());
    }
    out += &format!(" {:>13}\n", "total");
    for r in results {
        out += &format!("{:<18}", r.label);
        let mut total = 0.0;
        for kind in kinds {
            let percent = r.cpu.get(&kind).copied().unwrap_or(0.0) / r.seconds * 100.0;
            total += percent;
            out += &format!(" {:>12.1}%", percent);
        }
        out += &format!(" {:>12.1}%\n", total);
    }
    out + &format!("(CPU time as a share of one core over {SECONDS} s, from ps; 100% is one core busy)")
}

fn webkit_version() -> String {
    let plist = "/System/Library/Frameworks/WebKit.framework/Resources/Info.plist";
    let read =
        |key: &str| Cmd::new("plutil").args(["-extract", key, "raw", "-o", "-", plist]).stdout().unwrap_or_default();
    let os = Cmd::new("sw_vers").stdout().unwrap_or_default().replace('\n', "; ");
    format!(
        "WebKit.framework {} (build {}); {}",
        read("CFBundleShortVersionString").trim(),
        read("CFBundleVersion").trim(),
        os.trim()
    )
}

/// `macos svg-cpu`.
pub fn svg_cpu() -> Result<()> {
    let home = AppHome::new("svg-cpu")?;
    let dir = work("svg-cpu")?;
    let shots = repo().join("svg-cpu-shots");
    mkdir(&shots)?;
    let version = webkit_version();
    eprintln!("{version}");

    let mut runs = vec![Run { label: "off", page: "svg-anim.html", enable: None, late: false }];
    let mut results = Vec::new();
    let mut features = String::new();
    let mut key = LBSE_KEY.to_owned();
    let mut index = 0;
    while index < runs.len() {
        let (result, text) = measure(&home, &runs[index], &dir, &shots)?;
        if index == 0 {
            // The first run's list names the flag (and everything else relevant) for the rest.
            key = lbse_key(&text);
            features = grep(&text, "webkit-feature ").join("\n");
            write(&shots.join("webkit-features.txt"), format!("{version}\n{features}\n"))?;
            runs.push(Run { label: "lbse-config", page: "svg-anim.html", enable: Some(key.clone()), late: false });
            runs.push(Run { label: "lbse-late", page: "svg-anim.html", enable: Some(key.clone()), late: true });
            runs.push(Run { label: "stopped", page: "svg-anim-still.html", enable: None, late: false });
        }
        eprintln!("--- {}\n{}\n{}", result.label, result.animations, result.live.join("\n"));
        results.push(result);
        index += 1;
    }

    let table = table(&results);
    let relevant_features = relevant(&features);
    eprintln!("{table}");
    eprintln!(
        "--- WebKit features related to SVG, compositing, GPU or animation ({} of {}), key used: {key}",
        relevant_features.len(),
        features.lines().count()
    );
    eprintln!("{}", relevant_features.join("\n"));
    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
        let mut text =
            format!("### Animated SVG in the macOS webview (desktop#149)\n\n{version}\n\n```\n{table}\n```\n\n");
        for r in &results {
            text += &format!("- `{}`: {}\n", r.label, r.live.join(" | "));
        }
        text += &format!(
            "\n<details><summary>Features related to SVG, compositing, GPU or animation ({} of {})</summary>\n\n```\n{}\n```\n</details>\n",
            relevant_features.len(),
            features.lines().count(),
            relevant_features.join("\n")
        );
        let old = std::fs::read_to_string(&summary).unwrap_or_default();
        write(std::path::Path::new(&summary), old + &text)?;
    }
    ensure(!features.is_empty(), "the app listed no WebKit features")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_is_read() {
        assert_eq!(cpu_seconds("0:01.50"), Some(1.5));
        assert_eq!(cpu_seconds("12:03.25"), Some(723.25));
        assert_eq!(cpu_seconds("1:02:03.00"), Some(3723.0));
        assert_eq!(cpu_seconds("2-00:00:01.00"), Some(172801.0));
        assert_eq!(cpu_seconds("x"), None);
    }

    #[test]
    fn processes_are_told_apart_and_differenced() {
        let ps = "  10   0:02.00 /Apps/Sidevoice.app/Contents/MacOS/sidevoice-desktop\n\
                  11   0:10.00 /System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU\n\
                  12   0:20.00 /System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent\n\
                  13   9:00.00 /usr/bin/other\n";
        let before = processes(ps, 10);
        assert_eq!(before.len(), 3);
        let after = processes(&ps.replace("0:10.00", "0:15.00").replace("0:02.00", "0:03.00"), 10);
        let used = spent(&before, &after);
        assert_eq!(used[&Kind::Gpu], 5.0);
        assert_eq!(used[&Kind::App], 1.0);
        assert_eq!(used[&Kind::WebContent], 0.0);
    }

    #[test]
    fn the_flag_is_found_in_the_list() {
        let log = "x\nsidevoice: webkit-feature when=config list=internal key=Foo status=1 default=false value=false name=\"a\"\n\
                   sidevoice: webkit-feature when=config list=feature key=LayerBasedSVGEngineEnabled status=2 default=false value=false name=\"Layer-based SVG Engine\"\n";
        assert_eq!(lbse_key(log), "LayerBasedSVGEngineEnabled");
        assert_eq!(lbse_key("nothing"), LBSE_KEY);
        assert_eq!(relevant(log).len(), 1);
    }
}
