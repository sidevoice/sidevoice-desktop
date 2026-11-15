//! What the probe flows accept: the readings of the log lines the probe page and the card helper write, and the
//! conditions they must meet. Text in, verdict out, the same on every OS.

use std::fs;
use std::path::Path;

use crate::util::{ensure, has, re};
use crate::Result;

/// The proxy's port, from the app's log line `local-host proxy on http://127.0.0.1:<port>`.
pub fn proxy_port(log: &str) -> Option<String> {
    let line = re(r"local-host proxy on http://127\.0\.0\.1:([0-9]+)");
    log.lines().find_map(|l| line.captures(l)).map(|c| c[1].to_owned())
}

/// The checks on the probe page's `probe-engine ok` line (each pattern matched line by line, as grep did, when there
/// are several): the native engine inside the app, through the bridge the
/// interface uses (docs/BRIDGE.md): capabilities with the OS's real memory (the CPU for sherpa-onnx, Metal for
/// whisper.cpp, never Core ML; remote providers are catalogues of their own), downloads with progress in the web's
/// shape (by build id, by engine, by nothing), the builds on disk by model + engine, a page engine refused rather
/// than replaced, a download cancelled by its AbortSignal (one already aborted never starts), and removing a model
/// (by key and code).
pub fn check_engine_line(line: &str) -> Result<()> {
    let must = |pattern: &str| ensure(has(line, pattern), format!("probe-engine: no {pattern}"));
    must(r#"capabilities=\{"runs":"native","os":"macos","arch":"aarch64","has":\["#)?;
    for accelerator in ["cpu", "metal"] {
        must(&format!(r#""has":\[[^\]]*"{accelerator}""#))?;
    }
    ensure(!line.contains(r#""coreml""#), "Core ML offered")?;
    must(r#""memory_mb":[1-9][0-9]{3,}"#)?;
    must(r" installed=kokoro-82m-v1.0@sherpa-onnx,silero-vad@sherpa-onnx,whisper-tiny@whisper-cpp ")?;
    must(r" progress_calls=[1-9]")?;
    must(r" refused=yes ")?;
    must(r#" memory=\{"total_mb":[1-9][0-9]{3,},"available_mb":[1-9][0-9]*\}"#)?;
    must(
        r" cancel=install-cancelled/install_cancelled pre_aborted=install-cancelled cancel_gone=true progress_event=done\+fraction\+total:true",
    )?;
    must(r" uninstall=undefined/true/model_unknown:model-not-found/true")
}

/// The checks on the probe page's `probe-voice ok` line: the voice call the app runs (host.voice): the seam's
/// call-scoped methods and acknowledged route, the engine's catalogues (host.engine) with the voice activity detector, an unknown model refused
/// by key and code, the system's audio devices (the speaker selectable, an unlisted id refused), and a start that
/// settles: listening, or refused only for want of a microphone or a speaker here. A headless runner may also let
/// start succeed and reject the input route because capture is inactive. In that case stopping can still emit an
/// empty take, but no captured samples. Returns the build and the end-of-turn model the web's settings named. A
/// routed call asked for its take, which reached the page as a `Float32Array` at 16 kHz before the stop resolved.
pub fn check_voice_line(line: &str) -> Result<(String, String)> {
    let must = |pattern: &str| ensure(has(line, pattern), format!("probe-voice: no {pattern}"));
    must(" keys=6 calls=2 route=(ok|not-attempted|voice_failed/audio-input-inactive) delayed_turn=true ")?;
    must(" devices=[0-9]+/[0-9]+/true unknown_device=voice_audio_device_unknown/audio-device-unknown ")?;
    // The local catalogue first; a provider with no key lists nothing and says so.
    must(" catalogs=local,[a-z-]+:credential-missing")?;
    must(" models=[1-9][0-9]* vad=true unknown=voice_model_unknown/model-unknown ")?;
    // The web's settings: a build of the model (one not its own refused by code) and the end-of-turn model chosen.
    must(" bad_build=voice_build_not_found/build-not-found ")?;
    // The models of one half loaded ahead of a call (or refused by code, a runner without the network), and a call with an
    // output half only, which needs no microphone: it works (`off`), or is refused only for want of a speaker.
    must(" preload=(ok|voice_[a-z_]+/[a-z-]+) listen=(off|voice_[a-z_]+/[a-z-]+) ")?;
    let first = line.lines().next().unwrap_or_default();
    let build = last_word_after(first, "build");
    let turn = last_word_after(first, "turn_model");
    ensure(build != "none" && turn != "none", format!("no build ({build}) or end-of-turn model ({turn}) chosen"))?;
    ensure(
        has(line, r" route=ok .* start=ok take=Float32Array/16000/true samples=[0-9]+ ")
            || has(
                line,
                r" route=not-attempted .* start=voice_failed/(microphone-unavailable|microphone-denied|speaker-unavailable|audio-device-unavailable|audio-device-failed) take=none samples=none ",
            )
            || has(line, r" route=voice_failed/audio-input-inactive .* start=ok take=none samples=none ")
            || has(
                line,
                r" route=voice_failed/audio-input-inactive .* start=ok take=Float32Array/16000/true samples=0 ",
            ),
        "probe-voice: start, route and take disagree",
    )?;
    Ok((build, turn))
}

/// `sed -E 's/.* key=([^ ]+) .*/\1/'`: the value of the last ` key=value ` in `line`, or the whole line when there is
/// none.
fn last_word_after(line: &str, key: &str) -> String {
    re(&format!(r".* {key}=([^ ]+) .*")).captures(line).map_or_else(|| line.to_owned(), |c| c[1].to_owned())
}

/// Every path under `dir`, files and directories (`find DIR` without DIR itself); none when it is not there.
pub fn paths(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut found = Vec::new();
    for path in entries.filter_map(|entry| entry.ok().map(|e| e.path())) {
        if path.is_dir() {
            found.extend(paths(&path));
        }
        found.push(path);
    }
    found
}

/// `card-ci windows`' first line, when it found the card: its frame.
pub fn card_frame(windows: &str) -> Option<(i64, i64, i64, i64)> {
    let first = windows.lines().next()?;
    let c = re(r" x=(-?[0-9]+) y=(-?[0-9]+) w=([0-9]+) h=([0-9]+)").captures(first)?;
    Some((c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?, c[4].parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINE: &str = r#"probe-engine ok capabilities={"runs":"native","os":"macos","arch":"aarch64","has":["cpu","metal"],"memory_mb":16384} installed=kokoro-82m-v1.0@sherpa-onnx,silero-vad@sherpa-onnx,whisper-tiny@whisper-cpp progress_calls=12 refused=yes memory={"total_mb":16384,"available_mb":9000} cancel=install-cancelled/install_cancelled pre_aborted=install-cancelled cancel_gone=true progress_event=done+fraction+total:true uninstall=undefined/true/model_unknown:model-not-found/true"#;

    const VOICE: &str = "probe-voice ok keys=6 calls=2 route=not-attempted delayed_turn=true devices=2/3/true unknown_device=voice_audio_device_unknown/audio-device-unknown catalogs=local,openai:credential-missing models=7 vad=true unknown=voice_model_unknown/model-unknown bad_build=voice_build_not_found/build-not-found build=whisper-tiny@whisper-cpp turn_model=smart-turn-v3 preload=ok listen=off start=voice_failed/microphone-unavailable take=none samples=none ";

    #[test]
    fn the_engine_line_as_the_probe_writes_it_passes() {
        assert_eq!(check_engine_line(ENGINE), Ok(()));
    }

    #[test]
    fn the_engine_line_fails_on_each_missing_fact() {
        assert!(check_engine_line(&ENGINE.replace(r#""cpu","metal""#, r#""cpu""#)).is_err());
        assert!(check_engine_line(&ENGINE.replace(r#""metal""#, r#""metal","coreml""#)).is_err());
        assert!(check_engine_line(&ENGINE.replace("16384}", "512}")).is_err());
        assert!(check_engine_line(&ENGINE.replace("progress_calls=12", "progress_calls=0")).is_err());
        assert!(check_engine_line(&ENGINE.replace("done+fraction", "done fraction")).is_err());
        assert!(check_engine_line(&ENGINE.replace("refused=yes", "refused=no")).is_err());
        // A pattern never spans two lines.
        let split = ENGINE.replace(r#""cpu","metal""#, "\"cpu\"\n\"metal\"]");
        assert!(check_engine_line(&split).is_err());
    }

    #[test]
    fn the_voice_line_gives_its_build_and_turn_model() {
        let (build, turn) = check_voice_line(VOICE).unwrap();
        assert_eq!((build.as_str(), turn.as_str()), ("whisper-tiny@whisper-cpp", "smart-turn-v3"));
        assert!(check_voice_line(&VOICE.replace("build=whisper-tiny@whisper-cpp", "build=none")).is_err());
        assert!(check_voice_line(
            &VOICE.replace("start=voice_failed/microphone-unavailable", "start=voice_failed/model-failed")
        )
        .is_err());
        let listened = VOICE.replace("route=not-attempted", "route=ok").replace(
            "start=voice_failed/microphone-unavailable take=none samples=none",
            "start=ok take=Float32Array/16000/true samples=128",
        );
        assert!(check_voice_line(&listened).is_ok());
        let unavailable_route = VOICE
            .replace("route=not-attempted", "route=voice_failed/audio-input-inactive")
            .replace("start=voice_failed/microphone-unavailable", "start=ok");
        assert!(check_voice_line(&unavailable_route).is_ok());
        let empty_take = unavailable_route.replace("take=none samples=none", "take=Float32Array/16000/true samples=0");
        assert!(check_voice_line(&empty_take).is_ok());
        assert!(check_voice_line(&empty_take.replace("samples=0", "samples=1")).is_err());
        assert!(check_voice_line(&empty_take.replace("Float32Array", "Array")).is_err());
        assert!(check_voice_line(&unavailable_route.replace("route=voice_failed/audio-input-inactive", "route=ok"))
            .is_err());
        assert!(check_voice_line(&listened.replace("route=ok", "route=voice_failed/audio-input-inactive")).is_err());
        // A start that listened hands its take; one refused has none.
        assert!(check_voice_line(&VOICE.replace("start=voice_failed/microphone-unavailable", "start=ok")).is_err());
        assert!(check_voice_line(&listened.replace("Float32Array", "Array")).is_err());
        assert!(check_voice_line(&VOICE.replace("keys=6", "keys=5")).is_err());
        assert!(check_voice_line(&VOICE.replace("preload=ok", "preload=")).is_err());
        assert!(check_voice_line(&VOICE.replace("listen=off", "listen=on")).is_err());
        assert!(check_voice_line(&VOICE.replace(
            "preload=ok listen=off",
            "preload=voice_failed/model-load-failed listen=voice_failed/speaker-unavailable"
        ))
        .is_ok());
        assert!(check_voice_line(&VOICE.replace("route=not-attempted", "route=missing")).is_err());
        assert!(check_voice_line(&VOICE.replace("route=not-attempted", "route=ok")).is_err());
        assert!(check_voice_line(&VOICE.replace("delayed_turn=true", "delayed_turn=false")).is_err());
    }

    #[test]
    fn a_missing_value_is_the_whole_line_as_sed_leaves_it() {
        assert_eq!(last_word_after("a bad_build=x/y c", "build"), "a bad_build=x/y c");
        assert_eq!(last_word_after("a build=one b build=two c", "build"), "two");
    }

    #[test]
    fn the_proxy_port_and_the_card_frame_are_read() {
        let log =
            "x\nsidevoice: local-host proxy on http://127.0.0.1:53123 (page)\nlocal-host proxy on http://127.0.0.1:1\n";
        assert_eq!(proxy_port(log).as_deref(), Some("53123"));
        assert_eq!(proxy_port("nothing"), None);
        let windows = "card onscreen=true above_editor=true editor_onscreen=true x=-120 y=40 w=320 h=78\n";
        assert_eq!(card_frame(windows), Some((-120, 40, 320, 78)));
        assert_eq!(card_frame("card onscreen=false editor_onscreen=true\n  pid=1 layer=0 bounds=x=1"), None);
    }

    #[test]
    fn paths_lists_files_and_directories_below() {
        let dir = std::env::temp_dir().join(format!("xtask-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("models/whisper-small@whisper-cpp")).unwrap();
        let found = paths(&dir.join("models"));
        assert!(found.iter().any(|p| p.to_string_lossy().contains("whisper-small")));
        assert!(!found.iter().any(|p| p.is_file()));
        assert!(paths(&dir.join("partial")).is_empty(), "no directory, nothing there");
        fs::create_dir_all(dir.join("partial/a")).unwrap();
        fs::write(dir.join("partial/a/half"), b"x").unwrap();
        assert!(paths(&dir.join("partial")).iter().any(|p| p.is_file()));
        fs::remove_dir_all(&dir).unwrap();
    }
}
