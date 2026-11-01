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
//! The one exception is `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` as a file-scope inner
//! attribute of a Cargo binary root (`src/main.rs`, `src/bin/*.rs`, `src/bin/*/main.rs` of a crate): the linker reads
//! it from the crate root, so it cannot move into `platform`. Any other predicate, value, placement or file counts.
//!
//! Inside `platform` the OS is allowed, but its shape is checked (recipe items 1-2):
//! - `platform/mod.rs` carries no OS `cfg`/`cfg_attr` and no `allow`/`expect` of `dead_code` or `unused*` lints, at
//!   any depth of an attribute (`cfg_attr(test, allow(dead_code))` included): an OS that cannot do something says so
//!   through the interface;
//! - no item of a trait declared in `platform` is gated by an OS condition (`cfg(target_os = ..)`,
//!   `cfg(any(target_os = .., test))`, `cfg_attr`): the OS file implements the common method, and anything of one
//!   OS stays in that OS's file.
//!   An OS condition on the trait, on an inline module around it or on the whole file counts the same.
//!
//! Host kinds are not OS conditions, and have no place anywhere, `platform` included (D-RULE, AGENTS.md → "Host kind is
//! a crate boundary"). What counts, in every Rust file, and in every Cargo manifest:
//! - `desktop` or `mobile` (the conditions tauri-build defines) in `cfg`, `cfg_attr` or `cfg!`, and
//!   `feature = "desktop"` or `feature = "mobile"` there;
//! - a `cfg_aliases!` entry named `desktop` or `mobile`, or built on one, and every use of such an alias;
//! - a host kind given to a `macro_rules!` that puts its argument in a `cfg` (`gate!(mobile)` for
//!   `($c:meta) => { #[cfg($c)] .. }`), wherever the macro is defined: only an argument that reaches the `cfg` counts, by
//!   its position among the call's comma-separated ones (`named!(test, desktop)` names a function `desktop`), except
//!   where the matcher is not a plain list of fragments (a repetition, a separator other than the comma), which reads
//!   every argument;
//! - a build script that prints `cargo:rustc-cfg=desktop` or `=mobile` (or declares one with `rustc-check-cfg`);
//! - in a `Cargo.toml`: a feature named `desktop` or `mobile`, a `[target.'cfg(..)']` table whose predicate names one,
//!   and a `check-cfg` lint that declares one.
//!
//! Code for one kind of host lives in the crate of that host; `host_graph` checks the crates' dependencies.
//!
//! Limits, by design of a token scan: a program name or OS variable reaching `Command::new` through a constant
//! or a computed value, a bare function imported by glob (`use libc::*`) and the arms of `cfg_select!` are not
//! followed; a host kind reaching a `cfg` through a macro that is not a `macro_rules!` of the repository, or through a
//! second macro, is not either (a scan is not an expander); and an alias that groups OS values under a name of its
//! own (`any(target_os = "ios", ..)` as `handheld`) is for review, not for this scan.
use std::{
    collections::{BTreeSet, HashMap, HashSet},
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
/// The host kinds (D-RULE): tauri-build's `desktop` and `mobile` conditions, and the Cargo features by those names.
const HOST_KINDS: [&str; 2] = ["desktop", "mobile"];
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

/// Host kinds anywhere in a predicate: the conditions `desktop` and `mobile`, an alias built on one, and
/// `feature = "desktop"` or `feature = "mobile"`.
fn names_host(stream: &TokenStream, aliases: &HashSet<String>) -> bool {
    let trees = flat(stream.clone());
    trees.iter().enumerate().any(|(at, tree)| match tree {
        TokenTree::Ident(id) => {
            let name = id.to_string();
            let feature = name == "feature"
                && punct(trees.get(at + 1), '=')
                && matches!(trees.get(at + 2), Some(TokenTree::Literal(lit))
                    if string_value(lit).is_some_and(|value| HOST_KINDS.contains(&value.as_str())));
            feature || HOST_KINDS.contains(&name.as_str()) || aliases.contains(&name)
        }
        TokenTree::Group(g) => names_host(&g.stream(), aliases),
        _ => false,
    })
}

/// Whether one attribute (`name` and the tokens of its parentheses) is an OS condition: a `cfg` predicate, a
/// `cfg_attr` predicate, or a `cfg`/`cfg_attr`/`windows_subsystem` among the attributes a `cfg_attr` applies.
/// Other payloads (`doc = stringify!(unix)`, `derive(..)`) are data, not conditions.
fn os_attr(name: &str, args: &TokenStream, aliases: &HashSet<String>) -> bool {
    condition_attr(name, args, &|predicate| names_os(predicate, aliases), &|other| other == "windows_subsystem")
}

/// [`os_attr`] for the host kinds: no attribute is one by itself, only a predicate that names one.
fn host_attr(name: &str, args: &TokenStream, aliases: &HashSet<String>) -> bool {
    condition_attr(name, args, &|predicate| names_host(predicate, aliases), &|_| false)
}

/// Whether one attribute holds a condition: a `cfg` predicate, a `cfg_attr` predicate, or a `cfg`/`cfg_attr` among the
/// attributes a `cfg_attr` applies, as `names` says of a predicate; or any other attribute `other` takes for one.
fn condition_attr(
    name: &str,
    args: &TokenStream,
    names: &dyn Fn(&TokenStream) -> bool,
    other: &dyn Fn(&str) -> bool,
) -> bool {
    match name {
        "cfg" => names(args),
        "cfg_attr" => {
            let parts = flat(args.clone());
            let mut parts = parts.split(|tree| punct(Some(tree), ','));
            let predicate: TokenStream = parts.next().unwrap_or_default().iter().cloned().collect();
            names(&predicate)
                || parts.any(|part| {
                    let nested = part.first().and_then(ident).unwrap_or_default();
                    let args = group(part.get(1), Delimiter::Parenthesis).unwrap_or_default();
                    condition_attr(&nested, &args, names, other)
                })
        }
        name => other(name),
    }
}

/// The one OS attribute allowed outside `platform`: `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`,
/// as a file-scope inner attribute of a Cargo binary root (see [`binary_root`]). The linker reads it from the crate
/// root, so it cannot live in a module; no other predicate, value or placement is accepted in its place.
fn console_subsystem(body: &[TokenTree]) -> bool {
    if body.first().and_then(ident).as_deref() != Some("cfg_attr") {
        return false;
    }
    let args = group(body.get(1), Delimiter::Parenthesis).unwrap_or_default();
    let parts = flat(args);
    let mut parts = parts.split(|tree| punct(Some(tree), ','));
    let predicate = parts.next().unwrap_or_default();
    let attribute = parts.next().unwrap_or_default();
    let not_debug = predicate.len() == 2
        && ident(&predicate[0]).as_deref() == Some("not")
        && group(predicate.get(1), Delimiter::Parenthesis).is_some_and(
            |inner| matches!(flat(inner).as_slice(), [only] if ident(only).as_deref() == Some("debug_assertions")),
        );
    not_debug
        && parts.next().is_none()
        && attribute.len() == 3
        && ident(&attribute[0]).as_deref() == Some("windows_subsystem")
        && punct(attribute.get(1), '=')
        && matches!(attribute.get(2), Some(TokenTree::Literal(lit)) if string_value(lit).as_deref() == Some("windows"))
}

/// Whether `path` (relative to the repository `root`) is the root of a Cargo binary: `src/main.rs`, `src/bin/*.rs` or
/// `src/bin/*/main.rs` of a crate (a folder holding a `Cargo.toml`). A `[[bin]] path` elsewhere is not followed.
fn binary_root(root: &Path, path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let crate_dir = match parts.as_slice() {
        [dir @ .., "src", "main.rs"] => dir,
        [dir @ .., "src", "bin", name] if name.ends_with(".rs") => dir,
        [dir @ .., "src", "bin", _, "main.rs"] => dir,
        _ => return false,
    };
    root.join(crate_dir.join("/")).join("Cargo.toml").is_file()
}

/// [`os_attr`] on the tokens inside `#[..]`.
fn os_attr_tokens(body: &[TokenTree], aliases: &HashSet<String>) -> bool {
    let name = body.first().and_then(ident).unwrap_or_default();
    let args = group(body.get(1), Delimiter::Parenthesis).unwrap_or_default();
    os_attr(&name, &args, aliases)
}

fn os_attr_syn(attribute: &syn::Attribute, aliases: &HashSet<String>) -> bool {
    let name = attribute.path().get_ident().map(ToString::to_string).unwrap_or_default();
    let args = match &attribute.meta {
        syn::Meta::List(list) => list.tokens.clone(),
        _ => TokenStream::new(),
    };
    os_attr(&name, &args, aliases)
}

/// Whether an attribute removes the item it sits on on some OS: a `cfg` with an OS predicate, or a `cfg_attr`
/// that applies one, or applies any `cfg` on an OS (`cfg_attr(windows, cfg(feature = "x"))`). An OS-conditional lint allowance (`cfg_attr(windows, allow(..))`) keeps the item everywhere.
fn removes_on_os(attribute: &syn::Attribute, aliases: &HashSet<String>) -> bool {
    fn nested(name: &str, args: &TokenStream, aliases: &HashSet<String>, under_os: bool) -> bool {
        match name {
            "cfg" => under_os || names_os(args, aliases),
            "cfg_attr" => {
                let parts = flat(args.clone());
                let mut parts = parts.split(|tree| punct(Some(tree), ','));
                let predicate: TokenStream = parts.next().unwrap_or_default().iter().cloned().collect();
                let under_os = under_os || names_os(&predicate, aliases);
                parts.any(|part| {
                    let name = part.first().and_then(ident).unwrap_or_default();
                    nested(&name, &group(part.get(1), Delimiter::Parenthesis).unwrap_or_default(), aliases, under_os)
                })
            }
            _ => false,
        }
    }
    let name = attribute.path().get_ident().map(ToString::to_string).unwrap_or_default();
    match &attribute.meta {
        syn::Meta::List(list) => nested(&name, &list.tokens, aliases, false),
        _ => false,
    }
}

/// Traits of a platform file whose items are not the same on every OS. The OS conditions that enclose a trait (its
/// own, its inline modules', the file's inner attributes) count as much as the ones on its items.
struct TraitShape<'a> {
    aliases: &'a HashSet<String>,
    gates: usize,
    lines: Vec<usize>,
}

