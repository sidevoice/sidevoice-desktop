//! Native stand-in connector. Configuration lives beside the executable, never in shell text.
use process_wrap::std::ChildWrapper;
use serde_json::{json, Value};
use sidevoice_desktop_test_support::{fixture, platform::process};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}
fn emit(value: &Value) {
    println!("{value}");
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--fixture-sleep") {
        // Until the stop file named by the next argument exists (a test ends its sleeper with it), or 30 s.
        let stop = args.get(1).map(PathBuf::from);
        for _ in 0..1500 {
            if stop.as_ref().is_some_and(|stop| stop.exists()) {
                break;
            }
            sleep(20);
        }
        return;
    }
    let executable = std::env::current_exe().unwrap();
    let config: Value = serde_json::from_slice(&fs::read(fixture::config_path(&executable)).unwrap()).unwrap();
    let data = PathBuf::from(std::env::var_os("SIDEVOICE_DATA_DIR").unwrap_or_default());
    if let Some(ran) = config["ran"].as_str() {
        fs::write(ran, format!("{} {} {}", args.join(" "), data.display(), std::env::current_dir().unwrap().display()))
            .unwrap();
    }
    if config["install_record"] == true {
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.json.tmp"), "{}").unwrap();
        fs::rename(data.join("install.json.tmp"), data.join("install.json")).unwrap();
    }
    if config["child"] == true || config["detached"] == true {
        let mut child = Command::new(&executable);
        child.arg("--fixture-sleep");
        if let Some(stop) = config["stop"].as_str() {
            child.arg(stop);
        }
        let mut spawned: Box<dyn ChildWrapper> = if config["detached"] == true {
            child.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            process::detach(child).unwrap()
        } else {
            Box::new(child.spawn().unwrap())
        };
        fs::write(config["pid"].as_str().unwrap(), spawned.id().to_string()).unwrap();
        // The parent intentionally leaves its descendant running for the runner's lifecycle checks.
        if config["wait_child"] == true {
            let _ = spawned.wait();
        }
    }
    if let Some(stderr) = config["stderr"].as_str() {
        eprintln!("{stderr}");
    }
    sleep(config["delay_ms"].as_u64().unwrap_or(0));
    match config["mode"].as_str() {
        Some("host") | Some("login") => {
            let mut log = fs::OpenOptions::new().create(true).append(true).open(data.join("calls.log")).unwrap();
            writeln!(log, "{}", args.join(" ")).unwrap();
            let pair = args.iter().take(2).map(String::as_str).collect::<Vec<_>>().join(" ");
            if config["mode"] == "login" {
                let mut login = fs::read_to_string(data.join("login")).is_ok_and(|v| v.trim() == "true");
                if pair == "service status" && data.join("gate").exists() {
                    fs::remove_file(data.join("gate")).unwrap();
                    while !data.join("release").exists() {
                        sleep(20);
                    }
                }
                if pair == "service login" {
                    login = args.get(2).is_some_and(|a| a == "on");
                    fs::write(data.join("login"), login.to_string()).unwrap();
                }
                let mut answer = json!({"ok":true,"state":"stopped-by-person","service":"launchd"});
                if pair == "service status" || pair == "service login" {
                    answer["start_at_login"] = json!(login);
                }
                emit(&answer);
                return;
            }
            match pair.as_str() {
                "--version --json" => {
                    let version: Value = serde_json::from_slice(&fs::read(data.join("version")).unwrap()).unwrap();
                    fs::write(data.join("version-read"), "").unwrap();
                    if data.join("slow").exists() {
                        sleep(1000);
                    }
                    emit(&json!({"ok":true,"version":version}));
                }
                "service status" => emit(&config["status"]),
                "pair-device --json" => {
                    emit(&json!({"ok":true,"code":"SV1-CODE","expires_in":600,"reach":"local-only","payload":{}}))
                }
                "service login" if config["unsupported_login"] == true => {
                    emit(
                        &json!({"ok":false,"error":{"key":"connector.usage","params":{"detail":"invalid subcommand"},"message":"m"}}),
                    );
                    std::process::exit(1);
                }
                "service start" => {
                    emit(&json!({"ok":false,"error":{"key":"service.not-loaded","message":"m"}}));
                    std::process::exit(1);
                }
                _ => emit(&json!({"ok":true,"state":"starting","service":"launchd"})),
            }
        }
        _ => {
            if let Some(raw) = config["stdout"].as_str() {
                print!("{raw}");
            } else if let Some(response) = config.get("response") {
                emit(response);
            } else {
                let cwd = std::env::current_dir().ok();
                emit(&json!({"ok":true,"args":args.join(" "),"data":data,"which":config["which"],"cwd":cwd}));
            }
        }
    }
    std::process::exit(config["exit"].as_i64().unwrap_or(0) as i32);
}
