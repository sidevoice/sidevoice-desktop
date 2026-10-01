//! Just enough HTTP/1.1 for the local host: parsing a request or response head strictly (the proxy refuses anything
//! it cannot read unambiguously), writing heads back, and a one-request-per-connection client for the app's own
//! calls over `C/local.sock`.
//!
//! Header values are kept byte for byte (latin-1 into `char`s), so a head the proxy rewrites carries every other
//! header exactly as it came.

use std::io::{self, BufRead, BufReader, Cursor, Read, Write};

/// The largest head either side may send.
pub const HEAD_LIMIT: usize = 64 * 1024;
/// The largest body the app reads as a client (health, identity, pairing answers are small).
pub const BODY_LIMIT: u64 = 1024 * 1024;

/// Reads from `stream` into `buf` until a head ends (`\r\n\r\n`) and returns its length, terminator included. What
/// follows it (the start of a body) stays in `buf` after the head.
pub fn read_head(stream: &mut impl Read, buf: &mut Vec<u8>) -> io::Result<usize> {
    let mut scanned: usize = 0;
    loop {
        let from = scanned.saturating_sub(3);
        if let Some(at) = buf[from..].windows(4).position(|w| w == b"\r\n\r\n") {
            return Ok(from + at + 4);
        }
        scanned = buf.len();
        if buf.len() >= HEAD_LIMIT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too large"));
        }
        let mut chunk = [0u8; 8192];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed before the head ended"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Header fields in order, names lower-cased, values as sent (surrounding blanks trimmed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(pub Vec<(String, String)>);

/// The same field more than once, where only one makes sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duplicate;

impl Headers {
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0.iter().filter(move |(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// The field's value when it is there once; `Err` when it is there more than once.
    pub fn one(&self, name: &str) -> Result<Option<&str>, Duplicate> {
        let mut values = self.0.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str());
        let first = values.next();
        if values.next().is_some() {
            return Err(Duplicate);
        }
        Ok(first)
    }

    pub fn first(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n == name)
    }

    /// Whether a comma-separated field (`Connection`) names `token`, in any case.
    pub fn lists(&self, name: &str, token: &str) -> bool {
        self.all(name).flat_map(|v| v.split(',')).any(|t| t.trim().eq_ignore_ascii_case(token))
    }

    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(n, _)| n != name);
    }

    pub fn push(&mut self, name: &str, value: impl Into<String>) {
        self.0.push((name.to_string(), value.into()));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub target: String,
    pub headers: Headers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn to_latin1(text: &str) -> impl Iterator<Item = u8> + '_ {
    // Every char here came from a byte (`latin1`) or from our own ASCII.
    text.chars().map(|c| c as u32 as u8)
}

/// Splits a head (terminator included) into its first line and its fields. Refuses bare CR or LF, folded lines,
/// names that are not tokens and values with control characters.
fn parse_lines(head: &[u8]) -> Result<(String, Headers), &'static str> {
    if !head.ends_with(b"\r\n\r\n") {
        return Err("unterminated head");
    }
    // Every line, the last field's included, ends with CRLF; the blank line's own CRLF is left out.
    let body = &head[..head.len() - 2];
    let mut lines = body[..body.len() - 1].split(|&b| b == b'\n');
    let mut headers = Headers::default();
    let first = lines.next().ok_or("empty head")?;
    let first = strip_cr(first)?;
    for line in lines {
        let line = strip_cr(line)?;
        if line.first().is_some_and(|b| *b == b' ' || *b == b'\t') {
            return Err("folded header line");
        }
        let colon = line.iter().position(|&b| b == b':').ok_or("header line without a colon")?;
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        if name.is_empty() || !name.iter().all(|&b| is_tchar(b)) {
            return Err("bad header name");
        }
        if value.iter().any(|&b| (b < 0x20 && b != b'\t') || b == 0x7f) {
            return Err("control character in a header value");
        }
        let value = value.trim_ascii();
        headers.0.push((latin1(name).to_ascii_lowercase(), latin1(value)));
    }
    Ok((latin1(first), headers))
}

