//! Platform boundary check, including code inactive on this CI host.
//! Counts matches even in comments and strings: keep OS examples inside platform too.
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
    let mut errors = Vec::new();
    let mut total = 0;
    for path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(path).map_err(|e| e.to_string())?;
        let source = std::fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
        let actual = count(Path::new(path), &source);
        if actual > 0 {
            total += actual;
            errors.push(format!("{path}: {actual} OS occurrences; move OS code into platform"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{}\nPlatform boundary: {total} OS occurrences in {} files", errors.join("\n"), errors.len()))
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
