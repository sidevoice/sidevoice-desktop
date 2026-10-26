//! Platform boundary check, including code inactive on this CI host.
//!
//! Reads Rust tokens, not text: comments and string contents are never code, so OS examples in docs and fixtures
//! cost nothing. What counts as an OS operation outside `platform`:
//! - a path rooted in an OS crate (`libc`, `rustix`, `windows*`, `objc2*`, `core_foundation*`…), `std::os`, or
//!   `std::env::consts::{OS, FAMILY}`;
//! - an OS condition (`target_os`, `target_family`, `target_vendor`, `unix`, `windows`) in `cfg`, `cfg_attr` or
//!   `cfg!`, or a `cfg_aliases!` definition of one, and every use of such an alias;
//! - any name bound to an OS path by `use`/`extern crate` (`use libc as sys;` makes `sys::…` OS code);
//! - a `Command::new("<program>")` of a program that exists on one OS only (shells, `uname`, `launchctl`…);
//! - the Cargo OS variables (`CARGO_CFG_TARGET_OS`…) read by a build script.
//!
//! Limits, by design of a token scan: a program name or OS variable reaching `Command::new` through a constant
//! or a computed value, and a bare function imported by glob (`use libc::*`), are not followed.
use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
    str::FromStr,
};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use crate::{util::repo, Result};

/// Crates (and prefixes of crate families) that are an OS API.
const OS_CRATES: [&str; 11] = [
    "libc",
    "rustix",
    "nix",
    "winapi",
    "winreg",
    "windows",
    "windows_sys",
    "windows_core",
    "cocoa",
    "objc",
    "core_graphics",
];
const OS_CRATE_PREFIXES: [&str; 3] = ["objc2", "core_foundation", "windows_"];
/// Names that make a `cfg`/`cfg_attr` predicate or an attribute OS-specific.
const OS_CONDITIONS: [&str; 6] =
    ["target_os", "target_family", "target_vendor", "unix", "windows", "windows_subsystem"];
/// Cargo's view of the target OS, as build scripts read it: `CARGO_CFG_<suffix>`.
const OS_ENV_SUFFIXES: [&str; 4] = ["TARGET_OS", "TARGET_FAMILY", "UNIX", "WINDOWS"];
/// Programs that exist on one OS only (or whose behaviour is that OS's), matched on the file stem, lowercased.
const OS_PROGRAMS: [&str; 45] = [
    "sh",
    "bash",
    "zsh",
    "dash",
    "cmd",
    "powershell",
    "pwsh",
    "uname",
    "ps",
    "kill",
    "pkill",
    "killall",
    "taskkill",
    "tasklist",
    "wmic",
    "reg",
    "sc",
    "net",
    "icacls",
    "attrib",
    "chmod",
    "chown",
    "chgrp",
    "id",
    "whoami",
    "hostname",
    "lsof",
    "launchctl",
    "osascript",
    "open",
    "xdg-open",
    "defaults",
    "security",
    "sw_vers",
    "sysctl",
    "systemctl",
    "codesign",
    "hdiutil",
    "xcrun",
    "notarytool",
    "ditto",
    "spctl",
    "plutil",
    "signtool",
    "msiexec",
];

fn is_os_crate(name: &str) -> bool {
    OS_CRATES.contains(&name) || OS_CRATE_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn ident(tree: &TokenTree) -> Option<String> {
    match tree {
        TokenTree::Ident(id) => Some(id.to_string().trim_start_matches("r#").to_string()),
        _ => None,
    }
}

fn punct(tree: Option<&TokenTree>, ch: char) -> bool {
    matches!(tree, Some(TokenTree::Punct(p)) if p.as_char() == ch)
}

fn group(tree: Option<&TokenTree>, delimiter: Delimiter) -> Option<TokenStream> {
    match tree {
        Some(TokenTree::Group(g)) if g.delimiter() == delimiter => Some(g.stream()),
        _ => None,
    }
}

fn flat(stream: TokenStream) -> Vec<TokenTree> {
    stream.into_iter().collect()
}

/// `::` at `at` (two joint colons).
fn path_sep(trees: &[TokenTree], at: usize) -> bool {
    punct(trees.get(at), ':') && punct(trees.get(at + 1), ':')
}

/// Whether the identifier at `at` continues a path (`a::HERE`) instead of starting one.
fn mid_path(trees: &[TokenTree], at: usize) -> bool {
    at >= 3 && path_sep(trees, at - 2) && ident(&trees[at - 3]).is_some()
}

/// OS conditions anywhere in a predicate (idents only: `feature = "windows"` is a string).
fn names_os(stream: &TokenStream, aliases: &HashSet<String>) -> bool {
    stream.clone().into_iter().any(|tree| match tree {
        TokenTree::Ident(id) => {
            let name = id.to_string();
            OS_CONDITIONS.contains(&name.as_str()) || aliases.contains(&name)
        }
        TokenTree::Group(g) => names_os(&g.stream(), aliases),
        _ => false,
    })
}

fn string_value(literal: &proc_macro2::Literal) -> Option<String> {
    syn::parse_str::<syn::LitStr>(&literal.to_string()).ok().map(|lit| lit.value())
}

/// `(path, rename)` of every leaf of a `use` tree.
fn leaves(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<(Vec<String>, Option<String>)>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            leaves(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut path = prefix.clone();
            if n.ident != "self" {
                path.push(n.ident.to_string());
            }
            out.push((path, None));
        }
        syn::UseTree::Rename(r) => {
            let mut path = prefix.clone();
            if r.ident != "self" {
                path.push(r.ident.to_string());
            }
            out.push((path, Some(r.rename.to_string())));
        }
        syn::UseTree::Glob(_) => out.push((prefix.clone(), None)),
        syn::UseTree::Group(g) => g.items.iter().for_each(|item| leaves(item, prefix, out)),
    }
}

