//! Host kind is a crate boundary (D-RULE; AGENTS.md → "Host kind is a crate boundary"): what the Cargo workspace may
//! depend on, and how a mobile target is built. `platform_guard` rejects the conditions; this is the crates.
//!
//! The crates sit at the workspace root and each app composes the ones it uses:
//! - the host crates, `desktop/` and `mobile/`, depend on neither each other nor anything that depends on the other;
//! - the shared crates, `app-core/` and `app-tauri/`, depend on neither host crate;
//! - `app-core/` has no shell in its closure: no Tauri, no webview and no window crate, in any kind of dependency.
//!
//! They are read from `cargo metadata` with every feature on, once for each supported target: a dependency behind a
//! feature (`shell = ["dep:tauri"]`) is a dependency of the crate that has the feature, and one under a
//! `cfg(target_os)` is seen on the targets it applies to. This reads resolution only: it builds nothing, so it runs
//! before a crate has its iOS or Android bodies. A crate that is not in the workspace yet has nothing to violate and is
//! not looked for. (What the manifests say of the host kinds, as a feature or a target table, is `platform_guard`'s.)
//!
//! A build for a mobile target names its package (`-p`): the workspace holds the other kind of host too, and a
//! workspace-wide build for iOS or Android would try to build it. The steps of the workflows and composite actions
//! (`.github`, `.yml` and `.yaml`) are read for that, as parsed YAML and shell words, with a matrix's literal values.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use regex::Regex;
use serde_json::Value;
use serde_yaml_ng::Value as Yaml;

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

/// Where the workflow or composite action `text` builds for a mobile target without naming its package, or for the
/// whole workspace: a step whose `run` has a `cargo` command with a mobile `--target` (or `cargo ndk`) and no
/// `-p`/`--package`, or with `--workspace`, `--all` or `--exclude`.
///
/// The YAML is parsed, so its comments are not commands, and the shell is read word by word (quotes, `#` comments, `;`,
/// `&&`, `|`, a backslash at the end of a line). A `--target ${{ matrix.key }}` is read as each literal value the job's
/// matrix gives that key, row by row. A target that cannot be known (another expression, a shell variable, a matrix
/// made by `fromJSON`) counts as a mobile one: say it, or name the package.
pub fn unscoped_mobile_builds(text: &str) -> Result<Vec<String>> {
    let document: Yaml = serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
    // A workflow has `jobs.<id>.steps`; a composite action, `runs.steps`.
    let mut jobs: Vec<(String, &Yaml)> = Vec::new();
    if let Some(Yaml::Mapping(map)) = document.get("jobs") {
        jobs.extend(map.iter().filter_map(|(id, job)| Some((format!("job `{}`", id.as_str()?), job))));
    }
    if let Some(runs) = document.get("runs") {
        jobs.push(("the action".to_owned(), runs));
    }
    let mut places = Vec::new();
    for (place, job) in jobs {
        let matrix = matrix_rows(job.get("strategy").and_then(|strategy| strategy.get("matrix")));
        let steps = job.get("steps").and_then(Yaml::as_sequence).into_iter().flatten();
        for (number, step) in steps.enumerate() {
            let Some(script) = step.get("run").and_then(Yaml::as_str) else {
                continue;
            };
            let unscoped = expansions(script, &matrix)
                .iter()
                .any(|script| commands(script).iter().any(|command| unscoped_mobile_build(command)));
            if unscoped {
                places.push(format!("{place}, step {}", number + 1));
            }
        }
    }
    Ok(places)
}

