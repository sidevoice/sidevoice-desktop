//! `host.computerName` (docs/BRIDGE.md): the name the person gave this computer, which the page proposes as this
//! device's name. The shell asks the system (`src/platform/host.rs`); what it answers is read here.

/// The name as the page gets it: trimmed, and none when nothing is left.
pub fn given(raw: &str) -> Option<String> {
    let name = raw.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Linux's pretty hostname, the one `hostnamectl set-hostname --pretty` writes: `PRETTY_HOSTNAME` in
/// `/etc/machine-info`, an environment-style file whose values may be quoted (`"Ana's laptop"`, `'Mac de Ana'`) with
/// backslash escapes inside double quotes. None when it is not set, or blank.
pub fn pretty_hostname(machine_info: &str) -> Option<String> {
    let value = machine_info.lines().rev().find_map(|line| line.trim().strip_prefix("PRETTY_HOSTNAME="))?;
    let unquoted = if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            out.push(if c == '\\' { chars.next().unwrap_or('\\') } else { c });
        }
        out
    } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        inner.to_owned()
    } else {
        value.to_owned()
    };
    given(&unquoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_name_is_none() {
        assert_eq!(given("  Mac de Ana \n"), Some("Mac de Ana".into()));
        assert_eq!(given(" \n"), None);
        assert_eq!(given(""), None);
    }

    #[test]
    fn the_pretty_hostname_is_read_as_systemd_writes_it() {
        let file = "# written by hostnamectl\nCHASSIS=laptop\nPRETTY_HOSTNAME=\"Ana\\'s \\\"work\\\" laptop\"\n";
        assert_eq!(pretty_hostname(file), Some("Ana's \"work\" laptop".into()));
        assert_eq!(pretty_hostname("PRETTY_HOSTNAME='Mac de Ana'"), Some("Mac de Ana".into()));
        assert_eq!(pretty_hostname("PRETTY_HOSTNAME=anas-box"), Some("anas-box".into()));
        // The last assignment wins, as when the file is sourced.
        assert_eq!(pretty_hostname("PRETTY_HOSTNAME=old\nPRETTY_HOSTNAME=new"), Some("new".into()));
    }

    #[test]
    fn no_pretty_hostname_is_none() {
        assert_eq!(pretty_hostname(""), None);
        assert_eq!(pretty_hostname("CHASSIS=desktop\nICON_NAME=computer-desktop"), None);
        assert_eq!(pretty_hostname("PRETTY_HOSTNAME=\"\""), None);
        assert_eq!(pretty_hostname("PRETTY_HOSTNAME=\"  \""), None);
        assert_eq!(pretty_hostname("#PRETTY_HOSTNAME=commented"), None);
    }
}