/// Whether a resolved path is an OS API.
fn os_path(path: &[String]) -> bool {
    let at = |i: usize| path.get(i).map(|s| s.trim_start_matches("r#"));
    match at(0) {
        Some("std" | "core" | "alloc") => {
            at(1) == Some("os")
                || (at(1) == Some("env") && at(2) == Some("consts") && matches!(at(3), Some("OS" | "FAMILY")))
        }
        Some(root) => is_os_crate(root),
        None => false,
    }
}

#[derive(Default)]
struct Scan {
    /// OS conditions defined by `cfg_aliases!`, repository-wide (a build script defines what other files use).
    cfg_aliases: HashSet<String>,
    /// Names this file binds to OS paths (`use libc as sys;`).
    bound: HashSet<String>,
    /// Names that are a process `Command` under another name.
    commands: HashSet<String>,
    lines: BTreeSet<usize>,
    count: usize,
    /// First pass: only learn names, report nothing.
    collecting: bool,
}

impl Scan {
    fn found(&mut self, tree: &TokenTree) {
        if !self.collecting {
            self.count += 1;
            self.lines.insert(tree.span().start().line);
        }
    }

    fn bind(&mut self, at: &TokenTree, leaves: Vec<(Vec<String>, Option<String>)>) {
        for (path, rename) in leaves {
            if path.last().is_some_and(|last| last.ends_with("Command") || last.ends_with("CommandWrap")) {
                self.commands.extend(rename.clone());
            }
            if os_path(&path) {
                self.found(at);
                self.bound.extend(rename.or_else(|| path.last().cloned()));
            }
        }
    }

    fn walk(&mut self, trees: &[TokenTree]) {
        let mut i = 0;
        while i < trees.len() {
            match &trees[i] {
                TokenTree::Group(g) => self.walk(&flat(g.stream())),
                TokenTree::Literal(lit) => {
                    if string_value(lit).is_some_and(|value| {
                        value.strip_prefix("CARGO_CFG_").is_some_and(|name| OS_ENV_SUFFIXES.contains(&name))
                    }) {
                        self.found(&trees[i]);
                    }
                }
                TokenTree::Punct(p) if p.as_char() == '#' => {
                    let at = i + usize::from(punct(trees.get(i + 1), '!')) + 1;
                    if let Some(inner) = group(trees.get(at), Delimiter::Bracket) {
                        let body = flat(inner.clone());
                        let name = body.first().and_then(ident);
                        let cfg = matches!(name.as_deref(), Some("cfg" | "cfg_attr"));
                        if name.as_deref() == Some("windows_subsystem") {
                            self.found(&trees[i]);
                        }
                        if cfg && names_os(&inner, &self.cfg_aliases) {
                            self.found(&trees[i]);
                        }
                        // Other attributes can carry paths and strings (`link`, `doc`): scan them like any tokens.
                        if !cfg {
                            self.walk(&body);
                        }
                        i = at;
                    }
                }
                TokenTree::Ident(_) => {
                    i = self.item(trees, i);
                    continue;
                }
                TokenTree::Punct(_) => {}
            }
            i += 1;
        }
    }

