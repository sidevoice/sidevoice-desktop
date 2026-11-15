//! The grammar of a machine's log tail line, the one the machine writes (sidevoice-core `crash` module): the app checks it
//! again before it sends anything.
//!
//! `<offset> <LEVEL> [<span>: ]<target>: ...`: the offset from the crash (`-12.345s`: no date, nothing that says when the
//! machine was in use), the level `WARN` or `ERROR`, any spans the event was logged in (`name{fields}:` or `name:`), then a
//! target of the piece's own (or the platform's, or the panic's). A dependency's line, a debug line and a raw stderr line are
//! none of these.

use super::machine::Piece;

/// Whether `time` is an offset: a sign, seconds, three digits of milliseconds and `s`.
fn is_offset(time: &str) -> bool {
    let Some(rest) = time.strip_prefix(['-', '+']).and_then(|rest| rest.strip_suffix('s')) else {
        return false;
    };
    rest.split_once('.').is_some_and(|(seconds, millis)| {
        (1..=12).contains(&seconds.len())
            && seconds.bytes().all(|b| b.is_ascii_digit())
            && millis.len() == 3
            && millis.bytes().all(|b| b.is_ascii_digit())
    })
}

/// The first `target:` or `span{fields}:` of `rest` and what follows it.
fn split_head(rest: &str) -> Option<(&str, &str)> {
    let name = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
    if name > 0 && rest[name..].starts_with('{') {
        let (mut quoted, mut escaped) = (false, false);
        for (at, c) in rest.char_indices().skip_while(|(at, _)| *at <= name) {
            match c {
                _ if escaped => escaped = false,
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                '}' if !quoted => {
                    return rest[at + 1..].strip_prefix(':').map(|after| (&rest[..=at], after));
                }
                _ => {}
            }
        }
        return None;
    }
    rest.split_once(": ")
}

/// Whether `line` is a warning or an error of `piece`'s own log, as the machine writes it into a record.
pub fn is_tail_line(line: &str, piece: Piece) -> bool {
    let mut parts = line.split_whitespace();
    let (Some(time), Some(level)) = (parts.next(), parts.next()) else { return false };
    if !is_offset(time) || !matches!(level, "WARN" | "ERROR") {
        return false;
    }
    let rest = line.trim_start()[time.len()..].trim_start();
    let mut rest = rest[level.len()..].trim_start();
    let own = match piece {
        Piece::Core => "sidevoice_core",
        Piece::Connector => "sidevoice_connector",
    };
    let targets = [own, "sidevoice_platform", "panic"];
    loop {
        let Some((head, after)) = split_head(rest) else { return false };
        if targets
            .iter()
            .any(|name| head == *name || head.strip_prefix(name).is_some_and(|tail| tail.starts_with("::")))
        {
            return true;
        }
        let ident = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !(ident(head) || head.contains('{') && head.ends_with('}')) {
            return false;
        }
        rest = after.trim_start();
    }
}
