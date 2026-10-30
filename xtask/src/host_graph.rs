//! Host kind is a crate boundary (D-RULE; AGENTS.md → "Host kind is a crate boundary"): what the Cargo workspace may
//! depend on, and how a mobile target is built. `platform_guard` rejects the conditions; this is the crates.
//!
//! The crates sit at the workspace root and each app composes the ones it uses:
//! - the host crates, `desktop/` and `mobile/`, depend on neither each other nor anything that depends on the other;
//! - the shared crates, `app-core/` and `app-tauri/`, depend on neither host crate;
//! - `app-core/` has no shell in its closure: no Tauri, no webview and no window crate, in any kind of dependency.
//!
//! They are read from `cargo metadata`, once for each supported target, so a dependency under a `cfg(target_os)` is
//! seen on the targets it applies to. This reads resolution only: it builds nothing, so it runs before a crate has
//! its iOS or Android bodies. A crate that is not in the workspace yet has nothing to violate and is not looked for.
//!
//! A build for a mobile target names its package (`-p`): the workspace holds the other kind of host too, and a
//! workspace-wide build for iOS or Android would try to build it. The workflows are read for that.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use regex::Regex;
use serde_json::Value;

use crate::util::{repo, Cmd};
use crate::Result;

/// The targets the app supports, as Cargo names them.
const TARGETS: [&str; 5] = [
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-ios",
    "aarch64-linux-android",
];
/// The host crates, by their directory at the workspace root.
const HOSTS: [&str; 2] = ["desktop", "mobile"];
/// The shared crates, by directory.
const SHARED: [&str; 2] = ["app-core", "app-tauri"];
/// The shared crate that has no shell.
const CORE: &str = "app-core";

/// Whether a crate is the shell: Tauri, with its plugins and runtime, and the webview and window crates under it.
fn is_shell(name: &str) -> bool {
    matches!(name, "tauri" | "wry" | "tao") || name.starts_with("tauri-")
}

/// The dependency graph of one `cargo metadata` answer.
struct Graph {
    /// Package id → name.
    names: HashMap<String, String>,
    /// Workspace member directory (below the workspace root, `/`-separated) → package id.
    members: HashMap<String, String>,
    /// Package id → the packages it depends on, whatever the kind of dependency.
    edges: HashMap<String, Vec<String>>,
}

