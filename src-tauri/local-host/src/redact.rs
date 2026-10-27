//! Text from outside (an installer's message and error output, a library's error) made fit for the app log: redacted
//! by default. Credentials are recognised by what surrounds them (a sensitive name before `=`/`:`, a sensitive word
//! before the value, a URL's userinfo, query and fragment, a private-key block, a long opaque string), not by a list of
//! spellings alone; what is left (paths, messages, versions) stays, because it is what a diagnosis needs.

/// What stands where a value was withheld.
pub const WITHHELD: &str = "[redacted]";

/// Names whose value is a credential, matched with `-`, `_` and case ignored, as a part of a name (`X-Amz-Signature`).
const SENSITIVE_NAMES: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "auth",
    "key",
    "code",
    "signature",
    "credential",
    "cookie",
    "session",
    "bearer",
];

/// Words that announce a credential as the next word (`Bearer abc`, `pairing code: 123456`), matched whole.
const ANNOUNCING_WORDS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "bearer",
    "basic",
    "code",
    "key",
    "apikey",
    "cookie",
    "credentials",
];

fn normalized(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '-' | '_')).flat_map(char::to_lowercase).collect()
}

fn sensitive_name(name: &str) -> bool {
    let name = normalized(name);
    !name.is_empty() && SENSITIVE_NAMES.iter().any(|s| name.contains(s))
}

fn announcing(word: &str) -> bool {
    let word = normalized(word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_'));
    ANNOUNCING_WORDS.contains(&word.as_str())
}

/// A long string with no structure: hex, base64, a JWT or an opaque id.
fn opaque(word: &str) -> bool {
    let word = word.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | ')' | '(' | '[' | ']'));
    word.len() >= 32
        && !word.starts_with('/')
        && !word.starts_with("./")
        && word.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-.~".contains(c))
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
}

/// `scheme://host/path` of a URL: no userinfo, query or fragment.
fn url(word: &str) -> String {
    let (scheme, rest) = word.split_once("://").unwrap_or(("", word));
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let rest = &rest[..end];
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let mut out = format!("{scheme}://{host}");
    if !path.is_empty() {
        out.push('/');
        out.push_str(path);
    }
    if end < word.len() - scheme.len() - 3 {
        out.push_str("?[redacted]");
    }
    out
}

fn word(word: &str, after_announcing: bool) -> String {
    if word.contains("://") {
        // A URL may sit after a `key=`/`(`: redact the URL part as one.
        let start = word.find("://").map_or(0, |i| {
            word[..i].rfind(|c: char| !c.is_ascii_alphanumeric() && !"+.-".contains(c)).map_or(0, |p| p + 1)
        });
        return format!("{}{}", &word[..start], url(&word[start..]));
    }
    if after_announcing {
        return WITHHELD.to_string();
    }
    for sep in ['=', ':'] {
        if let Some((name, value)) = word.split_once(sep) {
            if sensitive_name(name) && !value.is_empty() {
                return format!("{name}{sep}{WITHHELD}");
            }
        }
    }
    if opaque(word) {
        return WITHHELD.to_string();
    }
    word.to_string()
}

/// One line, redacted. Whitespace between words is kept.
pub fn line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after = false;
    for (i, piece) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if piece.is_empty() {
            continue;
        }
        out.push_str(&word(piece, after));
        // `name:`/`name=` ending a word announces the next one too (`pairing code: 123456`).
        after = announcing(piece) && !piece.contains("://");
    }
    out
}

/// Lines in order, redacted; a private-key block (`-----BEGIN ...` to `-----END ...`) is withheld whole, and so is
/// the rest of one whose start is not among the lines.
pub fn lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut in_key = false;
    lines
        .into_iter()
        .map(|text| {
            if text.contains("-----BEGIN") {
                in_key = !text.contains("-----END");
                return WITHHELD.to_string();
            }
            if in_key {
                in_key = !text.contains("-----END");
                return WITHHELD.to_string();
            }
            if text.contains("-----END") {
                return WITHHELD.to_string();
            }
            line(text)
        })
        .collect()
}

/// A free text, redacted and cut to `limit` characters.
pub fn text(text: &str, limit: usize) -> String {
    let all = lines(text.lines()).join(" / ");
    if all.chars().count() <= limit {
        return all;
    }
    all.chars().take(limit).chain("…".chars()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gone(input: &str, secret: &str) {
        let out = line(input);
        assert!(!out.contains(secret), "{input:?} -> {out:?}");
    }

    #[test]
    fn credentials_in_their_common_spellings_are_withheld() {
        for (input, secret) in [
            ("api-key=abc123", "abc123"),
            ("apiKey=abc123", "abc123"),
            ("API_KEY: abc123", "abc123"),
            ("pairing code: 123456", "123456"),
            ("device_code=abc123", "abc123"),
            ("Authorization: Bearer abc123", "abc123"),
            ("GET https://example.invalid/callback?code=abc123", "abc123"),
            ("https://example.invalid/download?X-Amz-Signature=abc123#frag", "abc123"),
            ("https://user:pw123@registry.example/pkg", "pw123"),
            ("npm ERR! //registry.example/:_authToken=npm_abc123", "npm_abc123"),
            ("session=Zm9vYmFyYmF6cXV4cXV1eHF1dXhxdXV4", "Zm9vYmFy"),
            ("0123456789abcdef0123456789abcdef01234567", "0123456789abcdef"),
        ] {
            gone(input, secret);
        }
    }

    #[test]
    fn a_url_keeps_its_host_and_path_and_harmless_text_stays() {
        assert_eq!(
            line("GET https://registry.example/pkg/-/pkg-1.tgz?x=1"),
            "GET https://registry.example/pkg/-/pkg-1.tgz?[redacted]"
        );
        assert_eq!(line("tokenizer initialization failed"), "tokenizer initialization failed");
        assert_eq!(line("connector: unsafe private directory /tmp/x"), "connector: unsafe private directory /tmp/x");
        assert_eq!(
            line("/home/u/.local/share/sidevoice/connector/bin/installer-1234567890abcdef12345678"),
            "/home/u/.local/share/sidevoice/connector/bin/installer-1234567890abcdef12345678"
        );
    }

    #[test]
    fn a_private_key_block_is_withheld_whole_even_without_its_header() {
        let out = lines([
            "before",
            "-----BEGIN PRIVATE KEY-----",
            "MIIEvQIBADANBg",
            "kqhkiG9w0BAQ",
            "-----END PRIVATE KEY-----",
            "after",
        ]);
        assert_eq!(out, ["before", WITHHELD, WITHHELD, WITHHELD, WITHHELD, "after"]);
        let out = lines(["short", "tail of a body", "-----END PRIVATE KEY-----", "after"]);
        assert_eq!(out[3], "after");
        assert_eq!(out[2], WITHHELD);
    }

    #[test]
    fn a_text_is_cut_to_its_limit() {
        assert_eq!(text(&"a ".repeat(100), 10).chars().count(), 11);
    }
}