    /// Handles the identifier at `i`; returns the next index to visit.
    fn item(&mut self, trees: &[TokenTree], i: usize) -> usize {
        let tree = &trees[i];
        let name = ident(tree).unwrap_or_default();
        let sep = path_sep(trees, i + 1);
        let start = !mid_path(trees, i);
        let extern_crate = name == "extern" && trees.get(i + 1).and_then(ident).as_deref() == Some("crate");
        if name == "use" || extern_crate {
            let end = trees[i..].iter().position(|t| punct(Some(t), ';')).map_or(trees.len(), |p| i + p);
            let tokens: TokenStream = trees[i..(end + 1).min(trees.len())].iter().cloned().collect();
            if name == "use" {
                if let Ok(item) = syn::parse2::<syn::ItemUse>(tokens) {
                    let mut out = Vec::new();
                    leaves(&item.tree, &mut Vec::new(), &mut out);
                    self.bind(tree, out);
                }
            } else if let Ok(item) = syn::parse2::<syn::ItemExternCrate>(tokens) {
                let rename = item.rename.map(|(_, id)| id.to_string());
                self.bind(tree, vec![(vec![item.ident.to_string()], rename)]);
            }
            return end + 1;
        }
        if name == "cfg" && punct(trees.get(i + 1), '!') {
            if let Some(inner) = group(trees.get(i + 2), Delimiter::Parenthesis) {
                if names_os(&inner, &self.cfg_aliases) {
                    self.found(tree);
                }
                return i + 3;
            }
        }
        if name == "cfg_aliases" && punct(trees.get(i + 1), '!') {
            if let Some(TokenTree::Group(body)) = trees.get(i + 2) {
                self.define_cfg_aliases(&flat(body.stream()));
                return i + 3;
            }
        }
        if sep && start {
            if matches!(name.as_str(), "std" | "core" | "alloc") {
                let seg = |n: usize| trees.get(i + 3 + 3 * n).and_then(ident);
                let os = seg(0).as_deref() == Some("os");
                let consts = seg(0).as_deref() == Some("env")
                    && path_sep(trees, i + 4)
                    && seg(1).as_deref() == Some("consts")
                    && path_sep(trees, i + 7)
                    && matches!(seg(2).as_deref(), Some("OS" | "FAMILY"));
                if os || consts {
                    self.found(tree);
                }
            } else if is_os_crate(&name) || self.bound.contains(&name) {
                self.found(tree);
            }
        }
        // `Command::new("sh")`, wherever the type came from (`std::process::`, `tokio::process::`, an alias).
        if sep
            && (name.ends_with("Command") || name.ends_with("CommandWrap") || self.commands.contains(&name))
            && matches!(trees.get(i + 3).and_then(ident).as_deref(), Some("new" | "with_new"))
        {
            let program =
                group(trees.get(i + 4), Delimiter::Parenthesis).and_then(|args| match args.into_iter().next() {
                    Some(TokenTree::Literal(lit)) => string_value(&lit),
                    _ => None,
                });
            if let Some(program) = program {
                let file = program.rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
                let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".cmd")).unwrap_or(&file);
                if OS_PROGRAMS.contains(&stem) {
                    self.found(tree);
                }
            }
        }
        i + 1
    }

    /// `cfg_aliases! { name: { condition }, … }`: each alias that names an OS counts, and becomes one.
    fn define_cfg_aliases(&mut self, body: &[TokenTree]) {
        for entry in body.split(|tree| punct(Some(tree), ',')) {
            let Some(alias) = entry.first().and_then(ident) else {
                continue;
            };
            let condition: TokenStream = entry[1..].iter().cloned().collect();
            if names_os(&condition, &self.cfg_aliases) {
                self.found(&entry[0]);
                self.cfg_aliases.insert(alias);
            }
        }
    }
}

/// What one file holds: OS operations (count and lines), plus the OS aliases it defines for the others.
struct Report {
    count: usize,
    lines: BTreeSet<usize>,
    cfg_aliases: HashSet<String>,
}

fn scan(source: &str, cfg_aliases: &HashSet<String>) -> Result<Report> {
    let stream = TokenStream::from_str(source).map_err(|e| e.to_string())?;
    let trees = flat(stream);
    let mut scan = Scan { cfg_aliases: cfg_aliases.clone(), collecting: true, ..Scan::default() };
    scan.walk(&trees);
    scan.collecting = false;
    scan.walk(&trees);
    Ok(Report { count: scan.count, lines: scan.lines, cfg_aliases: scan.cfg_aliases })
}

