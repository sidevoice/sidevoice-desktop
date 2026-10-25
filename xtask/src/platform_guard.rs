//! Textual ratchet for OS dependencies, including code inactive on this CI host.
//! Counts matches even in comments and strings: keep OS examples inside platform too.
use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::{util::repo, Result};

fn count(path: &Path, source: &str) -> usize {
    if path.components().any(|part| part.as_os_str() == "platform")
        || path.file_name().is_some_and(|name| name == "platform.rs")
    {
        return 0;
    }
    // Split spellings so the checker does not count its own pattern as OS debt.
    let api = Regex::new(concat!(
        r"\b(?:std\s*::\s*os\s*::|(?:libc|rustix|windows_sys|windows)\s*::|objc",
        r"2\w*\b|coc",
        r"oa\b|core_",
        r"foundation\b)"
    ))
    .unwrap();
    let cfg = Regex::new(r"\bcfg(?:_attr)?\s*!?\s*\(").unwrap();
    let os = Regex::new(r"\b(?:target_os|unix|windows|target_family)\b").unwrap();
    api.find_iter(source).count()
        + cfg
            .find_iter(source)
            .filter(|start| {
                let mut depth = 1;
                let tail = &source[start.end()..];
                let end = tail
                    .char_indices()
                    .find_map(|(index, ch)| {
                        match ch {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        (depth == 0).then_some(index)
                    })
                    .unwrap_or(tail.len());
                os.is_match(&tail[..end])
            })
            .count()
}

pub fn check() -> Result<()> {
    let root = repo();
    // Git's inventory includes every crate and build script, without target/, vendored caches or worktrees.
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.rs"])
        .current_dir(&root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("git ls-files failed".into());
    }
    let debt = std::fs::read_to_string(root.join("platform-debt.txt")).map_err(|e| e.to_string())?;
    let mut limits = BTreeMap::new();
    for line in debt.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let (path, limit) = line.split_once(' ').ok_or("invalid platform-debt.txt line")?;
        let limit = limit.parse::<usize>().map_err(|e| e.to_string())?;
        if limits.insert(path.to_owned(), limit).is_some() {
            return Err(format!("duplicate platform debt: {path}"));
        }
    }
    let mut errors = Vec::new();
    for path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(path).map_err(|e| e.to_string())?;
        let source = std::fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
        let actual = count(Path::new(path), &source);
        let limit = limits.remove(path).unwrap_or(0);
        if actual > limit {
            errors.push(format!("{path}: {actual} OS occurrences, allowed {limit}; move OS code into platform"));
        } else if actual < limit {
            eprintln!("{path}: platform debt dropped from {limit} to {actual}; lower platform-debt.txt");
        }
    }
    for path in limits.keys() {
        eprintln!("{path}: file removed; remove its platform-debt.txt entry");
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_os_code() {
        assert_eq!(
            count(
                Path::new("tests/example.rs"),
                concat!(
                    "#[cfg_",
                    "attr(\n all(target_os = \"linux\", unix), ignore)]\nuse std::",
                    "os::unix::fs::PermissionsExt;"
                )
            ),
            2
        );
    }

    #[test]
    fn allows_platform_files() {
        for path in ["src/platform/user/unix.rs", "xtask/src/platform.rs"] {
            assert_eq!(count(Path::new(path), concat!("#[cf", "g(unix)] use li", "bc::geteuid;")), 0);
        }
    }
}
