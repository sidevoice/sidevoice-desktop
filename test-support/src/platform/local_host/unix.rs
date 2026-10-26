//! Unix: the local host itself (`serve`) and its socket, a Unix domain socket.
use super::{status_of, Duplex, Modes};
use crate::fake_core::FakeCore;
use sidevoice_local_host::host::{Config, LocalHost};
use sidevoice_local_host::paths::DataDirs;
use sidevoice_local_host::state::State;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::exit;
use std::time::Duration;

pub struct Os;

impl Modes for Os {
    fn serve(data: &Path, app: &Path, port_file: &Path) -> io::Result<()> {
        let dirs = DataDirs::new(data);
        let _core = FakeCore::start(&dirs.data, 1, true).expect("the stand-in core starts");
        std::fs::set_permissions(&dirs.data, std::fs::Permissions::from_mode(0o755)).expect("chmod D");
        let config = Config { dirs, app_dir: app.to_path_buf(), name: "ci".into() };
        let host = LocalHost::start(config, |line| println!("{line}")).expect("the proxy starts");
        let report = host.poll();
        if report.state != State::Running {
            eprintln!("not running: {report:?}");
            exit(1);
        }
        // The first user can: the secret it holds opens the proxy.
        let secret = host.pairing().expect("a pairing")["token"].as_str().unwrap().to_string();
        let port = host.proxy().port();
        let status = status_of(
            &format!(
                "GET /api/device/devices HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: tauri://localhost\r\n\
         Authorization: Bearer {secret}\r\n\r\n"
            ),
            port,
        );
        println!("serve: the first user through the proxy with the secret -> {status}");
        if status != 200 {
            exit(1);
        }
        std::fs::write(port_file, port.to_string()).expect("port file");
        host.watch();
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    fn open_core_socket(socket: &Path) -> io::Result<Box<dyn Duplex>> {
        Ok(Box::new(UnixStream::connect(socket)?))
    }
}