impl Graph {
    fn read(metadata: &Value) -> Result<Graph> {
        let root = Path::new(metadata["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?);
        let text = |value: &Value, what: &str| {
            value.as_str().map(str::to_owned).ok_or_else(|| format!("cargo metadata: no {what}"))
        };
        let (mut names, mut manifests) = (HashMap::new(), HashMap::new());
        for package in metadata["packages"].as_array().ok_or("cargo metadata: no packages")? {
            let id = text(&package["id"], "package id")?;
            names.insert(id.clone(), text(&package["name"], "package name")?);
            manifests.insert(id, text(&package["manifest_path"], "manifest_path")?);
        }
        let mut members = HashMap::new();
        for id in metadata["workspace_members"].as_array().ok_or("cargo metadata: no workspace_members")? {
            let id = text(id, "member id")?;
            let manifest = manifests.get(&id).ok_or_else(|| format!("cargo metadata: no package {id}"))?;
            let dir = Path::new(manifest).parent().and_then(|dir| dir.strip_prefix(root).ok());
            if let Some(dir) = dir {
                let dir: Vec<_> = dir.components().map(|part| part.as_os_str().to_string_lossy()).collect();
                members.insert(dir.join("/"), id);
            }
        }
        let mut edges = HashMap::new();
        for node in metadata["resolve"]["nodes"].as_array().ok_or("cargo metadata: no resolve")? {
            let id = text(&node["id"], "node id")?;
            let deps = node["deps"].as_array().into_iter().flatten();
            edges.insert(id, deps.filter_map(|dep| dep["pkg"].as_str().map(str::to_owned)).collect());
        }
        Ok(Graph { names, members, edges })
    }

    /// The names along the shortest dependency path from `from` to the first package `found` accepts.
    fn path<'a>(&'a self, from: &'a str, found: impl Fn(&str) -> bool) -> Option<Vec<&'a str>> {
        let mut came_from: HashMap<&str, &str> = HashMap::new();
        let mut seen: HashSet<&str> = HashSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(id) = queue.pop_front() {
            for next in self.edges.get(id).into_iter().flatten().map(String::as_str) {
                if !seen.insert(next) {
                    continue;
                }
                came_from.insert(next, id);
                if found(self.names.get(next).map_or("", String::as_str)) {
                    let mut path = vec![next];
                    while let Some(&before) = came_from.get(path[path.len() - 1]) {
                        path.push(before);
                    }
                    path.reverse();
                    return Some(path.into_iter().map(|id| self.names.get(id).map_or(id, String::as_str)).collect());
                }
                queue.push_back(next);
            }
        }
        None
    }
}

/// What one `cargo metadata` answer breaks.
pub fn violations(metadata: &Value) -> Result<Vec<String>> {
    let graph = Graph::read(metadata)?;
    let name = |dir: &str| graph.members.get(dir).and_then(|id| graph.names.get(id));
    let mut found = Vec::new();
    let mut forbid = |from: &str, what: &str, goal: &dyn Fn(&str) -> bool| {
        if let Some(path) = graph.members.get(from).and_then(|id| graph.path(id, goal)) {
            found.push(format!("{from}/ must not depend on {what}: {}", path.join(" -> ")));
        }
    };
    for host in HOSTS {
        for other in HOSTS.into_iter().filter(|other| *other != host) {
            if let Some(other_name) = name(other) {
                forbid(host, &format!("{other}/"), &|crate_name| crate_name == other_name);
            }
        }
    }
    for shared in SHARED {
        for host in HOSTS {
            if let Some(host_name) = name(host) {
                forbid(shared, &format!("the host crate {host}/"), &|crate_name| crate_name == host_name);
            }
        }
    }
    forbid(CORE, "a shell (Tauri, wry, tao)", &is_shell);
    Ok(found)
}

/// Lines (1-based) of the workflow `text` that build for iOS or Android without naming the package, or for the whole
/// workspace: a `cargo` command with a mobile `--target` (or `cargo ndk`) that has no `-p`/`--package`, or has
/// `--workspace`, `--all` or `--exclude`.
pub fn unscoped_mobile_builds(text: &str) -> Vec<usize> {
    let mobile = Regex::new(r"\bcargo\b(?:.*--target[= ]\S*-(?:ios|android)|\s+ndk\b)").expect("a regex");
    let scoped = Regex::new(r"\s(-p\S*|--package\b\S*)(\s|$)").expect("a regex");
    let wide = Regex::new(r"\s(?:--workspace|--all|--exclude)(?:\s|=|$)").expect("a regex");
    let mut found = Vec::new();
    let (mut command, mut start) = (String::new(), 0);
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if command.is_empty() {
            start = index + 1;
        }
        // A command continues on the next line after a backslash. Comments and step names are not commands.
        if line.starts_with('#') || line.starts_with("- name:") || line.starts_with("name:") {
            continue;
        }
        command.push(' ');
        command.push_str(line.trim_end_matches('\\'));
        if line.ends_with('\\') {
            continue;
        }
        if mobile.is_match(&command) && (!scoped.is_match(&command) || wide.is_match(&command)) {
            found.push(start);
        }
        command.clear();
    }
    found
}

/// `cargo xtask host-graph`: the workspace's dependency graph on each supported target, and the workflows' builds for
/// mobile targets.
pub fn check() -> Result<()> {
    let mut errors = Vec::new();
    for target in TARGETS {
        let metadata = Cmd::new("cargo")
            .args(["metadata", "--format-version", "1", "--locked", "--filter-platform", target])
            .dir(repo().join("desktop"))
            .stdout()?;
        let metadata: Value = serde_json::from_str(&metadata).map_err(|e| format!("cargo metadata: {e}"))?;
        errors.extend(violations(&metadata)?.into_iter().map(|error| format!("{target}: {error}")));
    }
    let github = repo().join(".github");
    for dir in ["workflows", "actions"] {
        let mut files = Vec::new();
        collect_yaml(&github.join(dir), &mut files);
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            for line in unscoped_mobile_builds(&text) {
                let file = file.strip_prefix(repo()).unwrap_or(&file).display();
                errors.push(format!(
                    "{file}:{line}: a build for a mobile target names its package (-p), never the workspace"
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{}\nHost kind is a crate boundary: {} violations (AGENTS.md)", errors.join("\n"), errors.len()))
    }
}

/// Every `.yml` below `dir`.
fn collect_yaml(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_yaml(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "yml") {
            found.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A `cargo metadata` answer: `members` are (directory, name) below `/ws`, `others` are packages of the registry,
    /// and `edges` are (from, to) by name.
    fn metadata(members: &[(&str, &str)], others: &[&str], edges: &[(&str, &str)]) -> Value {
        let packages: Vec<Value> = members
            .iter()
            .map(|(dir, name)| (name, format!("/ws/{dir}/Cargo.toml")))
            .chain(others.iter().map(|name| (name, format!("/registry/{name}/Cargo.toml"))))
            .map(|(name, manifest)| json!({"id": name, "name": name, "manifest_path": manifest}))
            .collect();
        let nodes: Vec<Value> = packages
            .iter()
            .map(|package| {
                let id = package["id"].as_str().unwrap();
                let deps: Vec<Value> =
                    edges.iter().filter(|(from, _)| *from == id).map(|(_, to)| json!({"pkg": to})).collect();
                json!({"id": id, "deps": deps})
            })
            .collect();
        let ids: Vec<&str> = members.iter().map(|(_, name)| *name).collect();
        json!({"workspace_root": "/ws", "packages": packages, "workspace_members": ids, "resolve": {"nodes": nodes}})
    }

    const WORKSPACE: [(&str, &str); 5] = [
        ("desktop", "sidevoice-desktop"),
        ("mobile", "sidevoice-mobile"),
        ("app-core", "sidevoice-app-core"),
        ("app-tauri", "sidevoice-app-tauri"),
        ("desktop/local-host", "sidevoice-local-host"),
    ];

    fn found(edges: &[(&str, &str)]) -> Vec<String> {
        let others = ["tauri", "tauri-runtime", "wry", "serde", "tauri-plugin-opener"];
        violations(&metadata(&WORKSPACE, &others, edges)).unwrap()
    }

    #[test]
    fn each_app_composes_the_shared_crates() {
        let edges = [
            ("sidevoice-desktop", "sidevoice-app-tauri"),
            ("sidevoice-desktop", "sidevoice-local-host"),
            ("sidevoice-mobile", "sidevoice-app-tauri"),
            ("sidevoice-app-tauri", "sidevoice-app-core"),
            ("sidevoice-app-tauri", "tauri"),
            ("sidevoice-app-core", "serde"),
            ("sidevoice-desktop", "tauri-plugin-opener"),
        ];
        assert_eq!(found(&edges), Vec::<String>::new());
    }

    #[test]
    fn a_host_does_not_depend_on_the_other_host_directly_or_through_anything() {
        let direct = found(&[("sidevoice-mobile", "sidevoice-desktop")]);
        assert_eq!(direct, ["mobile/ must not depend on desktop/: sidevoice-mobile -> sidevoice-desktop"]);
        let through = found(&[
            ("sidevoice-desktop", "sidevoice-local-host"),
            ("sidevoice-local-host", "serde"),
            ("sidevoice-mobile", "serde"),
            ("serde", "sidevoice-desktop"),
        ]);
        assert_eq!(through, ["mobile/ must not depend on desktop/: sidevoice-mobile -> serde -> sidevoice-desktop"]);
        assert_eq!(found(&[("sidevoice-desktop", "sidevoice-mobile")]).len(), 1, "the other way round too");
    }

    #[test]
    fn the_shared_crates_depend_on_neither_host() {
        let found = found(&[
            ("sidevoice-app-tauri", "sidevoice-app-core"),
            ("sidevoice-app-core", "sidevoice-desktop"),
            ("sidevoice-app-tauri", "sidevoice-mobile"),
        ]);
        assert_eq!(
            found,
            [
                "app-core/ must not depend on the host crate desktop/: sidevoice-app-core -> sidevoice-desktop",
                "app-tauri/ must not depend on the host crate desktop/: sidevoice-app-tauri -> sidevoice-app-core -> sidevoice-desktop",
                "app-tauri/ must not depend on the host crate mobile/: sidevoice-app-tauri -> sidevoice-mobile",
            ]
        );
    }

    #[test]
    fn the_core_has_no_shell_in_its_closure() {
        let direct = found(&[("sidevoice-app-core", "tauri")]);
        assert_eq!(direct, ["app-core/ must not depend on a shell (Tauri, wry, tao): sidevoice-app-core -> tauri"]);
        let through = found(&[("sidevoice-app-core", "serde"), ("serde", "tauri-plugin-opener")]);
        assert_eq!(through.len(), 1, "{through:?}");
        assert!(through[0].ends_with("sidevoice-app-core -> serde -> tauri-plugin-opener"));
        // The Tauri plugin may have the shell: it is the shell's binding.
        assert!(found(&[("sidevoice-app-tauri", "tauri"), ("sidevoice-app-tauri", "wry")]).is_empty());
    }

    #[test]
    fn a_crate_that_is_not_in_the_workspace_yet_has_nothing_to_violate() {
        let only_desktop = metadata(&[("desktop", "sidevoice-desktop")], &["tauri"], &[("sidevoice-desktop", "tauri")]);
        assert!(violations(&only_desktop).unwrap().is_empty());
        assert!(violations(&json!({"workspace_root": "/ws"})).is_err());
    }

    #[test]
    fn builds_for_a_mobile_target_name_their_package() {
        let ok = "\
steps:
  - name: cargo build --workspace --target aarch64-apple-ios
  - run: cargo build -p sidevoice-mobile --target aarch64-apple-ios-sim
  - run: cargo test --workspace
  - run: |
      cargo check \\
        --package sidevoice-app-core \\
        --target aarch64-linux-android
  # cargo build --workspace --target aarch64-apple-ios
  - run: cargo ndk -t arm64-v8a build -p sidevoice-mobile
";
        assert_eq!(unscoped_mobile_builds(ok), Vec::<usize>::new());
        let bad = "\
steps:
  - run: cargo build --target aarch64-apple-ios
  - run: cargo build --workspace --target=aarch64-linux-android
  - run: |
      cargo build \\
        --target aarch64-apple-ios-sim \\
        --all
  - run: cargo ndk -t arm64-v8a build
  - run: cargo build -p sidevoice-mobile --exclude sidevoice-desktop --target aarch64-apple-ios
  - run: cargo build -p sidevoice-app-core
";
        assert_eq!(unscoped_mobile_builds(bad), [2, 3, 5, 8, 9]);
    }
}
