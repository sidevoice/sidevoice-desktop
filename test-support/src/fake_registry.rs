//! A stand-in npm registry, for tests only: the two documents and the tarball of a `sidevoice` release
//! built on the spot, served through [`Fetch`]; and the tarballs themselves, malformed ones included, written header by
//! header so a test can put anything in a name.

#[cfg(unix)]
use sidevoice_local_host::{install::Failure, npm::Fetch};
pub struct Response {
    pub status: u16,
    pub length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}
const REGISTRY: &str = "https://registry.npmjs.org";
use base64::Engine as _;
use sha2::{Digest, Sha512};
use std::collections::HashMap;
use std::io::Read;
use std::io::Write;
use std::sync::Mutex;
use std::time::Duration;

/// One tar entry: a ustar header with `path` written as it is (no check, up to 100 bytes), then `data`, padded.
pub fn tar_entry(path: &str, data: &[u8], kind: tar::EntryType, mode: u32, link: Option<&str>) -> Vec<u8> {
    let mut header = tar::Header::new_ustar();
    {
        let raw = header.as_old_mut();
        let name = path.as_bytes();
        raw.name = [0; 100];
        raw.name[..name.len().min(100)].copy_from_slice(&name[..name.len().min(100)]);
        if let Some(link) = link {
            let link = link.as_bytes();
            raw.linkname = [0; 100];
            raw.linkname[..link.len().min(100)].copy_from_slice(&link[..link.len().min(100)]);
        }
    }
    header.set_mode(mode);
    header.set_size(data.len() as u64);
    header.set_entry_type(kind);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    let mut bytes = header.as_bytes().to_vec();
    bytes.extend_from_slice(data);
    bytes.resize(bytes.len().div_ceil(512) * 512, 0);
    bytes
}

pub fn gzip(raw: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(raw).unwrap();
    encoder.finish().unwrap()
}

/// A gzipped tar of plain files `(path, contents, mode)`, as `npm pack` makes one (files only, no directories).
pub fn tarball(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (path, data, mode) in files {
        raw.extend(tar_entry(path, data, tar::EntryType::Regular, *mode, None));
    }
    raw.extend([0u8; 1024]);
    gzip(&raw)
}

#[derive(Clone)]
struct Served {
    status: u16,
    body: Vec<u8>,
    length: Option<u64>,
    /// The body comes 1 KiB at a time, each after this long.
    delay: Option<Duration>,
}

pub struct FakeRegistry {
    served: Mutex<HashMap<String, Served>>,
    asked: Mutex<Vec<String>>,
    tarball_url: String,
}

impl FakeRegistry {
    /// `sidevoice` `version` as the latest, listing `platform` at the same version, whose tarball holds `files` below
    /// `package/` (`(path, contents, mode)`).
    pub fn release(version: &str, platform: &str, files: &[(&str, &[u8], u32)]) -> FakeRegistry {
        let files: Vec<(String, &[u8], u32)> =
            files.iter().map(|(path, data, mode)| (format!("package/{path}"), *data, *mode)).collect();
        let files: Vec<(&str, &[u8], u32)> = files.iter().map(|(p, d, m)| (p.as_str(), *d, *m)).collect();
        let archive = tarball(&files);
        let short = platform.rsplit('/').next().unwrap_or(platform);
        let tarball_url = format!("{REGISTRY}/{platform}/-/{short}-{version}.tgz");
        let integrity =
            format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&archive)));
        let latest = serde_json::json!({
            "name": "sidevoice", "version": version, "bin": {"sidevoice": "bin/sidevoice.js"},
            "optionalDependencies": {platform: version, "@sidevoice/sidevoice-other-cpu": version},
        });
        let document = serde_json::json!({
            "name": platform, "version": version,
            "dist": {"tarball": tarball_url, "integrity": integrity, "shasum": "0"},
        });
        let registry = FakeRegistry { served: Mutex::default(), asked: Mutex::default(), tarball_url };
        registry.serve(&format!("{REGISTRY}/sidevoice/latest"), 200, latest.to_string().into_bytes());
        registry.serve(&format!("{REGISTRY}/{platform}/{version}"), 200, document.to_string().into_bytes());
        registry.serve(&registry.tarball_url.clone(), 200, archive);
        registry
    }

    pub fn tarball_url(&self) -> String {
        self.tarball_url.clone()
    }

    pub fn tarball_bytes(&self) -> Vec<u8> {
        self.served.lock().unwrap()[&self.tarball_url].body.clone()
    }

    pub fn serve(&self, url: &str, status: u16, body: Vec<u8>) {
        let length = Some(body.len() as u64);
        self.served.lock().unwrap().insert(url.to_string(), Served { status, body, length, delay: None });
    }

    /// `body` under a `Content-Length` that may not be its length.
    pub fn serve_with_length(&self, url: &str, body: Vec<u8>, length: u64) {
        self.served
            .lock()
            .unwrap()
            .insert(url.to_string(), Served { status: 200, body, length: Some(length), delay: None });
    }

    /// What `url` serves comes slowly: 1 KiB at a time, each after `delay`.
    pub fn slow(&self, url: &str, delay: Duration) {
        if let Some(served) = self.served.lock().unwrap().get_mut(url) {
            served.delay = Some(delay);
        }
    }

    /// Every URL asked, in order.
    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

struct Slow {
    body: std::io::Cursor<Vec<u8>>,
    delay: Duration,
}

impl Read for Slow {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(self.delay);
        let n = buf.len().min(1024);
        self.body.read(&mut buf[..n])
    }
}

impl FakeRegistry {
    pub fn get(&self, url: &str) -> Response {
        self.asked.lock().unwrap().push(url.to_string());
        let Some(served) = self.served.lock().unwrap().get(url).cloned() else {
            return Response { status: 404, length: None, body: Box::new(std::io::empty()) };
        };
        let body: Box<dyn Read + Send> = match served.delay {
            Some(delay) => Box::new(Slow { body: std::io::Cursor::new(served.body), delay }),
            None => Box::new(std::io::Cursor::new(served.body)),
        };
        Response { status: served.status, length: served.length, body }
    }
}

#[cfg(unix)]
impl Fetch for FakeRegistry {
    fn get(&self, url: &str) -> Result<sidevoice_local_host::npm::Response, Failure> {
        let response = FakeRegistry::get(self, url);
        Ok(sidevoice_local_host::npm::Response {
            status: response.status,
            length: response.length,
            body: response.body,
        })
    }
}
