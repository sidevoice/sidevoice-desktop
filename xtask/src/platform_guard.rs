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
        r"\b(?:std\s*::\s*(?:os\b|\{[^;]*?\bos\b)|",
        r"(?:libc|rustix|windows_sys|windows|objc",
        r"2\w*|coc",
        r"oa|core_",
        r"foundation)\s*::)"
    ))
    .unwrap();
    // Bare crate segments and aliases are meaningful inside use trees, not in cfg conditions.
    let imports = Regex::new(r"\buse\s+[^;]+;").unwrap();
    let bare = Regex::new(concat!(
        r"\b(?:libc|rustix|windows_sys|windows|objc",
        r"2\w*|coc",
        r"oa|core_",
        r"foundation)\b\s*(?:as\b|[,};])"
    ))
    .unwrap();
    let cfg = Regex::new(r"\bcfg(?:_attr)?\s*!?\s*\(").unwrap();
    let os = Regex::new(r"\b(?:target_os|unix|windows|target_family)\b").unwrap();
    api.find_iter(source).count()
        + imports.find_iter(source).map(|tree| bare.find_iter(tree.as_str()).count()).sum::<usize>()
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

fn parse_debt(debt: &str) -> Result<BTreeMap<String, usize>> {
    let mut limits = BTreeMap::new();
    for line in debt.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let (path, limit) = line.split_once(' ').ok_or("invalid platform-debt.txt line")?;
        let limit = limit.parse::<usize>().map_err(|e| e.to_string())?;
        if limits.insert(path.to_owned(), limit).is_some() {
            return Err(format!("duplicate platform debt: {path}"));
        }
    }
    Ok(limits)
}

fn base_debt(root: &Path) -> Result<Option<BTreeMap<String, usize>>> {
    let branch = std::env::var("GITHUB_BASE_REF").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "main".into());
    let reference = format!("refs/remotes/origin/{branch}");
    let git = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(root).output().map_err(|e| e.to_string())
    };
    // Actions checks out a shallow merge commit; fetch its base explicitly instead of silently skipping CI.
    if std::env::var("CI").as_deref() == Ok("true") {
        let refspec = format!("+refs/heads/{branch}:{reference}");
        if !git(&["fetch", "--depth=1", "origin", &refspec])?.status.success() {
            return Err(format!("cannot fetch platform debt base {branch}"));
        }
    }
    if !git(&["rev-parse", "--verify", &reference])?.status.success() {
        eprintln!("platform-guard: no local base {reference}; skipping base debt comparison");
        return Ok(None);
    }
    let ledger = format!("{reference}:platform-debt.txt");
    let output = git(&["show", &ledger])?;
    if output.status.success() {
        return parse_debt(std::str::from_utf8(&output.stdout).map_err(|e| e.to_string())?).map(Some);
    }
    // Bootstrap this guard's first PR against the actual base sources, never against its proposed allowance.
    let tree = git(&["ls-tree", "-r", "--name-only", &reference])?;
    if !tree.status.success() {
        return Err("cannot read platform debt base tree".into());
    }
    let mut limits = BTreeMap::new();
    for path in std::str::from_utf8(&tree.stdout).map_err(|e| e.to_string())?.lines().filter(|p| p.ends_with(".rs")) {
        let source = git(&["show", &format!("{reference}:{path}")])?;
        if !source.status.success() {
            return Err(format!("cannot read base source {path}"));
        }
        let actual = count(Path::new(path), std::str::from_utf8(&source.stdout).map_err(|e| e.to_string())?);
        if actual > 0 {
            limits.insert(path.to_owned(), actual);
        }
    }
    Ok(Some(limits))
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
    let mut limits = parse_debt(&debt)?;
    let mut errors = Vec::new();
    if let Some(base) = base_debt(&root)? {
        for (path, limit) in &limits {
            match base.get(path) {
                None => errors.push(format!("{path}: cannot add a platform debt entry")),
                Some(previous) if limit > previous => {
                    errors.push(format!("{path}: platform debt cannot increase from {previous} to {limit}"))
                }
                _ => {}
            }
        }
    }
    for path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(path).map_err(|e| e.to_string())?;
        let source = std::fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
        let actual = count(Path::new(path), &source);
        let limit = limits.remove(path).unwrap_or(0);
        if actual > limit {
            errors.push(format!("{path}: {actual} OS occurrences, allowed {limit}; move OS code into platform"));
        } else if actual < limit {
            errors.push(format!("{path}: platform debt dropped from {limit} to {actual}; lower platform-debt.txt"));
        }
    }
    for path in limits.keys() {
        errors.push(format!("{path}: file removed; remove its platform-debt.txt entry"));
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
    #[test]
    fn catches_grouped_import() {
        assert_eq!(count(Path::new("src/example.rs"), concat!("use std", "::{os::unix::fs::PermissionsExt};")), 1);
    }

    #[test]
    fn catches_renamed_import() {
        assert_eq!(count(Path::new("src/example.rs"), concat!("use li", "bc as sys; sys::getpid();")), 1);
    }
}