fn scalar(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(text) => Some(text.clone()),
        Yaml::Number(number) => Some(number.to_string()),
        Yaml::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The rows of a job's matrix, as GitHub makes them: one for each combination of its lists (less the `exclude`d ones),
/// each `include` entry added to the rows it does not contradict, or a row of its own when it fits none. A matrix that
/// is an expression has no rows we can read.
fn matrix_rows(matrix: Option<&Yaml>) -> Vec<HashMap<String, String>> {
    let Some(Yaml::Mapping(matrix)) = matrix else {
        return Vec::new();
    };
    let entries = |key: &str| -> Vec<HashMap<String, String>> {
        let entries = matrix.get(key).and_then(Yaml::as_sequence).into_iter().flatten().filter_map(Yaml::as_mapping);
        entries
            .map(|entry| entry.iter().filter_map(|(k, v)| Some((k.as_str()?.to_owned(), scalar(v)?))).collect())
            .collect()
    };
    let lists: Vec<(String, Vec<String>)> = matrix
        .iter()
        .filter_map(|(key, value)| {
            Some((key.as_str()?.to_owned(), value.as_sequence()?.iter().filter_map(scalar).collect()))
        })
        .filter(|(key, _)| key != "include" && key != "exclude")
        .collect();
    let mut rows = vec![HashMap::new()];
    for (key, values) in &lists {
        rows = rows
            .iter()
            .flat_map(|row: &HashMap<String, String>| {
                values.iter().map(move |value| {
                    let mut row = row.clone();
                    row.insert(key.clone(), value.clone());
                    row
                })
            })
            .collect();
    }
    if lists.is_empty() {
        rows.clear();
    }
    let excluded = entries("exclude");
    rows.retain(|row| !excluded.iter().any(|exclude| exclude.iter().all(|(key, value)| row.get(key) == Some(value))));
    for include in entries("include") {
        // The keys the lists give are the original ones: an include never changes them, and adds the rest to the rows
        // it agrees with.
        let agrees = |row: &HashMap<String, String>| {
            lists.iter().all(|(key, _)| include.get(key).is_none_or(|value| row.get(key) == Some(value)))
        };
        let mut added = false;
        for row in rows.iter_mut().filter(|row| agrees(row)) {
            row.extend(include.iter().map(|(key, value)| (key.clone(), value.clone())));
            added = true;
        }
        if !added {
            rows.push(include);
        }
    }
    rows
}

/// `script` once for each row of the matrix, with the row's values in the `${{ matrix.key }}` expressions it names; an
/// expression stays where its row has no value for the key.
fn expansions(script: &str, rows: &[HashMap<String, String>]) -> Vec<String> {
    let expression = Regex::new(r"\$\{\{\s*matrix\.([A-Za-z0-9_-]+)\s*\}\}").expect("a regex");
    let with = |row: &HashMap<String, String>| {
        expression
            .replace_all(script, |found: &regex::Captures| {
                row.get(&found[1]).cloned().unwrap_or_else(|| found[0].to_owned())
            })
            .into_owned()
    };
    if rows.is_empty() {
        vec![script.to_owned()]
    } else {
        rows.iter().map(with).collect()
    }
}

/// The commands of a shell script, each as its words: quotes taken off, a `${{ … }}` expression kept whole (GitHub
/// substitutes it before the shell runs, quotes or not), `#` comments dropped, a backslash-newline joining lines, and
/// `;`, `&`, `|` and newlines ending a command.
fn commands(script: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = script.chars().collect();
    let (mut commands, mut words, mut word) = (Vec::new(), Vec::new(), String::new());
    let (mut quote, mut in_word, mut at) = (None::<char>, false, 0);
    while at < chars.len() {
        let c = chars[at];
        if c == '$' && chars.get(at + 1) == Some(&'{') && chars.get(at + 2) == Some(&'{') {
            let end = (at..chars.len().saturating_sub(1)).find(|&i| chars[i] == '}' && chars[i + 1] == '}');
            let end = end.map_or(chars.len(), |end| end + 2);
            word.extend(&chars[at..end]);
            (in_word, at) = (true, end);
            continue;
        }
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') if at + 1 < chars.len() => {
                at += 1;
                word.push(chars[at]);
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => (quote, in_word) = (Some(c), true),
            (None, '\\') => {
                // A backslash-newline joins the lines; before anything else it quotes the next character.
                at += 1;
                if let Some(&next) = chars.get(at).filter(|next| **next != '\n') {
                    word.push(next);
                    in_word = true;
                }
            }
            (None, '#') if !in_word => {
                while chars.get(at + 1).is_some_and(|next| *next != '\n') {
                    at += 1;
                }
            }
            (None, c) if c.is_whitespace() && c != '\n' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                }
                in_word = false;
            }
            (None, '\n' | ';' | '&' | '|') => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                }
                in_word = false;
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
        at += 1;
    }
    if in_word {
        words.push(word);
    }
    if !words.is_empty() {
        commands.push(words);
    }
    commands
}