fn strip_cr(line: &[u8]) -> Result<&[u8], &'static str> {
    let line = line.strip_suffix(b"\r").ok_or("bare line feed")?;
    if line.contains(&b'\r') {
        return Err("bare carriage return");
    }
    Ok(line)
}

/// `METHOD SP origin-or-other-form SP HTTP/1.1`, nothing looser.
pub fn parse_request(head: &[u8]) -> Result<RequestHead, &'static str> {
    let (first, headers) = parse_lines(head)?;
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("bad request line");
    };
    if method.is_empty() || !method.bytes().all(is_tchar) {
        return Err("bad method");
    }
    if target.is_empty() || !target.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("bad request target");
    }
    if version != "HTTP/1.1" {
        return Err("not HTTP/1.1");
    }
    Ok(RequestHead { method: method.to_string(), target: target.to_string(), headers })
}

pub fn parse_response(head: &[u8]) -> Result<ResponseHead, &'static str> {
    let (first, headers) = parse_lines(head)?;
    let mut parts = first.splitn(3, ' ');
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else { return Err("bad status line") };
    if !version.starts_with("HTTP/1.") || code.len() != 3 {
        return Err("bad status line");
    }
    let status = code.parse().map_err(|_| "bad status code")?;
    Ok(ResponseHead { status, reason: parts.next().unwrap_or("").to_string(), headers })
}

fn write_fields(out: &mut Vec<u8>, headers: &Headers) {
    for (name, value) in &headers.0 {
        out.extend(to_latin1(name));
        out.extend_from_slice(b": ");
        out.extend(to_latin1(value));
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
}

pub fn request_head(method: &str, target: &str, headers: &Headers) -> Vec<u8> {
    let mut out = format!("{method} {target} HTTP/1.1\r\n").into_bytes();
    write_fields(&mut out, headers);
    out
}

pub fn response_head(status: u16, reason: &str, headers: &Headers) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("HTTP/1.1 {status} ").as_bytes());
    out.extend(to_latin1(reason));
    out.extend_from_slice(b"\r\n");
    write_fields(&mut out, headers);
    out
}

/// The reason phrase the proxy's own answers carry.
pub fn reason(status: u16) -> &'static str {
    match status {
        101 => "Switching Protocols",
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        411 => "Length Required",
        421 => "Misdirected Request",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// An answer the app read as a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> io::Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// One request on `stream` (which the caller connected and gave its timeouts), `Connection: close`, and its whole
/// answer. `Host: localhost`: the core answers to loopback names only, and over its socket there is no other.
pub fn exchange<S: Read + Write>(
    stream: &mut S,
    method: &str,
    target: &str,
    fields: &[(&str, &str)],
    body: Option<&[u8]>,
) -> io::Result<Response> {
    let mut headers = Headers::default();
    headers.push("host", "localhost");
    headers.push("connection", "close");
    headers.push("accept", "application/json");
    for (name, value) in fields {
        headers.push(name, *value);
    }
    if let Some(body) = body {
        headers.push("content-type", "application/json");
        headers.push("content-length", body.len().to_string());
    }
    let mut out = request_head(method, target, &headers);
    out.extend_from_slice(body.unwrap_or_default());
    stream.write_all(&out)?;
    stream.flush()?;

    let mut buf = Vec::new();
    let end = read_head(stream, &mut buf)?;
    let head = parse_response(&buf[..end]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let rest = Cursor::new(buf[end..].to_vec());
    let mut reader = BufReader::new(rest.chain(stream));
    let body = if head.headers.lists("transfer-encoding", "chunked") {
        read_chunked(&mut reader)?
    } else if let Some(length) = head.headers.first("content-length") {
        let length: u64 = length.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad length"))?;
        if length > BODY_LIMIT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "answer too large"));
        }
        let mut body = vec![0; length as usize];
        reader.read_exact(&mut body)?;
        body
    } else {
        let mut body = Vec::new();
        reader.take(BODY_LIMIT + 1).read_to_end(&mut body)?;
        if body.len() as u64 > BODY_LIMIT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "answer too large"));
        }
        body
    };
    Ok(Response { status: head.status, headers: head.headers, body })
}