impl TraitShape<'_> {
    fn gated(&self, attributes: &[syn::Attribute]) -> bool {
        attributes.iter().any(|attribute| removes_on_os(attribute, self.aliases))
    }
}

impl<'ast> syn::visit::Visit<'ast> for TraitShape<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        let gate = usize::from(self.gated(&item.attrs));
        self.gates += gate;
        syn::visit::visit_item_mod(self, item);
        self.gates -= gate;
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        if self.gates > 0 || self.gated(&item.attrs) {
            self.lines.push(item.trait_token.span.start().line);
        }
        for member in &item.items {
            let attributes = match member {
                syn::TraitItem::Const(m) => &m.attrs,
                syn::TraitItem::Fn(m) => &m.attrs,
                syn::TraitItem::Type(m) => &m.attrs,
                syn::TraitItem::Macro(m) => &m.attrs,
                _ => continue,
            };
            for attribute in attributes.iter().filter(|a| os_attr_syn(a, self.aliases)) {
                self.lines.push(attribute.pound_token.span.start().line);
            }
        }
        // Default-method bodies may hold other traits (and modules); members' own gates were counted above.
        syn::visit::visit_item_trait(self, item);
    }
}

fn string_value(literal: &proc_macro2::Literal) -> Option<String> {
    syn::parse_str::<syn::LitStr>(&literal.to_string()).ok().map(|lit| lit.value())
}

/// `(path, rename)` of every leaf of a `use` tree; a glob's rename is `*`.
fn leaves(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<(Vec<String>, Option<String>)>) {
    let name = |id: &syn::Ident| id.to_string().trim_start_matches("r#").to_string();
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(name(&p.ident));
            leaves(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut path = prefix.clone();
            if n.ident != "self" {
                path.push(name(&n.ident));
            }
            out.push((path, None));
        }
        syn::UseTree::Rename(r) => {
            let mut path = prefix.clone();
            if r.ident != "self" {
                path.push(name(&r.ident));
            }
            out.push((path, Some(name(&r.rename))));
        }
        syn::UseTree::Glob(_) => out.push((prefix.clone(), Some("*".into()))),
        syn::UseTree::Group(g) => g.items.iter().for_each(|item| leaves(item, prefix, out)),
    }
}

/// Whether a resolved path is an OS API.
fn os_path(path: &[String]) -> bool {
    let at = |i: usize| path.get(i).map(String::as_str);
    match at(0) {
        Some("std" | "core" | "alloc") => {
            at(1) == Some("os")
                || (at(1) == Some("env") && at(2) == Some("consts") && matches!(at(3), Some("OS" | "FAMILY")))
        }
        // Local sockets and pipes: Unix sockets, Windows named pipes, and the crate that abstracts them.
        Some("tokio") => {
            (at(1) == Some("net")
                && (at(2).is_some_and(|name| name.starts_with("Unix")) || matches!(at(2), Some("unix" | "windows"))))
                || (at(1) == Some("signal") && matches!(at(2), Some("unix" | "windows")))
        }
        Some("mio") => at(1) == Some("net") && at(2).is_some_and(|name| name.starts_with("Unix")),
        Some("interprocess") => true,
        Some(root) => is_os_crate(root),
        None => false,
    }
}

/// Whether a string is a path that exists on one OS only (`/proc/…`, `C:\…`, a named pipe).
fn os_path_literal(value: &str) -> bool {
    const UNIX_ROOTS: [&str; 14] = [
        "/proc/",
        "/sys/",
        "/dev/",
        "/etc/",
        "/usr/",
        "/bin/",
        "/sbin/",
        "/var/",
        "/run/",
        "/tmp/",
        "/Library/",
        "/System/",
        "/Applications/",
        "/Users/",
    ];
    let bytes = value.as_bytes();
    UNIX_ROOTS.iter().any(|root| value.starts_with(root) || value == root.trim_end_matches('/'))
        || value.starts_with("~/Library")
        || value.starts_with("\\\\.\\")
        || value.starts_with("\\\\?\\")
        || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/'))
}

/// Free functions and constructors whose argument is a filesystem path.
const FS_FUNCTIONS: [&str; 22] = [
    "read",
    "read_to_string",
    "read_dir",
    "read_link",
    "write",
    "open",
    "create",
    "create_new",
    "create_dir",
    "create_dir_all",
    "remove_file",
    "remove_dir",
    "remove_dir_all",
    "metadata",
    "symlink_metadata",
    "canonicalize",
    "copy",
    "rename",
    "exists",
    "try_exists",
    "set_permissions",
    "hard_link",
];
/// Methods whose argument is a path or a command line.
const PATH_METHODS: [&str; 5] = ["arg", "args", "join", "current_dir", "open"];

/// Shared by every file: names a build script or another module defines for the rest.
#[derive(Default, Clone, PartialEq)]
struct Shared {
    /// OS conditions defined by `cfg_aliases!` (a build script defines what other files use).
    cfg_aliases: HashSet<String>,
    /// Host kinds defined the same way: entries named like one or built on one.
    host_aliases: HashSet<String>,
    /// `macro_rules!` that put an argument in a `cfg` predicate (`gate!(mobile)` is `#[cfg(mobile)]`), and which
    /// arguments reach it.
    gate_macros: HashMap<String, Reach>,
    /// Types that wrap a process `Command` (`struct Cmd { command: Command }`, `type Run = Command`).
    command_types: HashSet<String>,
}

fn is_command_type(name: &str, shared: &Shared) -> bool {
    name.ends_with("Command") || name.ends_with("CommandWrap") || shared.command_types.contains(name)
}

#[derive(Default)]
struct Scan {
    shared: Shared,
    /// Every path each name in this file is imported as (`use libc as sys;` binds `sys` to `libc`).
    imports: HashMap<String, Vec<Vec<String>>>,
    lines: BTreeSet<usize>,
    /// Literals already reported, by position.
    literals: HashSet<(usize, usize)>,
    count: usize,
    /// First pass: only learn names, report nothing.
    collecting: bool,
    /// The file is a Cargo binary root, where the console-subsystem crate attribute is allowed at file scope.
    crate_root: bool,
    /// How many groups (braces, parentheses, brackets) enclose the tokens being walked; 0 is file scope.
    depth: usize,
}

