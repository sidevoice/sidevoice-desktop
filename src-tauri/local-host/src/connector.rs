//! The node service's live state: `node.status` on the connector's IPC socket, `D/connector.sock` (SEAMS §4), JSON
//! lines `{id, method, params}` → `{id, ok, result|error}`. Connecting never starts anything: no answer is simply no
//! connector, and the caller asks the service manager through the CLI instead ([`crate::cli`]).

use crate::checks;
use crate::paths::DataDirs;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

/// SEAMS §3: a connector that does not answer within 1.5 s is not answering.
pub const TIMEOUT: Duration = Duration::from_millis(1500);
const LINE_LIMIT: u64 = 1024 * 1024;

/// `node.status`'s result, or `None` when no connector of this user answers in time (absent, unsafe, silent, or an
/// answer that is not one).
pub fn node_status(dirs: &DataDirs) -> Option<Value> {
    let stream = checks::connect(&dirs.data, &dirs.connector_socket(), checks::owned_not_writable).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    let mut writer = &stream;
    writer.write_all(b"{\"id\":1,\"method\":\"node.status\",\"params\":{}}\n").ok()?;
    let mut line = String::new();
    BufReader::new(&stream).take(LINE_LIMIT).read_line(&mut line).ok()?;
    let reply: Value = serde_json::from_str(&line).ok()?;
    if reply.get("id") != Some(&Value::from(1)) || reply.get("ok") != Some(&Value::Bool(true)) {
        return None;
    }
    let result = reply.get("result")?;
    result.get("state")?.as_str()?;
    Some(result.clone())
}