fn read_chunked(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        reader.take(1024).read_line(&mut line)?;
        let size = line.trim_end().split(';').next().unwrap_or("").trim();
        let size = u64::from_str_radix(size, 16).map_err(|_| bad("bad chunk size"))?;
        if size == 0 {
            // Trailers, up to the blank line.
            loop {
                let mut trailer = String::new();
                if reader.take(8192).read_line(&mut trailer)? == 0 || trailer.trim_end().is_empty() {
                    return Ok(body);
                }
            }
        }
        if body.len() as u64 + size > BODY_LIMIT {
            return Err(bad("answer too large"));
        }
        let start = body.len();
        body.resize(start + size as usize, 0);
        reader.read_exact(&mut body[start..])?;
        let mut crlf = [0; 2];
        reader.read_exact(&mut crlf)?;
        if &crlf != b"\r\n" {
            return Err(bad("bad chunk end"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_head_parses_with_its_fields_in_order() {
        let head = b"GET /api/x?y=1 HTTP/1.1\r\nHost: 127.0.0.1:5000\r\nOrigin: tauri://localhost\r\nX-A:  b \r\n\r\n";
        let request = parse_request(head).unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.target, "/api/x?y=1");
        assert_eq!(request.headers.one("host"), Ok(Some("127.0.0.1:5000")));
        assert_eq!(request.headers.first("x-a"), Some("b"));
        assert_eq!(request_head("GET", "/api/x?y=1", &request.headers).len(), head.len() - 2);
    }

    #[test]
    fn anything_ambiguous_is_refused() {
        for head in [
            &b"GET / HTTP/1.1\nHost: a\r\n\r\n"[..],
            b"GET / HTTP/1.1\r\nHost: a\rb\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: a\r\n folded\r\n\r\n",
            b"GET / HTTP/1.1\r\nHo st: a\r\n\r\n",
            b"GET / HTTP/1.1\r\n: a\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost a\r\n\r\n",
            b"GET / HTTP/1.1\r\nX: a\x00b\r\n\r\n",
            b"GET  / HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.0\r\n\r\n",
            b"GET / HTTP/1.1 extra\r\n\r\n",
            b"G\"T / HTTP/1.1\r\n\r\n",
        ] {
            assert!(parse_request(head).is_err(), "{:?}", String::from_utf8_lossy(head));
        }
    }

    #[test]
    fn duplicates_are_told_apart_from_absence() {
        let request = parse_request(b"GET / HTTP/1.1\r\nOrigin: a\r\norigin: b\r\n\r\n").unwrap();
        assert_eq!(request.headers.one("origin"), Err(Duplicate));
        assert_eq!(request.headers.one("host"), Ok(None));
    }

    #[test]
    fn the_client_reads_length_chunked_and_closed_bodies() {
        for (answer, expected) in [
            (&b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi"[..], &b"hi"[..]),
            (
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;x=y\r\nhi\r\n3\r\n th\r\n0\r\nT: v\r\n\r\n",
                b"hi th",
            ),
            (b"HTTP/1.1 404 Not Found\r\n\r\nall of it", b"all of it"),
        ] {
            let mut stream = Duplex { input: Cursor::new(answer.to_vec()), output: Vec::new() };
            let response = exchange(&mut stream, "POST", "/p", &[("x-k", "v")], Some(b"{}")).unwrap();
            assert_eq!(response.body, expected);
            let sent = String::from_utf8(stream.output).unwrap();
            assert!(sent.starts_with("POST /p HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n"), "{sent}");
            assert!(sent.ends_with("content-length: 2\r\n\r\n{}"), "{sent}");
            // Native's own calls carry no Origin: the core refuses its socket-only routes to anything that does.
            assert!(!sent.contains("origin"), "{sent}");
        }
    }

    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