/// The path starting at `trees[at]` (`a::b::<T>::c`): its segments and the index after it.
fn parse_path(trees: &[TokenTree], at: usize) -> (Vec<String>, usize) {
    let mut segments: Vec<String> = ident(&trees[at]).into_iter().collect();
    let mut end = at + 1;
    while path_sep(trees, end) {
        match trees.get(end + 2) {
            Some(tree) if ident(tree).is_some() => {
                segments.extend(ident(tree));
                end += 3;
            }
            Some(TokenTree::Punct(p)) if p.as_char() == '<' => {
                let mut depth = 0;
                let mut k = end + 2;
                while k < trees.len() {
                    if punct(trees.get(k), '<') {
                        depth += 1;
                    } else if punct(trees.get(k), '>') && !(k > 0 && punct(trees.get(k - 1), '-')) {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                end = (k + 1).min(trees.len());
            }
            _ => break,
        }
    }
    (segments, end)
}

fn any_group(tree: Option<&TokenTree>) -> Option<TokenStream> {
    match tree {
        Some(TokenTree::Group(g)) => Some(g.stream()),
        _ => None,
    }
}

fn first_string(stream: TokenStream) -> Option<String> {
    match stream.into_iter().next() {
        Some(TokenTree::Literal(lit)) => string_value(&lit),
        _ => None,
    }
}

fn cargo_os_variable(value: &str) -> bool {
    value.strip_prefix("CARGO_CFG_").is_some_and(|name| OS_ENV_SUFFIXES.contains(&name))
}

/// `#[link(name = "AppKit", kind = "framework")]`, also nested in `cfg_attr`.
fn native_os_link(trees: &[TokenTree]) -> bool {
    trees.iter().enumerate().any(|(i, tree)| match tree {
        TokenTree::Group(g) => native_os_link(&flat(g.stream())),
        TokenTree::Ident(id) if id == "link" => group(trees.get(i + 1), Delimiter::Parenthesis).is_some_and(|args| {
            flat(args).iter().any(|arg| {
                matches!(arg, TokenTree::Literal(lit)
                    if string_value(lit).is_some_and(|kind| matches!(kind.as_str(), "framework" | "raw-dylib")))
            })
        }),
        _ => false,
    })
}

impl Scan {
    fn found(&mut self, tree: &TokenTree) {
        if !self.collecting {
            self.count += 1;
            self.lines.insert(tree.span().start().line);
        }
    }

    /// Every way `segments` can be read once the imports of this file are expanded.
    fn expand(&self, segments: &[String], depth: usize) -> Vec<Vec<String>> {
        let imported = segments.first().and_then(|first| self.imports.get(first));
        match imported {
            Some(paths) if depth > 0 => paths
                .iter()
                .flat_map(|path| {
                    let mut full = path.clone();
                    full.extend_from_slice(&segments[1..]);
                    if full.first() == segments.first() {
                        vec![full]
                    } else {
                        self.expand(&full, depth - 1)
                    }
                })
                .collect(),
            _ => vec![segments.to_vec()],
        }
    }

    fn bind(&mut self, at: &TokenTree, leaves: Vec<(Vec<String>, Option<String>)>) {
        for (path, rename) in leaves {
            if os_path(&path) {
                self.found(at);
            }
            let name = rename.or_else(|| path.last().cloned());
            if let Some(name) = name.filter(|name| name != "*" && name != "_") {
                let paths = self.imports.entry(name).or_default();
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }

    /// An OS path inside the arguments of a filesystem or command operation counts once per literal.
    fn path_literals(&mut self, trees: &[TokenTree]) {
        for tree in trees {
            match tree {
                TokenTree::Group(g) => self.path_literals(&flat(g.stream())),
                TokenTree::Literal(lit) => {
                    let start = lit.span().start();
                    if string_value(lit).is_some_and(|value| os_path_literal(&value))
                        && (self.collecting || self.literals.insert((start.line, start.column)))
                    {
                        self.found(tree);
                    }
                }
                _ => {}
            }
        }
    }

    /// The shape of `platform/mod.rs`: no OS conditions, no lint allowances for code unused on an OS.
    fn walk_root(&mut self, trees: &[TokenTree]) {
        let mut i = 0;
        while i < trees.len() {
            match &trees[i] {
                TokenTree::Group(g) => self.walk_root(&flat(g.stream())),
                TokenTree::Punct(p) if p.as_char() == '#' => {
                    let at = i + usize::from(punct(trees.get(i + 1), '!')) + 1;
                    if let Some(inner) = group(trees.get(at), Delimiter::Bracket) {
                        let body = flat(inner);
                        if os_attr_tokens(&body, &self.shared.cfg_aliases) || allows_unused(&body) {
                            self.found(&trees[i]);
                        }
                        i = at;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn walk(&mut self, trees: &[TokenTree]) {
        let mut i = 0;
        while i < trees.len() {
            match &trees[i] {
                TokenTree::Group(g) => {
                    self.depth += 1;
                    self.walk(&flat(g.stream()));
                    self.depth -= 1;
                }
                TokenTree::Punct(p) if p.as_char() == '#' => {
                    let at = i + usize::from(punct(trees.get(i + 1), '!')) + 1;
                    if let Some(inner) = group(trees.get(at), Delimiter::Bracket) {
                        let body = flat(inner.clone());
                        let name = body.first().and_then(ident);
                        let cfg = matches!(name.as_deref(), Some("cfg" | "cfg_attr"));
                        let inner = punct(trees.get(i + 1), '!');
                        let allowed = self.crate_root && self.depth == 0 && inner && console_subsystem(&body);
                        if !allowed && (name.as_deref() == Some("windows_subsystem") || native_os_link(&body)) {
                            self.found(&trees[i]);
                        }
                        if !allowed && cfg && os_attr_tokens(&body, &self.shared.cfg_aliases) {
                            self.found(&trees[i]);
                        }
                        // Other attributes can carry paths and strings (`doc`): scan them like any tokens.
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
                TokenTree::Literal(_) | TokenTree::Punct(_) => {}
            }
            i += 1;
        }
    }

    /// Handles the identifier at `i`; returns the next index to visit.
    fn item(&mut self, trees: &[TokenTree], i: usize) -> usize {
        let tree = &trees[i];
        let name = ident(tree).unwrap_or_default();
        let bang = punct(trees.get(i + 1), '!');
        if i > 0 && punct(trees.get(i - 1), '.') {
            // A method call: `.arg("/dev/null")`, `.join("C:\\x")`.
            if PATH_METHODS.contains(&name.as_str()) {
                if let Some(args) = group(trees.get(i + 1), Delimiter::Parenthesis) {
                    self.path_literals(&flat(args));
                }
            }
            return i + 1;
        }
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
                let rename = item.rename.map(|(_, id)| id.to_string().trim_start_matches("r#").to_string());
                self.bind(tree, vec![(vec![item.ident.to_string()], rename)]);
            }
            return end + 1;
        }
        if matches!(name.as_str(), "struct" | "type") {
            self.learn_command_type(trees, i);
        }
        if bang && name == "cfg" {
            if let Some(inner) = any_group(trees.get(i + 2)) {
                if names_os(&inner, &self.shared.cfg_aliases) {
                    self.found(tree);
                }
                return i + 3;
            }
        }
        if bang && matches!(name.as_str(), "env" | "option_env") {
            if let Some(inner) = any_group(trees.get(i + 2)) {
                if first_string(inner).is_some_and(|value| cargo_os_variable(&value)) {
                    self.found(tree);
                }
                return i + 3;
            }
        }
        if bang && name == "cfg_aliases" {
            if let Some(TokenTree::Group(body)) = trees.get(i + 2) {
                self.define_cfg_aliases(&flat(body.stream()));
                return i + 3;
            }
        }
        if !mid_path(trees, i) {
            self.path(trees, i);
        }
        i + 1
    }

    /// `struct Cmd { command: Command, … }` and `type Run = Command;` make `Cmd`/`Run` a command type.
    fn learn_command_type(&mut self, trees: &[TokenTree], at: usize) {
        let Some(name) = trees.get(at + 1).and_then(ident) else {
            return;
        };
        let mut wraps = false;
        for tree in &trees[at + 2..] {
            match tree {
                TokenTree::Punct(p) if p.as_char() == ';' => break,
                TokenTree::Group(g) => {
                    wraps |= mentions_command(g.stream());
                    if g.delimiter() == Delimiter::Brace {
                        break;
                    }
                }
                TokenTree::Ident(id) => wraps |= is_command_type(&id.to_string(), &self.shared),
                _ => {}
            }
        }
        if wraps {
            self.shared.command_types.insert(name);
        }
    }

    /// A path in expression, type or pattern position, and the call that may follow it.
    fn path(&mut self, trees: &[TokenTree], at: usize) {
        let (segments, end) = parse_path(trees, at);
        let options = self.expand(&segments, 3);
        if segments.len() > 1 && options.iter().any(|path| os_path(path)) {
            self.found(&trees[at]);
        }
        let Some(args) = group(trees.get(end), Delimiter::Parenthesis) else {
            return;
        };
        let (mut program, mut cargo_var, mut fs_path) = (None, false, false);
        for path in &options {
            let [.., prev, last] = path.as_slice() else {
                continue;
            };
            if matches!(last.as_str(), "new" | "with_new") && is_command_type(prev, &self.shared) {
                program = program.or_else(|| first_string(args.clone()));
            }
            if matches!(last.as_str(), "var" | "var_os") && path.len() == 3 && path[..2] == ["std", "env"] {
                cargo_var |= first_string(args.clone()).is_some_and(|value| cargo_os_variable(&value));
            }
            fs_path |= (matches!(prev.as_str(), "fs" | "File" | "OpenOptions")
                && FS_FUNCTIONS.contains(&last.as_str()))
                || (matches!(prev.as_str(), "Path" | "PathBuf" | "OsStr" | "OsString")
                    && matches!(last.as_str(), "new" | "from"));
        }
        if let Some(program) = program {
            let file = program.rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
            let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".cmd")).unwrap_or(&file);
            if OS_PROGRAMS.contains(&stem) {
                self.found(&trees[at]);
            }
        }
        if cargo_var {
            self.found(&trees[at]);
        }
        if fs_path {
            self.path_literals(&flat(args));
        }
    }

    /// `cfg_aliases! { name: { condition }, … }`: each alias that names an OS counts, and becomes one.
    fn define_cfg_aliases(&mut self, body: &[TokenTree]) {
        for entry in body.split(|tree| punct(Some(tree), ',')) {
            let Some(alias) = entry.first().and_then(ident) else {
                continue;
            };
            let condition: TokenStream = entry[1..].iter().cloned().collect();
            if names_os(&condition, &self.shared.cfg_aliases) {
                self.found(&entry[0]);
                self.shared.cfg_aliases.insert(alias);
            }
        }
    }
}

/// `allow(dead_code)`, `expect(unused_imports)`… anywhere in an attribute, so `cfg_attr(x, allow(dead_code))` too.
fn allows_unused(attribute: &[TokenTree]) -> bool {
    attribute.iter().enumerate().any(|(i, tree)| match tree {
        TokenTree::Group(g) => allows_unused(&flat(g.stream())),
        TokenTree::Ident(id) if id == "allow" || id == "expect" => group(attribute.get(i + 1), Delimiter::Parenthesis)
            .is_some_and(|lints| {
                lints.into_iter().any(|lint| {
                    matches!(&lint, TokenTree::Ident(name)
                        if name == "dead_code" || name.to_string().starts_with("unused"))
                })
            }),
        _ => false,
    })
}

fn mentions_command(stream: TokenStream) -> bool {
    stream.into_iter().any(|tree| match tree {
        TokenTree::Ident(id) => {
            let name = id.to_string();
            name.ends_with("Command") || name.ends_with("CommandWrap")
        }
        TokenTree::Group(g) => mentions_command(g.stream()),
        _ => false,
    })
}

/// What one file holds: OS operations (count and lines), plus the names it defines for the others.
struct Report {
    count: usize,
    lines: BTreeSet<usize>,
    shared: Shared,
}

fn scan(source: &str, shared: &Shared) -> Result<Report> {
    scan_file(source, shared, false)
}

/// [`scan`] of a file that is (`crate_root`) or is not a Cargo binary root.
fn scan_file(source: &str, shared: &Shared, crate_root: bool) -> Result<Report> {
    let stream = TokenStream::from_str(source).map_err(|e| e.to_string())?;
    let trees = flat(stream);
    let mut scan = Scan { shared: shared.clone(), collecting: true, crate_root, ..Scan::default() };
    scan.walk(&trees);
    scan.collecting = false;
    scan.walk(&trees);
    Ok(Report { count: scan.count, lines: scan.lines, shared: scan.shared })
}

/// The shape of one platform file (see the module docs); the OS itself is allowed there.
fn scan_platform(source: &str, root: bool, shared: &Shared) -> Result<Report> {
    let stream = TokenStream::from_str(source).map_err(|e| e.to_string())?;
    let mut scan = Scan { shared: shared.clone(), ..Scan::default() };
    if root {
        scan.walk_root(&flat(stream));
    }
    let file = syn::parse_file(source).map_err(|e| e.to_string())?;
    let mut shape = TraitShape { aliases: &shared.cfg_aliases, gates: 0, lines: Vec::new() };
    shape.gates = usize::from(shape.gated(&file.attrs));
    syn::visit::Visit::visit_file(&mut shape, &file);
    scan.count += shape.lines.len();
    scan.lines.extend(shape.lines);
    Ok(Report { count: scan.count, lines: scan.lines, shared: scan.shared })
}

/// Host-kind conditions of one file, wherever they stand: attributes (inside macros such as `cfg_if!` too), `cfg!`
/// and `cfg_aliases!`, a host kind handed to a macro that puts its argument in a `cfg`, and what a build script prints
/// to define one.
#[derive(Default)]
struct Hosts {
    aliases: HashSet<String>,
    /// The `macro_rules!` that put an argument in a `cfg` predicate: `gate!(mobile)` is `#[cfg(mobile)]`.
    gates: HashMap<String, Reach>,
    /// The file is a build script (or a module of one), where `println!` is how a `cfg` is defined.
    build_script: bool,
    lines: BTreeSet<usize>,
    count: usize,
}

impl Hosts {
    fn found(&mut self, tree: &TokenTree) {
        self.count += 1;
        self.lines.insert(tree.span().start().line);
    }

    fn walk(&mut self, trees: &[TokenTree]) {
        for (at, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(g) => self.walk(&flat(g.stream())),
                TokenTree::Punct(p) if p.as_char() == '#' => {
                    let body = at + usize::from(punct(trees.get(at + 1), '!')) + 1;
                    if let Some(inner) = group(trees.get(body), Delimiter::Bracket) {
                        let inner = flat(inner);
                        let name = inner.first().and_then(ident).unwrap_or_default();
                        let args = group(inner.get(1), Delimiter::Parenthesis).unwrap_or_default();
                        if host_attr(&name, &args, &self.aliases) {
                            self.found(tree);
                        }
                    }
                }
                TokenTree::Ident(id) if punct(trees.get(at + 1), '!') => {
                    let name = id.to_string();
                    if name == "macro_rules" {
                        let defined = trees.get(at + 2).and_then(ident);
                        if let (Some(defined), Some(body)) = (defined, any_group(trees.get(at + 3))) {
                            if let Some(reach) = gate_reach(&flat(body)) {
                                self.gates.entry(defined).and_modify(|known| known.merge(&reach)).or_insert(reach);
                            }
                        }
                        continue;
                    }
                    let Some(inner) = any_group(trees.get(at + 2)) else {
                        continue;
                    };
                    match name.as_str() {
                        "cfg" if names_host(&inner, &self.aliases) => self.found(tree),
                        "cfg_aliases" => self.define(&flat(inner)),
                        "println" | "print"
                            if self.build_script && first_string(inner.clone()).is_some_and(defines_host_cfg) =>
                        {
                            self.found(tree);
                        }
                        gate if self.gates.get(gate).is_some_and(|reach| reach.names_host(&inner, &self.aliases)) => {
                            self.found(tree);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// `cfg_aliases! { name: { condition }, … }`: an entry named like a host kind or built on one counts, and becomes one.
    fn define(&mut self, body: &[TokenTree]) {
        for entry in body.split(|tree| punct(Some(tree), ',')) {
            let Some(alias) = entry.first().and_then(ident) else {
                continue;
            };
            let condition: TokenStream = entry[1..].iter().cloned().collect();
            if HOST_KINDS.contains(&alias.as_str()) || names_host(&condition, &self.aliases) {
                self.found(&entry[0]);
                self.aliases.insert(alias);
            }
        }
    }
}

/// Whether a string is what a build script prints to define a host-kind condition: `cargo:rustc-cfg=mobile`, or
/// `cargo::rustc-check-cfg=cfg(mobile)` which declares one.
fn defines_host_cfg(value: String) -> bool {
    let rest = value.strip_prefix("cargo::").or_else(|| value.strip_prefix("cargo:")).unwrap_or_default();
    if let Some(name) = rest.strip_prefix("rustc-cfg=") {
        return HOST_KINDS.contains(&name.split('=').next().unwrap_or_default());
    }
    rest.strip_prefix("rustc-check-cfg=")
        .is_some_and(|check| TokenStream::from_str(check).is_ok_and(|check| names_host(&check, &HashSet::new())))
}

/// Which arguments of a call to a gate macro reach its `cfg`.
#[derive(Clone, Debug, PartialEq)]
enum Reach {
    /// The arguments at these positions among the call's comma-separated ones: for `($condition:meta, $name:ident)`
    /// the first, so in `named!(test, desktop)` `desktop` is only a name.
    Positions(BTreeSet<usize>),
    /// Every argument: the macro's matcher is not a plain list of fragments (a repetition, a separator other than the
    /// comma, a delimiter of its own), so the call's commas do not tell the positions.
    Any,
}

impl Reach {
    fn merge(&mut self, other: &Reach) {
        match (&mut *self, other) {
            (Reach::Positions(mine), Reach::Positions(theirs)) => mine.extend(theirs),
            _ => *self = Reach::Any,
        }
    }

    /// Whether the call's `arguments` give a host kind to an argument that reaches the `cfg`.
    fn names_host(&self, arguments: &TokenStream, aliases: &HashSet<String>) -> bool {
        match self {
            Reach::Any => names_host(arguments, aliases),
            Reach::Positions(positions) => commas(&flat(arguments.clone()))
                .into_iter()
                .enumerate()
                .any(|(at, argument)| positions.contains(&at) && names_host(&argument.into_iter().collect(), aliases)),
        }
    }
}

/// `trees` cut at their top-level commas.
fn commas(trees: &[TokenTree]) -> Vec<Vec<TokenTree>> {
    trees.split(|tree| punct(Some(tree), ',')).map(<[TokenTree]>::to_vec).collect()
}

/// The metavariables the tokens of a `macro_rules!` transcriber put in a `cfg` predicate: `#[cfg($c)]`,
/// `cfg_attr($c, ..)` or `cfg!($c)`. Such a macro passes whatever it is given for them to `cfg`.
fn cfg_variables(trees: &[TokenTree]) -> HashSet<String> {
    fn variables(trees: &[TokenTree], found: &mut HashSet<String>) {
        for (at, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(g) => variables(&flat(g.stream()), found),
                TokenTree::Punct(p) if p.as_char() == '$' => found.extend(trees.get(at + 1).and_then(ident)),
                _ => {}
            }
        }
    }
    let mut found = HashSet::new();
    for (at, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Group(g) => found.extend(cfg_variables(&flat(g.stream()))),
            TokenTree::Ident(id) if id == "cfg" || id == "cfg_attr" => {
                let call = group(trees.get(at + 1), Delimiter::Parenthesis);
                let bang = if punct(trees.get(at + 1), '!') { any_group(trees.get(at + 2)) } else { None };
                if let Some(args) = call.or(bang) {
                    variables(&flat(args), &mut found);
                }
            }
            _ => {}
        }
    }
    found
}

/// What a `macro_rules!` body does with its arguments: `None` when no arm puts one in a `cfg`, else which of them it
/// is. Each arm is `(matcher) => { transcriber }`; a body that is not arms is read whole, as taking any argument.
fn gate_reach(body: &[TokenTree]) -> Option<Reach> {
    let (mut reach, mut at) = (None::<Reach>, 0);
    while at < body.len() {
        let arm = (
            any_group(body.get(at)),
            punct(body.get(at + 1), '=') && punct(body.get(at + 2), '>'),
            any_group(body.get(at + 3)),
        );
        let (Some(matcher), true, Some(transcriber)) = arm else {
            return (!cfg_variables(body).is_empty()).then_some(Reach::Any);
        };
        at += 4 + usize::from(punct(body.get(at + 4), ';'));
        let used = cfg_variables(&flat(transcriber));
        if used.is_empty() {
            continue;
        }
        let matcher = flat(matcher);
        // A plain list is fragments (`$name:kind`) and commas, and nothing else at its top level.
        let plain = matcher.iter().all(|tree| match tree {
            TokenTree::Punct(p) => matches!(p.as_char(), ',' | '$' | ':'),
            TokenTree::Group(_) => false,
            _ => true,
        });
        let arm = if plain {
            let reaching = |segment: &Vec<TokenTree>| {
                segment.iter().enumerate().any(|(at, tree)| {
                    punct(Some(tree), '$')
                        && segment.get(at + 1).and_then(ident).is_some_and(|name| used.contains(&name))
                })
            };
            let segments = commas(&matcher);
            let positions = segments.iter().enumerate().filter(|(_, segment)| reaching(segment));
            Reach::Positions(positions.map(|(at, _)| at).collect())
        } else {
            Reach::Any
        };
        match &mut reach {
            Some(known) => known.merge(&arm),
            None => reach = Some(arm),
        }
    }
    reach
}

/// The host-kind conditions of one Rust file, wherever it is (`platform` included), and what it defines for the
/// others: aliases and macros that gate. `build_script`: a build script, or a module of one.
fn scan_host(source: &str, shared: &Shared, build_script: bool) -> Result<Report> {
    let trees = flat(TokenStream::from_str(source).map_err(|e| e.to_string())?);
    let mut hosts = Hosts {
        aliases: shared.host_aliases.clone(),
        gates: shared.gate_macros.clone(),
        build_script,
        ..Hosts::default()
    };
    // Twice: an alias or a macro counts where it is used even above the entry that defines it.
    hosts.walk(&trees);
    (hosts.count, hosts.lines) = (0, BTreeSet::new());
    hosts.walk(&trees);
    let shared = Shared { host_aliases: hosts.aliases, gate_macros: hosts.gates, ..shared.clone() };
    Ok(Report { count: hosts.count, lines: hosts.lines, shared })
}

/// The host-kind conditions of a Cargo manifest, which has no tokens to walk: a feature named `desktop` or `mobile`,
/// a `[target.'cfg(..)']` table whose predicate names one, and a `check-cfg` lint that declares one.
fn scan_manifest(text: &str, shared: &Shared) -> Result<Report> {
    let manifest: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
    let names = |predicate: &str| {
        TokenStream::from_str(predicate).is_ok_and(|predicate| names_host(&predicate, &shared.host_aliases))
    };
    let mut found: Vec<String> = Vec::new();
    let keys = |table: Option<&toml::Value>| -> Vec<String> {
        table.and_then(toml::Value::as_table).map(|table| table.keys().cloned().collect()).unwrap_or_default()
    };
    found.extend(keys(manifest.get("features")).into_iter().filter(|name| HOST_KINDS.contains(&name.as_str())));
    found.extend(keys(manifest.get("target")).into_iter().filter(|key| key.starts_with("cfg(") && names(key)));
    // `[lints.rust]`, and `[workspace.lints.rust]` for the members that inherit it.
    let lints = [manifest.get("lints"), manifest.get("workspace").and_then(|workspace| workspace.get("lints"))];
    for lints in lints.into_iter().flatten() {
        let declared =
            lints.get("rust").and_then(|rust| rust.get("unexpected_cfgs")).and_then(|lint| lint.get("check-cfg"));
        let declared = declared.and_then(toml::Value::as_array).into_iter().flatten().filter_map(toml::Value::as_str);
        found.extend(declared.filter(|declared| names(declared)).map(str::to_owned));
    }
    // A table has no spans here: the line is where the text first holds the name.
    let lines =
        found.iter().map(|name| text.lines().position(|line| line.contains(name.as_str())).map_or(1, |at| at + 1));
    Ok(Report { count: found.len(), lines: lines.collect(), shared: shared.clone() })
}

/// `platform/mod.rs`: the module root, which only declares and picks.
fn platform_root(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "mod.rs")
        && path.parent().and_then(Path::file_name).is_some_and(|dir| dir == "platform")
}

/// Platform code (the `platform` module, `platform.rs`) is where the OS belongs.
fn exempt(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == "platform")
        || path.file_name().is_some_and(|name| name == "platform.rs")
}

pub fn check() -> Result<()> {
    inspect(&repo())
}

/// The boundary check over the Rust files of the Git repository at `root`.
fn inspect(root: &Path) -> Result<()> {
    // Git's inventory includes every crate and build script, without target/, vendored caches or worktrees.
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.rs", "*Cargo.toml"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("git ls-files failed".into());
    }
    let (mut files, mut manifests) = (Vec::new(), Vec::new());
    for path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(path).map_err(|e| e.to_string())?.to_string();
        let manifest = Path::new(&path).file_name().is_some_and(|name| name == "Cargo.toml");
        if !manifest && !path.ends_with(".rs") {
            continue;
        }
        let source = std::fs::read_to_string(root.join(&path)).map_err(|e| format!("{path}: {e}"))?;
        (if manifest { &mut manifests } else { &mut files }).push((path, source));
    }
    // OS aliases, host-kind aliases and macros, and command wrappers are defined in one file and used in others, and
    // may build on each other.
    let mut shared = Shared::default();
    loop {
        let before = shared.clone();
        // Platform files too: an alias defined there (a build module) gates code everywhere.
        for (path, source) in &files {
            let found = scan(source, &shared).map_err(|e| format!("{path}: {e}"))?.shared;
            shared.cfg_aliases.extend(found.cfg_aliases);
            shared.command_types.extend(found.command_types);
            let hosts = scan_host(source, &shared, build_script(path)).map_err(|e| format!("{path}: {e}"))?.shared;
            shared.host_aliases.extend(hosts.host_aliases);
            for (name, reach) in hosts.gate_macros {
                shared.gate_macros.entry(name).and_modify(|known| known.merge(&reach)).or_insert(reach);
            }
        }
        if shared == before {
            break;
        }
    }
    // What each file holds: its OS occurrences, and its host-kind conditions (the host kinds are everyone's: `platform`
    // files and the Cargo manifests are read for them too).
    let mut reports = Vec::new();
    for (path, source) in &files {
        let os = if exempt(Path::new(path)) {
            scan_platform(source, platform_root(Path::new(path)), &shared)
        } else {
            scan_file(source, &shared, binary_root(root, path))
        };
        reports.push((path, os.map_err(|e| format!("{path}: {e}"))?, "OS occurrences", "move OS code into platform"));
        let hosts = scan_host(source, &shared, build_script(path)).map_err(|e| format!("{path}: {e}"))?;
        reports.push((path, hosts, "host-kind conditions", HOST_FIX));
    }
    for (path, source) in &manifests {
        let hosts = scan_manifest(source, &shared).map_err(|e| format!("{path}: {e}"))?;
        reports.push((path, hosts, "host-kind conditions", HOST_FIX));
    }
    let (mut errors, mut total, mut reported) = (Vec::new(), 0, BTreeSet::new());
    for (path, report, what, fix) in reports.into_iter().filter(|(_, report, ..)| report.count > 0) {
        total += report.count;
        reported.insert(path);
        let lines: Vec<String> = report.lines.iter().map(usize::to_string).collect();
        errors.push(format!("{path}: {} {what} (lines {}); {fix}", report.count, lines.join(", ")));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{}\nPlatform boundary: {total} violations in {} files", errors.join("\n"), reported.len()))
    }
}

const HOST_FIX: &str = "host kind is a crate boundary: put the code in the crate that owns it";

/// A build script, or a module of one: `build.rs`, and files in a `build/` directory.
fn build_script(path: &str) -> bool {
    let path = Path::new(path);
    path.file_name().is_some_and(|name| name == "build.rs")
        || path.parent().is_some_and(|dir| dir.components().any(|part| part.as_os_str() == "build"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(source: &str) -> usize {
        scan(source, &Shared::default()).unwrap().count
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
    fn allows_only_the_console_subsystem_attribute_of_a_crate_root() {
        let attribute = "#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")] fn main() {}";
        assert_eq!(scan_file(attribute, &Shared::default(), true).unwrap().count, 0);
        assert_eq!(n(attribute), 1, "not in a module");
        for other in [
            "#![windows_subsystem = \"windows\"] fn main() {}",
            "#![cfg_attr(windows, windows_subsystem = \"windows\")] fn main() {}",
            "#![cfg_attr(test, windows_subsystem = \"windows\")] fn main() {}",
            "#![cfg_attr(not(debug_assertions), windows_subsystem = \"console\")] fn main() {}",
            "#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\", cfg(unix))] fn main() {}",
            "#[cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")] fn main() {}",
            "#![cfg(unix)] fn main() {}",
            "mod child { #![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")] }",
            "fn f() { #![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")] }",
        ] {
            assert_eq!(scan_file(other, &Shared::default(), true).unwrap().count, 1, "{other}");
        }
    }

    #[test]
    fn only_cargo_binary_roots_may_carry_the_subsystem_attribute() {
        let root = std::env::temp_dir().join(format!("platform-guard-roots-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("app/Cargo.toml"), "").unwrap();
        for path in ["app/src/main.rs", "app/src/bin/tool.rs", "app/src/bin/tool/main.rs"] {
            assert!(binary_root(&root, path), "{path}");
        }
        for path in [
            "app/src/helper/main.rs",
            "app/src/lib.rs",
            "app/src/bin/tool/other.rs",
            "other/src/main.rs",
            "src/main.rs",
        ] {
            assert!(!binary_root(&root, path), "{path}");
        }
        std::fs::remove_dir_all(&root).unwrap();
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
        let aliases = scan(build, &Shared::default()).unwrap();
        assert_eq!(aliases.count, 2);
        assert_eq!(aliases.shared.cfg_aliases, HashSet::from(["apple".to_string(), "mac".to_string()]));
        let user = "#[cfg(mac)] fn a() {} #[cfg(other)] fn b() {} fn c() { if cfg!(apple) {} }";
        assert_eq!(scan(user, &aliases.shared).unwrap().count, 2);
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
        assert!(scan("fn f() { \"unterminated }", &Shared::default()).is_err());
    }

    fn with_wrapper(source: &str) -> usize {
        let shared = Shared { command_types: HashSet::from(["Cmd".to_string()]), ..Shared::default() };
        scan(source, &shared).unwrap().count
    }

    #[test]
    fn resolves_portable_parent_imports() {
        assert_eq!(n("use std::env; fn f() { let _ = env::consts::OS; }"), 1);
        assert_eq!(n("use std::env::consts; fn f() { let _ = consts::FAMILY; }"), 1);
        assert_eq!(n("use std as s; fn f() { let _ = s::os::unix::fs::chown; }"), 1);
        assert_eq!(n("use std::{env as e, os}; fn f() { let _ = e::consts::OS; os::unix::raw::pid_t::MAX; }"), 3);
        // Portable uses of the same parents stay allowed.
        assert_eq!(n("use std::env; fn f() { let _ = env::var(\"PORTABLE_SETTING\"); let _ = env::consts::ARCH; }"), 0);
    }

    #[test]
    fn sees_cfg_in_every_macro_delimiter() {
        assert_eq!(
            n("fn f() { let _ = cfg! { unix }; let _ = cfg![windows]; let _ = cfg!(target_os = \"linux\"); }"),
            3
        );
        let build = "fn main() { cfg_aliases::cfg_aliases! { mac: { target_os = \"macos\" } } }";
        let aliases = scan(build, &Shared::default()).unwrap();
        assert_eq!(scan("fn f() { let _ = cfg![mac]; let _ = cfg! { mac }; }", &aliases.shared).unwrap().count, 2);
    }

    #[test]
    fn sees_commands_through_generics_raw_names_and_wrappers() {
        assert_eq!(n("fn f() { std::process::Command::new::<&str>(\"sh\"); }"), 1);
        assert_eq!(n("use std::process::Command as r#Run; fn f() { r#Run::new(\"sh\"); }"), 1);
        assert_eq!(n("use std::process::Command as Run; fn f() { Run::new(\"cmd.exe\"); }"), 1);
        assert_eq!(with_wrapper("use crate::util::Cmd; fn f() { Cmd::new(\"powershell.exe\"); }"), 1);
        assert_eq!(with_wrapper("use crate::util::Cmd; fn f() { Cmd::new(\"git\").arg(\"status\"); }"), 0);
        let defined = "pub struct Cmd { command: Command, shown: Vec<String> } type Run = tokio::process::Command;";
        let learned = scan(defined, &Shared::default()).unwrap().shared.command_types;
        assert_eq!(learned, HashSet::from(["Cmd".to_string(), "Run".to_string()]));
        assert_eq!(n("struct Plain { shown: Vec<String> } fn f() { Plain::new(\"sh\"); }"), 0);
    }

    #[test]
    fn catches_local_sockets() {
        assert_eq!(n("fn f() { let _ = tokio::net::UnixStream::connect(\"socket\"); }"), 1);
        assert_eq!(n("use tokio::net::{TcpStream, UnixListener}; fn f() {}"), 1);
        assert_eq!(n("fn f() { let _ = tokio::net::windows::named_pipe::ClientOptions::new(); }"), 1);
        assert_eq!(n("fn f() { let _ = tokio::net::TcpStream::connect(\"127.0.0.1:1\"); }"), 0);
    }

    #[test]
    fn catches_os_paths_in_operations_only() {
        assert_eq!(n("fn f() { let _ = std::fs::read_to_string(\"/proc/self/status\"); }"), 1);
        assert_eq!(n("fn f() { let _ = std::path::PathBuf::from(\"C:\\\\Windows\\\\x\"); }"), 1);
        assert_eq!(n("fn f(c: &mut C) { c.args([\"-o\", \"/dev/null\"]).arg(r\"\\\\.\\pipe\\x\"); }"), 2);
        assert_eq!(n("fn f() { std::fs::read(Path::new(\"/etc/hosts\")); }"), 1);
        assert_eq!(n("fn f() { let _ = std::path::PathBuf::from(\"/tmp\"); }"), 1);
        // Documentation and data are not operations.
        assert_eq!(n("/// Reads /proc/self/status.\nfn f() { let _ = \"/proc/self/status\"; let _ = String::from(\"/dev/null\"); }"), 0);
        assert_eq!(n("fn f() { std::fs::read_to_string(\"notes/readme.txt\"); }"), 0);
    }

    #[test]
    fn catches_native_framework_links() {
        assert_eq!(n("#[link(name = \"AppKit\", kind = \"framework\")] extern \"C\" {}"), 1);
        assert_eq!(n("#[cfg_attr(feature = \"x\", link(name = \"AppKit\", kind = \"framework\"))] extern \"C\" {}"), 1);
        assert_eq!(n("#[link(name = \"m\")] extern \"C\" {}"), 0);
    }

    #[test]
    fn counts_cargo_variables_only_when_read() {
        assert_eq!(n("fn main() { let _ = std::env::var_os(\"CARGO_CFG_TARGET_FAMILY\"); }"), 1);
        assert_eq!(n("use std::env::var as get; fn main() { let _ = get(\"CARGO_CFG_UNIX\"); }"), 1);
        assert_eq!(n("use std::env; fn main() { let _ = env::var(\"CARGO_CFG_UNIX\"); }"), 1);
        assert_eq!(
            n("const X: &str = env!(\"CARGO_CFG_TARGET_OS\"); fn f() { option_env![\"CARGO_CFG_WINDOWS\"]; }"),
            2
        );
        assert_eq!(n("const EXAMPLE: &str = \"CARGO_CFG_TARGET_OS\"; #[test] fn f() { assert_eq!(name(), \"CARGO_CFG_TARGET_OS\"); }"), 0);
    }

    fn root(source: &str) -> usize {
        scan_platform(source, true, &Shared::default()).unwrap().count
    }

    fn other(source: &str) -> usize {
        scan_platform(source, false, &Shared::default()).unwrap().count
    }

    #[test]
    fn platform_root_rejects_os_cfg_and_unused_allows() {
        assert_eq!(root("#[cfg(target_os = \"macos\")] mod tray; #[cfg(windows)] mod tray;"), 2);
        assert_eq!(root("#[cfg_attr(unix, derive(Debug))] pub struct A;"), 1);
        assert_eq!(root("#![allow(dead_code)] #[allow(unused_imports)] use a::b; #[expect(unused)] fn f() {}"), 3);
        assert_eq!(root("#[cfg_attr(test, allow(dead_code))] fn f() {}"), 1);
        assert_eq!(root("#[cfg_attr(not(debug_assertions), cfg_attr(unix, allow(dead_code)))] fn f() {}"), 1);
    }

    #[test]
    fn platform_root_accepts_portable_declarations() {
        assert_eq!(
            root("mod tray; pub use tray::Tray; #[cfg(test)] mod tests {} #[allow(clippy::needless_return)] fn f() {}"),
            0
        );
        assert_eq!(
            root("#[cfg(feature = \"windows\")] mod a; #[derive(Debug)] struct B; #[deny(dead_code)] fn f() {}"),
            0
        );
        assert_eq!(root("/// Uses #[cfg(unix)] and allow(dead_code).\nfn f() { let _ = \"#[cfg(unix)]\"; }"), 0);
    }

    #[test]
    fn root_rules_apply_to_mod_rs_only() {
        assert_eq!(other("#[cfg(target_os = \"macos\")] mod mac; #[allow(dead_code)] fn f() {}"), 0);
        assert!(platform_root(Path::new("desktop/src/platform/mod.rs")));
        assert!(!platform_root(Path::new("desktop/src/platform/tray.rs")));
        assert!(!platform_root(Path::new("desktop/src/platform/tray/mod.rs")));
        assert!(!platform_root(Path::new("desktop/src/other/mod.rs")));
    }

    #[test]
    fn traits_reject_os_gated_items() {
        let methods =
            "pub trait Data { fn all(&self); #[cfg(any(target_os = \"macos\", test))] fn responsible(&self) -> u32; }";
        assert_eq!(other(methods), 1);
        assert_eq!(
            other(
                "trait T { #[cfg(target_os = \"macos\")] const X: u8; #[cfg(unix)] type A; #[cfg(windows)] fn f(); }"
            ),
            3
        );
        assert_eq!(other("trait T { #[cfg_attr(unix, inline)] fn f(&self) {} }"), 1);
        assert_eq!(other("mod m { pub trait T { #[cfg(not(target_os = \"linux\"))] fn f(&self); } }"), 1);
        // The same rule holds in a root.
        assert_eq!(root("trait T { #[cfg(unix)] fn f(&self); }"), 2);
        let aliases = Shared { cfg_aliases: HashSet::from(["mac".to_string()]), ..Shared::default() };
        assert_eq!(scan_platform("trait T { #[cfg(mac)] fn f(&self); }", false, &aliases).unwrap().count, 1);
    }

    #[test]
    fn traits_accept_portable_items() {
        assert_eq!(other("trait T { fn f(&self); #[cfg(test)] fn g(&self); #[cfg(feature = \"x\")] fn h(&self); }"), 0);
        assert_eq!(other("trait T { #[doc = \"unix\"] fn f(&self) { #[cfg(unix)] let _ = 1; } }"), 0);
        // OS gates on the implementing side are the OS file's own business.
        assert_eq!(other("impl T for A { #[cfg(unix)] fn f(&self) {} } fn g<T: Trait>() {}"), 0);
        assert_eq!(other("fn f() -> impl Trait { 1 } struct S; #[cfg(unix)] fn g() {}"), 0);
    }

    #[test]
    fn traits_reject_gates_on_the_trait_its_modules_and_its_file() {
        assert_eq!(other("#[cfg(unix)] pub trait T { fn f(); }"), 1);
        assert_eq!(other("#[cfg(unix)] mod m { pub trait T { fn f(); } }"), 1);
        assert_eq!(other("mod a { #[cfg(windows)] mod b { trait T {} trait U {} } trait V {} }"), 2);
        assert_eq!(other("#![cfg(unix)]\npub trait T { fn f(); }"), 1);
        // Selecting an OS implementation or gating an implementation stays allowed.
        assert_eq!(other("#[cfg(unix)] mod unix; #[cfg(unix)] impl T for A { fn f() {} } trait V { fn f(); }"), 0);
        assert_eq!(other("#[cfg(unix)] mod m { pub struct S; impl T for S {} }"), 0);
    }

    #[test]
    fn traits_are_found_behind_braces_in_their_header() {
        assert_eq!(other("pub trait T<const N: usize = { 1 }> { #[cfg(unix)] fn f(); }"), 1);
        assert_eq!(other("trait T: P<{ 1 }> { #[cfg(unix)] fn f(); }"), 1);
        assert_eq!(other("trait T<const N: usize = { 1 }> { fn f(); }"), 0);
    }

    #[test]
    fn os_words_in_a_portable_payload_are_not_conditions() {
        assert_eq!(other("trait T { #[cfg_attr(test, doc = stringify!(unix))] fn f(); }"), 0);
        assert_eq!(root("#[cfg_attr(test, doc = stringify!(unix))] mod a;"), 0);
        assert_eq!(other("trait T { #[cfg_attr(test, cfg(windows))] fn f(); }"), 1);
        assert_eq!(other("trait T { #[cfg_attr(test, cfg_attr(unix, inline))] fn f(); }"), 1);
        assert_eq!(n("#[cfg_attr(test, doc = stringify!(unix))] fn f() {}"), 0);
        assert_eq!(n("#[cfg_attr(test, cfg(unix))] fn f() {}"), 1);
    }

    #[test]
    fn aliases_defined_in_platform_files_reach_every_check() {
        let build = "pub fn configure() { cfg_aliases::cfg_aliases! { apple: { target_os = \"macos\" } } }";
        let shared = scan(build, &Shared::default()).unwrap().shared;
        assert_eq!(scan_platform("#[cfg(apple)] mod concern;", true, &shared).unwrap().count, 1);
        assert_eq!(scan_platform("trait T { #[cfg(apple)] fn f(); }", false, &shared).unwrap().count, 1);
    }

    #[test]
    fn a_platform_build_module_defining_the_alias_does_not_hide_the_shape_rules() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            ("build.rs", "#[path = \"src/platform/build.rs\"] mod build; fn main() { build::configure(); }"),
            (
                "src/platform/build.rs",
                "pub fn configure() { cfg_aliases::cfg_aliases! { apple: { target_os = \"macos\" } } }",
            ),
            ("src/platform/mod.rs", "#[cfg(apple)] mod concern;"),
            ("src/platform/concern.rs", "pub trait T { #[cfg(apple)] fn f(); }"),
        ];
        for (path, source) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).current_dir(dir.path()).status().unwrap().success());
        };
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        let message = inspect(dir.path()).unwrap_err();
        assert!(message.contains("src/platform/mod.rs") && message.contains("src/platform/concern.rs"), "{message}");
    }

    #[test]
    fn a_lint_allowance_gated_by_os_does_not_gate_the_traits_it_encloses() {
        assert_eq!(other("#![cfg_attr(windows, allow(dead_code))]\npub trait T { fn f(); }"), 0);
        assert_eq!(other("#[cfg_attr(windows, allow(dead_code))] mod m { pub trait T { fn f(); } }"), 0);
        assert_eq!(other("#[cfg_attr(test, cfg(windows))] pub trait T { fn f(); }"), 1);
    }

    #[test]
    fn an_os_cfg_attr_applying_a_portable_cfg_gates_what_it_encloses() {
        assert_eq!(other("#[cfg_attr(windows, cfg(feature = \"x\"))] pub trait T { fn f(); }"), 1);
        assert_eq!(other("#[cfg_attr(unix, cfg_attr(test, cfg(feature = \"x\")))] mod m { trait T {} }"), 1);
        assert_eq!(other("#![cfg_attr(windows, cfg(feature = \"x\"))]\ntrait T { fn f(); }"), 1);
        assert_eq!(other("#[cfg_attr(test, cfg(feature = \"x\"))] mod m { trait T { fn f(); } }"), 0);
        assert_eq!(other("#[cfg_attr(windows, allow(dead_code))] mod m { trait T { fn f(); } }"), 0);
    }

    fn host(source: &str) -> usize {
        scan_host(source, &Shared::default(), false).unwrap().count
    }

    fn host_in_build_script(source: &str) -> usize {
        scan_host(source, &Shared::default(), true).unwrap().count
    }

    fn manifest(text: &str) -> usize {
        scan_manifest(text, &Shared::default()).unwrap().count
    }

    #[test]
    fn host_kind_conditions_are_found_in_every_form() {
        for source in [
            "#[cfg(desktop)] fn a() {}",
            "#[cfg(mobile)] fn a() {}",
            "#![cfg(desktop)]",
            "#[cfg(not(mobile))] fn a() {}",
            "#[cfg(all(feature = \"x\", any(desktop, mobile)))] struct S;",
            "#[cfg_attr(desktop, derive(Debug))] struct S;",
            "#[cfg_attr(test, cfg(mobile))] fn a() {}",
            "#[cfg_attr(test, cfg_attr(not(desktop), inline))] fn a() {}",
            "cfg_if::cfg_if! { if #[cfg(desktop)] { fn a() {} } else { fn a() {} } }",
            "#[cfg(feature = \"desktop\")] fn a() {}",
            "#[cfg(not(feature = \"mobile\"))] fn a() {}",
            "mod m { #![cfg(desktop)] }",
        ] {
            assert_eq!(host(source), 1, "{source}");
        }
        assert_eq!(host("fn f() { if cfg!(desktop) {} let _ = cfg![mobile]; let _ = cfg! { not(desktop) }; }"), 3);
    }

    #[test]
    fn host_kind_words_that_are_not_conditions_are_left_alone() {
        for source in [
            // Names, data and documentation.
            "fn desktop() {} struct Mobile; let mobile = 1;",
            "/// Not for #[cfg(desktop)] or cfg!(mobile).\nfn f() { let _ = \"#[cfg(desktop)] cfg!(mobile)\"; }",
            "// #[cfg(mobile)]\nfn f() {}",
            // A variable named like a kind, handed to a macro that does not gate.
            "fn f(mobile: bool) { assert!(mobile); let _ = vec![mobile]; println!(\"{}\", mobile); }",
            // Other conditions, the OS ones included: the other scan's.
            "#[cfg(feature = \"tray\")] #[cfg(test)] #[cfg(target_os = \"ios\")] #[cfg(any(unix, windows))] fn a() {}",
            "#[cfg(feature = \"x\")] #[cfg_attr(test, doc = stringify!(desktop))] fn a() {}",
            "#[cfg(target_os = \"desktop\")] fn a() {}",
        ] {
            assert_eq!(host(source), 0, "{source}");
        }
    }

    #[test]
    fn host_kind_aliases_count_where_they_are_defined_and_where_they_are_used() {
        let build = "fn main() { cfg_aliases::cfg_aliases! { handheld: { mobile }, tablet: { all(handheld, x) }, \
                     plain: { feature = \"x\" }, desktop: { target_os = \"macos\" }, os: { windows } } }";
        let defined = scan_host(build, &Shared::default(), true).unwrap();
        assert_eq!(defined.count, 3, "handheld, tablet, desktop");
        assert_eq!(
            defined.shared.host_aliases,
            HashSet::from(["handheld".to_string(), "tablet".to_string(), "desktop".to_string()])
        );
        let user = "#[cfg(handheld)] fn a() {} #[cfg(plain)] #[cfg(os)] fn b() {} fn c() { let _ = cfg!(tablet); }";
        assert_eq!(scan_host(user, &defined.shared, false).unwrap().count, 2);
        // An alias above its definition in the same file counts too.
        assert_eq!(host(&format!("#[cfg(handheld)] fn a() {{}} {build}")), 4);
    }

    #[test]
    fn a_macro_that_puts_its_argument_in_a_cfg_gates_what_it_is_given() {
        let gate = "macro_rules! gate { ($c:meta) => { #[cfg($c)] fn f() {} }; }";
        for source in [
            format!("{gate} gate!(mobile);"),
            format!("{gate} gate!(any(unix, desktop));"),
            format!("{gate} gate! {{ feature = \"mobile\" }}"),
            // Used above its definition.
            format!("gate!(desktop); {gate}"),
            "macro_rules! test { ($c:meta) => { if cfg!($c) { 1 } else { 0 } }; } test!(mobile);".to_owned(),
            "macro_rules! attr { ($c:meta) => { #[cfg_attr($c, inline)] fn f() {} }; } attr!(desktop);".to_owned(),
        ] {
            assert_eq!(host(&source), 1, "{source}");
        }
        for source in [
            // Not given a kind, or not putting what it is given in a `cfg`.
            format!("{gate} gate!(test); gate!(feature = \"x\"); gate!(unix);"),
            "macro_rules! twice { ($e:expr) => { $e + $e }; } fn f(mobile: u8) -> u8 { twice!(mobile) }".to_owned(),
            "macro_rules! derive_it { ($c:meta) => { #[$c] struct S; }; } derive_it!(mobile);".to_owned(),
            "macro_rules! cfgs { () => { #[cfg(test)] fn f() {} }; } cfgs!(); fn g(desktop: u8) { cfgs!(); let _ = desktop; }"
                .to_owned(),
        ] {
            assert_eq!(host(&source), 0, "{source}");
        }
        // Only the arguments that reach the `cfg` are looked at: `desktop` as the name of a function is a name.
        let named = "macro_rules! named { ($condition:meta, $name:ident) => { #[cfg($condition)] fn $name() {} }; }";
        assert_eq!(host(&format!("{named} named!(mobile, test);")), 1);
        assert_eq!(host(&format!("{named} named!(any(unix, desktop), test);")), 1);
        for source in [
            format!("{named} named!(test, desktop); named!(unix, mobile);"),
            // The same, across arms: the position is the arm's own.
            "macro_rules! arms { ($name:ident) => {}; ($name:ident, $c:meta) => { #[cfg($c)] fn $name() {} }; } \
             arms!(mobile); arms!(mobile, test);"
                .to_owned(),
        ] {
            assert_eq!(host(&source), 0, "{source}");
        }
        // A matcher that is not a plain list of fragments (a repetition, a separator of its own) cannot tell the
        // positions, so every argument is looked at.
        for source in [
            "macro_rules! each { ($($c:meta),*) => { $(#[cfg($c)] fn f() {})* }; } each!(test, mobile);",
            "macro_rules! with { ($name:ident = $c:meta) => { #[cfg($c)] fn $name() {} }; } with!(f = desktop);",
            // Each arm adds the positions that reach its `cfg`.
            "macro_rules! arms { ($c:meta, $name:ident) => { #[cfg($c)] fn $name() {} }; \
             ($name:ident, $c:meta) => { #[cfg($c)] fn $name() {} }; } arms!(f, mobile);",
        ] {
            assert_eq!(host(source), 1, "{source}");
        }
        // Defined in one file, used in another.
        let defined = scan_host(gate, &Shared::default(), false).unwrap();
        assert!(defined.count == 0 && defined.shared.gate_macros.contains_key("gate"));
        assert_eq!(scan_host("gate!(mobile);", &defined.shared, false).unwrap().count, 1);
    }

    #[test]
    fn a_build_script_defines_a_host_kind_by_printing_it_and_a_string_is_not_that() {
        for source in [
            "fn main() { println!(\"cargo:rustc-cfg=mobile\"); }",
            "fn main() { println!(\"cargo::rustc-cfg=desktop\"); }",
            "fn main() { print!(\"cargo::rustc-check-cfg=cfg(mobile)\"); }",
        ] {
            assert_eq!(host_in_build_script(source), 1, "{source}");
            assert_eq!(host(source), 0, "not a build script: {source}");
        }
        for source in [
            "fn main() { println!(\"cargo:rustc-cfg=tray\"); println!(\"cargo:rerun-if-changed=desktop\"); }",
            "fn main() { println!(\"cargo::rustc-check-cfg=cfg(tray)\"); }",
            "fn main() { let expected = \"cargo:rustc-cfg=mobile\"; assert_eq!(line, \"cargo::rustc-cfg=desktop\"); }",
        ] {
            assert_eq!(host_in_build_script(source), 0, "{source}");
        }
        for path in ["build.rs", "desktop/build.rs", "desktop/build/platform.rs", "desktop/build/platform/mac.rs"] {
            assert!(build_script(path), "{path}");
        }
        for path in ["desktop/src/lib.rs", "desktop/core/src/build_info.rs", "xtask/src/platform/build_tools.rs"] {
            assert!(!build_script(path), "{path}");
        }
    }

    #[test]
    fn a_cargo_manifest_names_a_host_kind_in_its_features_target_tables_and_lints() {
        for text in [
            "[features]\nmobile = []\n",
            "[features]\ndefault = []\ndesktop = [\"dep:tauri\"]\n",
            "[target.'cfg(mobile)'.dependencies]\ntauri = \"2\"\n",
            "[target.'cfg(any(unix, desktop))'.dev-dependencies]\nx = \"1\"\n",
            "[target.'cfg(feature = \"mobile\")'.build-dependencies]\nx = \"1\"\n",
            "[lints.rust]\nunexpected_cfgs = { level = \"warn\", check-cfg = ['cfg(mobile)'] }\n",
            "[workspace.lints.rust]\nunexpected_cfgs = { level = \"warn\", check-cfg = ['cfg(desktop)'] }\n",
        ] {
            assert_eq!(manifest(text), 1, "{text}");
        }
        for text in [
            "[features]\nprobe = []\ntray = [\"dep:tray-icon\"]\n",
            "[target.'cfg(target_os = \"ios\")'.dependencies]\nx = \"1\"\n[target.'cfg(windows)'.dependencies]\ny = \"1\"\n",
            "[target.x86_64-unknown-linux-gnu.dependencies]\nx = \"1\"\n",
            // A dependency's own feature is its business.
            "[dependencies]\ntauri = { version = \"2\", features = [\"desktop\", \"mobile\"] }\n",
            "[lints.rust]\nunexpected_cfgs = { level = \"warn\", check-cfg = ['cfg(tokio_unstable)'] }\n",
        ] {
            assert_eq!(manifest(text), 0, "{text}");
        }
        let aliases = Shared { host_aliases: HashSet::from(["handheld".to_string()]), ..Shared::default() };
        assert_eq!(scan_manifest("[target.'cfg(handheld)'.dependencies]\nx = \"1\"\n", &aliases).unwrap().count, 1);
        let lines =
            scan_manifest("[package]\nname = \"a\"\n\n[features]\nmobile = []\n", &Shared::default()).unwrap().lines;
        assert_eq!(lines, BTreeSet::from([5]));
        assert!(scan_manifest("[features", &Shared::default()).is_err());
    }

    #[test]
    fn host_kinds_are_found_inside_platform_and_outside_it() {
        let files = [
            ("app/src/lib.rs", "#[cfg(mobile)] pub fn outside() {}"),
            ("app/src/platform/concern.rs", "#[cfg(desktop)] pub fn inside() {}"),
            ("app/src/platform/concern/ios.rs", "#[cfg(target_os = \"ios\")] pub fn os() {}"),
            ("app/src/platform/mod.rs", "mod concern; #[cfg(feature = \"mobile\")] mod gated;"),
            ("app/build/platform.rs", "pub fn configure() { cfg_aliases::cfg_aliases! { handheld: { mobile } } }"),
            ("app/src/other.rs", "#[cfg(handheld)] pub fn by_alias() {}"),
            // Through a macro defined in another file, in a manifest, and printed by a build script.
            ("app/src/macros.rs", "macro_rules! gate { ($c:meta) => { #[cfg($c)] fn f() {} }; }"),
            ("app/src/gated.rs", "gate!(mobile);"),
            ("app/Cargo.toml", "[package]\nname = \"app\"\n[features]\nmobile = []\n"),
            ("app/build.rs", "fn main() { println!(\"cargo:rustc-cfg=desktop\"); }"),
        ];
        let message = fixture_repo(&files).unwrap_err();
        for path in [
            "src/lib.rs",
            "src/platform/concern.rs",
            "src/platform/mod.rs",
            "build/platform.rs",
            "src/other.rs",
            "src/gated.rs",
            "Cargo.toml",
            "build.rs",
        ] {
            let line = message.lines().find(|line| line.contains(&format!("app/{path}:"))).unwrap_or_default();
            assert!(line.contains("host-kind conditions"), "{path}: {message}");
        }
        assert!(!message.contains("ios.rs") && !message.contains("macros.rs"), "{message}");
        assert!(message.contains("8 violations in 8 files"), "{message}");
        // The rest of the tree passes: the OS in `platform`, a feature of another name, a string that only looks like
        // what a build script prints.
        let clean = [
            ("app/src/lib.rs", "pub fn shared() {}"),
            ("app/src/platform/concern.rs", "pub trait Concern { fn f(&self); }"),
            ("app/src/platform/concern/ios.rs", "#[cfg(target_os = \"ios\")] pub fn os() {}"),
            (
                "app/src/platform/concern/macos.rs",
                "#[cfg(any(target_os = \"macos\", target_os = \"ios\"))] fn apple() {}",
            ),
            ("app/Cargo.toml", "[features]\nprobe = []\n[target.'cfg(target_os = \"ios\")'.dependencies]\nx = \"1\"\n"),
            (
                "app/build.rs",
                "fn main() { let expected = \"cargo:rustc-cfg=mobile\"; println!(\"cargo:rustc-cfg=tray\"); }",
            ),
        ];
        fixture_repo(&clean).unwrap();
    }

    /// `inspect` over a Git repository holding `files`.
    fn fixture_repo(files: &[(&str, &str)]) -> Result<()> {
        let dir = tempfile::tempdir().unwrap();
        for (path, source) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        inspect(dir.path())
    }
}