/// Words that run what follows them: `env FOO=bar cargo …`, `flock lock cargo …`, `time`, `nice -n 5`, `sh -c '…'`. Their
/// own arguments (options, a lock file, a duration) are not read: the command is the first `cargo` after them, or a
/// quoted script.
const WRAPPERS: [&str; 21] = [
    "env", "time", "nice", "ionice", "nohup", "setsid", "stdbuf", "timeout", "flock", "exec", "command", "sudo", "sh",
    "bash", "zsh", "dash", "then", "do", "else", "if", "while",
];

/// Whether one command is a `cargo` build for a mobile target (or `cargo ndk`, which is Android's) that does not name
/// its package, or names the whole workspace. Behind a wrapper (`WRAPPERS`) it is the same command.
fn unscoped_mobile_build(words: &[String]) -> bool {
    // `CC=zigcc cargo …`: the leading assignments are not the command.
    let assignment = |word: &String| {
        word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    let Some(at) = words.iter().position(|word| !assignment(word)) else {
        return false;
    };
    let program = Path::new(&words[at]).file_name().map(|name| name.to_string_lossy().into_owned());
    match program.as_deref() {
        Some("cargo") => {}
        Some(wrapper) if WRAPPERS.contains(&wrapper) => {
            let rest = &words[at + 1..];
            let cargo = rest.iter().position(|word| Path::new(word).file_name().is_some_and(|name| name == "cargo"));
            // A script handed to the wrapper (`flock lock -c 'cargo …'`, `sh -c "…"`) is read as commands.
            let script = |word: &String| word.contains(char::is_whitespace);
            return cargo.is_some_and(|cargo| unscoped_mobile_build(&rest[cargo..]))
                || rest
                    .iter()
                    .filter(|word| script(word))
                    .any(|word| commands(word).iter().any(|c| unscoped_mobile_build(c)));
        }
        _ => return false,
    }
    let args = &words[at + 1..];
    let targets = args.iter().enumerate().filter_map(|(at, arg)| match arg.strip_prefix("--target=") {
        Some(target) => Some(target),
        None => (arg == "--target").then(|| args.get(at + 1).map(String::as_str)).flatten(),
    });
    let mobile = args.first().is_some_and(|arg| arg == "ndk")
        || targets
            .into_iter()
            .any(|target| target.contains("-ios") || target.contains("-android") || target.contains('$'));
    let scoped = args
        .iter()
        .any(|arg| arg == "-p" || arg.starts_with("--package") || (arg.starts_with("-p") && !arg.starts_with("--")));
    let wide = args.iter().any(|arg| matches!(arg.as_str(), "--workspace" | "--all") || arg.starts_with("--exclude"));
    mobile && (!scoped || wide)
}

/// `cargo xtask host-graph`: the workspace's dependency graph on each supported target, and the workflows' builds for
/// mobile targets.
pub fn check() -> Result<()> {
    let mut errors = Vec::new();
    for target in TARGETS {
        let metadata = resolve(&repo().join("desktop"), target, true)?;
        errors.extend(violations(&metadata)?.into_iter().map(|error| format!("{target}: {error}")));
    }
    let github = repo().join(".github");
    for dir in ["workflows", "actions"] {
        let mut files = Vec::new();
        collect_yaml(&github.join(dir), &mut files);
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let file = file.strip_prefix(repo()).unwrap_or(&file).display();
            for place in unscoped_mobile_builds(&text).map_err(|e| format!("{file}: {e}"))? {
                errors.push(format!(
                    "{file}: {place}: a build for a mobile target names its package (-p), never the workspace"
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

/// `cargo metadata` of the workspace around `dir` on `target`, with every feature on: a dependency behind a feature
/// (`shell = ["dep:tauri"]`) is a dependency of the crate that has the feature, and it is what a build with that
/// feature gets.
fn resolve(dir: &Path, target: &str, locked: bool) -> Result<Value> {
    let mut command = Cmd::new("cargo").args(["metadata", "--format-version", "1", "--all-features"]);
    if locked {
        command = command.arg("--locked");
    }
    let metadata = command.args(["--filter-platform", target]).dir(dir).stdout()?;
    serde_json::from_str(&metadata).map_err(|e| format!("cargo metadata: {e}"))
}

/// Every `.yml` and `.yaml` below `dir`.
fn collect_yaml(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_yaml(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "yml" || extension == "yaml") {
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

    /// A cargo workspace on disk: `crates` are (directory, manifest) below its root.
    fn workspace(crates: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let members: Vec<String> = crates.iter().map(|(name, _)| format!("\"{name}\"")).collect();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            format!("[workspace]\nresolver = \"2\"\nmembers = [{}]\n", members.join(", ")),
        )
        .unwrap();
        for (name, manifest) in crates {
            std::fs::create_dir_all(dir.path().join(name).join("src")).unwrap();
            std::fs::write(dir.path().join(name).join("Cargo.toml"), manifest).unwrap();
            std::fs::write(dir.path().join(name).join("src/lib.rs"), "").unwrap();
        }
        dir
    }

    #[test]
    fn a_dependency_behind_a_feature_is_a_dependency() {
        // The shell is optional, behind a feature of `app-core` that no default turns on.
        let package = |name: &str, extra: &str| {
            format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n{extra}")
        };
        let core = package(
            "sidevoice-app-core",
            "[dependencies]\ntauri = { path = \"../tauri\", optional = true }\n[features]\nshell = [\"dep:tauri\"]\n",
        );
        let dir = workspace(&[("app-core", &core), ("tauri", &package("tauri", ""))]);
        let metadata = resolve(dir.path(), "x86_64-unknown-linux-gnu", false).unwrap();
        assert_eq!(
            violations(&metadata).unwrap(),
            ["app-core/ must not depend on a shell (Tauri, wry, tao): sidevoice-app-core -> tauri"]
        );
    }

    #[test]
    fn builds_for_a_mobile_target_name_their_package() {
        let found = |text: &str| unscoped_mobile_builds(text).unwrap();
        let ok = "\
jobs:
  desktop:
    steps:
      - name: cargo build --workspace --target aarch64-apple-ios
        run: cargo test --workspace
      - run: cargo build -p sidevoice-mobile --target aarch64-apple-ios-sim
      - run: |
          # cargo build --workspace --target aarch64-apple-ios
          cargo check \\
            --package sidevoice-app-core \\
            --target aarch64-linux-android
          CC=zigcc cargo ndk -t arm64-v8a build -psidevoice-mobile
      - run: echo 'cargo build --target aarch64-apple-ios'
  desktop-matrix:
    strategy:
      matrix:
        target: [x86_64-unknown-linux-gnu, aarch64-apple-darwin]
    steps:
      - run: cargo build --target ${{ matrix.target }}
  mobile-matrix:
    strategy:
      matrix:
        target: [aarch64-apple-ios]
    steps:
      - run: cargo build -p sidevoice-mobile --target ${{ matrix.target }}
  # Include entries are whole rows: (linux, --workspace) and (ios, -p mobile) never make (ios, --workspace).
  matrix-rows:
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            scope: --workspace
          - target: aarch64-apple-ios
            scope: -p sidevoice-mobile
    steps:
      - run: cargo build ${{ matrix.scope }} --target ${{ matrix.target }}
  matrix-merged:
    strategy:
      matrix:
        os: [macos-15, ubuntu-24.04]
        include:
          - os: macos-15
            target: aarch64-apple-ios
            scope: -p sidevoice-mobile
          - os: ubuntu-24.04
            target: x86_64-unknown-linux-gnu
            scope: --workspace
    steps:
      - run: cargo build ${{ matrix.scope }} --target ${{ matrix.target }}
  matrix-excluded:
    strategy:
      matrix:
        target: [aarch64-apple-ios, x86_64-unknown-linux-gnu]
        scope: [-p sidevoice-mobile, --workspace]
        exclude:
          - target: aarch64-apple-ios
            scope: --workspace
    steps:
      - run: cargo build ${{ matrix.scope }} --target ${{ matrix.target }}
  wrapped:
    steps:
      - run: env FOO=bar cargo build -p sidevoice-mobile --target aarch64-apple-ios
      - run: flock /tmp/cargo-build.lock cargo test --workspace
      - run: time cargo build --target x86_64-unknown-linux-gnu
";
        assert_eq!(found(ok), Vec::<String>::new());
        let bad = "\
jobs:
  plain:
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
      - run: cd mobile && CC=zigcc cargo build --release --target aarch64-apple-ios
  commented:
    steps:
      - run: cargo build --target aarch64-apple-ios # -p sidevoice-mobile
      - run: |
          cargo build --target aarch64-apple-ios # --package sidevoice-mobile
  matrix-unknown:
    steps:
      - run: cargo build --target ${{ matrix.target }}
      - run: cargo build --target \"$TARGET\"
  matrix-mobile:
    strategy:
      matrix:
        os: [macos-15]
        include:
          - target: x86_64-apple-ios
    steps:
      - run: cargo build --target ${{ matrix.target }}
  matrix-expression:
    strategy:
      matrix: ${{ fromJSON(needs.plan.outputs.matrix) }}
    steps:
      - run: cargo build --target ${{ matrix.target }}
  matrix-row:
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            scope: -p sidevoice-desktop
          - target: aarch64-apple-ios
            scope: --workspace
    steps:
      - run: cargo build ${{ matrix.scope }} --target ${{ matrix.target }}
  wrapped:
    steps:
      - run: env FOO=bar cargo build --workspace --target aarch64-apple-ios
      - run: time cargo build --target aarch64-apple-ios
      - run: nice -n 10 cargo build --target aarch64-linux-android
      - run: flock /tmp/cargo-build.lock cargo build --workspace --target aarch64-apple-ios
      - run: flock -w 600 /tmp/lock -c 'cargo build --target aarch64-apple-ios'
      - run: sudo -E env CC=zigcc cargo ndk -t arm64-v8a build
      - run: bash -c \"cargo build --target aarch64-apple-ios\"
";
        let bad: Vec<String> = found(bad);
        let steps = |job: &str| bad.iter().filter(|place| place.starts_with(&format!("job `{job}`"))).count();
        assert_eq!(
            bad[..7],
            [
                "job `plain`, step 1",
                "job `plain`, step 2",
                "job `plain`, step 3",
                "job `plain`, step 4",
                "job `plain`, step 5",
                "job `plain`, step 7",
                "job `commented`, step 1"
            ]
        );
        assert_eq!(steps("commented"), 2, "{bad:?}");
        assert_eq!(steps("matrix-unknown"), 2, "{bad:?}");
        assert_eq!(steps("matrix-mobile"), 1, "{bad:?}");
        assert_eq!(steps("matrix-expression"), 1, "{bad:?}");
        // Whole rows of a matrix: only (ios, --workspace) is a build of the workspace.
        assert_eq!(steps("matrix-row"), 1, "{bad:?}");
        // Behind a wrapper it is the same command.
        assert_eq!(steps("wrapped"), 7, "{bad:?}");
        // A composite action holds its steps under `runs`.
        let action = "runs:\n  using: composite\n  steps:\n    - run: cargo build --target aarch64-linux-android\n";
        assert_eq!(found(action), ["the action, step 1"]);
        assert!(unscoped_mobile_builds("jobs: [").is_err(), "an unreadable workflow is an error, never a pass");
    }

    #[test]
    fn workflows_are_found_whatever_their_extension() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.yml", "b.yaml", "c.txt", "d/e.yaml"] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let mut found = Vec::new();
        collect_yaml(dir.path(), &mut found);
        let mut names: Vec<String> =
            found.iter().map(|path| path.strip_prefix(dir.path()).unwrap().display().to_string()).collect();
        names.sort();
        assert_eq!(names, ["a.yml", "b.yaml", "d/e.yaml"]);
    }
}