/// Platform code (the `platform` module, `platform.rs`) is where the OS belongs.
fn exempt(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == "platform")
        || path.file_name().is_some_and(|name| name == "platform.rs")
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
    let mut files = Vec::new();
    for path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(path).map_err(|e| e.to_string())?.to_string();
        let source = std::fs::read_to_string(root.join(&path)).map_err(|e| format!("{path}: {e}"))?;
        files.push((path, source));
    }
    // OS aliases are defined in one file (a build script) and used in others, and may build on each other.
    let mut aliases = HashSet::new();
    loop {
        let before = aliases.len();
        for (path, source) in files.iter().filter(|(path, _)| !exempt(Path::new(path))) {
            aliases.extend(scan(source, &aliases).map_err(|e| format!("{path}: {e}"))?.cfg_aliases);
        }
        if aliases.len() == before {
            break;
        }
    }
    let mut errors = Vec::new();
    let mut total = 0;
    for (path, source) in files.iter().filter(|(path, _)| !exempt(Path::new(path))) {
        let report = scan(source, &aliases).map_err(|e| format!("{path}: {e}"))?;
        if report.count > 0 {
            total += report.count;
            let lines: Vec<String> = report.lines.iter().map(usize::to_string).collect();
            errors.push(format!(
                "{path}: {} OS occurrences (lines {}); move OS code into platform",
                report.count,
                lines.join(", ")
            ));
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

    fn n(source: &str) -> usize {
        scan(source, &HashSet::new()).unwrap().count
    }

    #[test]
    fn catches_os_code() {
        assert_eq!(
            n("#[cfg_attr(\n all(target_os = \"linux\", unix), ignore)]\nuse std::os::unix::fs::PermissionsExt;"),
            2
        );
        assert_eq!(n("fn f() { if cfg!(windows) {} let _ = std::env::consts::OS; }"), 2);
        assert_eq!(n("fn f() { unsafe { libc::getpid() }; }"), 1);
    }

    #[test]
    fn ignores_comments_strings_and_unrelated_names() {
        let source = "//! uses std::os::unix and #[cfg(unix)]\n/// libc::getpid()\nfn f(v: &[u8]) {\n // cfg!(windows)\n let _ = \"std::os::unix libc::x cfg(unix)\"; let _ = v.windows(2); let _ = r#\"Command::new(\"sh\")\"#;\n let _ = (cfg!(feature = \"windows\"), cfg!(test)); }";
        assert_eq!(n(source), 0);
    }

    #[test]
    fn exempts_platform_files() {
        for path in ["src/platform/user/unix.rs", "xtask/src/platform.rs"] {
            assert!(exempt(Path::new(path)));
        }
    }

    #[test]
    fn catches_grouped_and_nested_imports() {
        assert_eq!(n("use std::{os::unix::fs::PermissionsExt};"), 1);
        assert_eq!(n("use std::{fs, os::{unix::fs::PermissionsExt, raw}};"), 2);
    }

    #[test]
    fn follows_import_aliases() {
        assert_eq!(n("use libc as sys; fn f() { sys::getpid(); }"), 2);
        assert_eq!(n("use std::os as o; fn f() -> o::unix::raw::pid_t { 0 }"), 2);
        assert_eq!(n("use windows_sys::Win32::Foundation as f; fn g() { let _ = f::CloseHandle; }"), 2);
        assert_eq!(n("extern crate libc as l; fn f() { l::getpid(); }"), 2);
    }

    #[test]
    fn catches_os_in_macros_and_attribute_forms() {
        assert_eq!(n("fn f() { println!(\"{}\", libc::getpid()); }"), 1);
        assert_eq!(n("cfg_if::cfg_if! { if #[cfg(unix)] { fn a() {} } else { fn a() {} } }"), 1);
        assert_eq!(n("#![cfg(target_os = \"macos\")]"), 1);
        assert_eq!(n("#![windows_subsystem = \"windows\"] #[cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")] fn main() {}"), 2);
    }

    #[test]
    fn resolves_cfg_aliases_across_files() {
        let build = "fn main() { cfg_aliases::cfg_aliases! { apple: { target_vendor = \"apple\" }, mac: { all(apple, x) }, other: { feature = \"x\" } } }";
        let aliases = scan(build, &HashSet::new()).unwrap();
        assert_eq!(aliases.count, 2);
        assert_eq!(aliases.cfg_aliases, HashSet::from(["apple".to_string(), "mac".to_string()]));
        let user = "#[cfg(mac)] fn a() {} #[cfg(other)] fn b() {} fn c() { if cfg!(apple) {} }";
        assert_eq!(scan(user, &aliases.cfg_aliases).unwrap().count, 2);
    }

    #[test]
    fn catches_os_commands() {
        assert_eq!(n("fn f() { std::process::Command::new(\"uname\").arg(\"-a\"); }"), 1);
        assert_eq!(n("fn f() { tokio::process::Command::new(\"/bin/sh\"); }"), 1);
        assert_eq!(n("fn f() { Command::new(\"PowerShell.exe\"); }"), 1);
        assert_eq!(n("use std::process::Command as Cmd; fn f() { Cmd::new(\"launchctl\"); }"), 1);
        assert_eq!(n("fn f() { Command::new(\"git\"); Command::new(\"cargo\"); }"), 0);
    }

    #[test]
    fn catches_cargo_os_variables() {
        assert_eq!(n("fn main() { let _ = std::env::var(\"CARGO_CFG_TARGET_OS\"); }"), 1);
    }

    #[test]
    fn rejects_unreadable_source() {
        assert!(scan("fn f() { \"unterminated }", &HashSet::new()).is_err());
    }
}
