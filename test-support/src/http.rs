//! Fixture HTTP parsing, deliberately independent of the production parser.
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Default)]
pub struct Headers(Vec<(String, String)>);
impl Headers {
    pub fn first(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
    pub fn has(&self, name: &str) -> bool {
        self.first(name).is_some()
    }
    pub fn lists(&self, name: &str, token: &str) -> bool {
        self.first(name).unwrap_or("").split(',').any(|v| v.trim().eq_ignore_ascii_case(token))
    }
    pub fn push(&mut self, name: &str, value: impl Into<String>) {
        self.0.push((name.into(), value.into()));
    }
}
pub struct Head {
    pub method: String,
    pub target: String,
    pub headers: Headers,
}
pub async fn read_head(stream: &mut (impl AsyncRead + Unpin), buf: &mut Vec<u8>) -> io::Result<usize> {
    while !buf.ends_with(b"\r\n\r\n") {
        if buf.len() >= 65536 {
            return Err(io::Error::other("fixture head too large"));
        }
        buf.push(stream.read_u8().await?);
    }
    Ok(buf.len())
}
pub fn parse_request(buf: &[u8]) -> Result<Head, httparse::Error> {
    let mut fields = [httparse::EMPTY_HEADER; 128];
    let mut request = httparse::Request::new(&mut fields);
    request.parse(buf)?;
    Ok(Head {
        method: request.method.unwrap_or("").into(),
        target: request.path.unwrap_or("").into(),
        headers: Headers(
            request
                .headers
                .iter()
                .map(|h| (h.name.to_ascii_lowercase(), String::from_utf8_lossy(h.value).into_owned()))
                .collect(),
        ),
    })
}
pub fn response_head(status: u16, reason: &str, headers: &Headers) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in &headers.0 {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.into_bytes()
}
