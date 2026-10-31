//! The memory measurements (#145): `probe memory` (the probe app through a long session) and `probe voice-memory` (the
//! host's calls on the engine's real models). Both print `memory phase=NAME footprint=N resident=N metal=N|na
//! accelerator=N|na` lines (desktop/core/src/memory.rs); this reads them back, prints a compact table and fails only
//! on growth that is clear.
//!
//! What fails, and why these numbers:
//! - **Release**: once a call ends and the models' idle keep runs out, the footprint must have given back at least half
//!   of what the models added while loaded (when they added 100 MiB or more; below that it is noise). The models are the
//!   biggest thing a call holds; one that is still held after the keep is not freed.
//! - **Flat**: between two steps that repeat the same work (ten calls more, forty changes of a card setting, forty
//!   device lists) the footprint may not grow by more than [`FOOTPRINT_MIB`], nor the GPU's Metal allocation by more
//!   than [`METAL_MIB`]. Allocators keep some pages and caches fill once (the second call is not the first), so the
//!   limits are a margin over that, far below what one leaked model (hundreds of MiB), window or VAD stream adds each
//!   time. The devices poll runs for forty seconds at idle and gets the tighter [`IDLE_MIB`].
//!
//! Every step's numbers are printed whether or not it fails, so a run is a record to compare with the next.

use std::collections::BTreeMap;

use crate::Result;

const MIB: u64 = 1024 * 1024;
/// How much the footprint may grow across a repeated step.
pub const FOOTPRINT_MIB: u64 = 64;
/// How much the GPU's Metal allocation may grow across a repeated step.
pub const METAL_MIB: u64 = 32;
/// How much the footprint may grow in a stretch of idle.
pub const IDLE_MIB: u64 = 16;
/// Under this, the models' load is not told from noise.
const LOADED_MIB: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub phase: String,
    pub footprint: u64,
    pub resident: u64,
    pub metal: Option<u64>,
    pub accelerator: Option<u64>,
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace().find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
}

fn optional(line: &str, key: &str) -> Option<Option<u64>> {
    match field(line, key)? {
        "na" => Some(None),
        n => n.parse().ok().map(Some),
    }
}

/// The rows in `text`, in the order they were printed (a phase printed twice keeps its last values, where it first was).
pub fn parse(text: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for line in text.lines().filter_map(|line| line.find("memory phase=").map(|at| &line[at..])) {
        let row = (|| {
            Some(Row {
                phase: field(line, "phase")?.to_owned(),
                footprint: field(line, "footprint")?.parse().ok()?,
                resident: field(line, "resident")?.parse().ok()?,
                metal: optional(line, "metal")?,
                accelerator: optional(line, "accelerator")?,
            })
        })();
        let Some(row) = row else { continue };
        match rows.iter_mut().find(|old| old.phase == row.phase) {
            Some(old) => *old = row,
            None => rows.push(row),
        }
    }
    rows
}

fn mib(bytes: u64) -> String {
    format!("{:.0}", bytes as f64 / MIB as f64)
}

fn signed(from: u64, to: u64) -> String {
    let delta = to as i64 - from as i64;
    format!("{:+.0}", delta as f64 / MIB as f64)
}

/// The compact table: each step's footprint, resident set, Metal allocation and GPU regions in MiB, and the footprint's
/// change since the first step.
pub fn table(rows: &[Row]) -> String {
    let na = |v: Option<u64>| v.map_or("n/a".to_owned(), mib);
    let first = rows.first().map_or(0, |row| row.footprint);
    let mut out = format!(
        "{:<16} {:>10} {:>10} {:>10} {:>12} {:>10}\n",
        "step", "footprint", "resident", "metal", "accelerator", "vs first"
    );
    for row in rows {
        out += &format!(
            "{:<16} {:>10} {:>10} {:>10} {:>12} {:>10}\n",
            row.phase,
            mib(row.footprint),
            mib(row.resident),
            na(row.metal),
            na(row.accelerator),
            signed(first, row.footprint)
        );
    }
    out + "(MiB)"
}

/// One check over the steps.
#[derive(Debug, Clone, Copy)]
pub enum Rule {
    /// What loaded at `live` is given back by `ended`.
    Release { idle: &'static str, live: &'static str, ended: &'static str },
    /// Nothing grows from `from` to `to` beyond `footprint` MiB (and the Metal allocation beyond [`METAL_MIB`]).
    Flat { from: &'static str, to: &'static str, footprint: u64 },
}

/// The failures of `rules` over `rows`: one sentence each. A rule whose steps are missing fails (the run did not get there).
pub fn check(rows: &[Row], rules: &[Rule]) -> Vec<String> {
    let by: BTreeMap<&str, &Row> = rows.iter().map(|row| (row.phase.as_str(), row)).collect();
    let mut failures = Vec::new();
    for rule in rules {
        let find = |phase: &str| by.get(phase).copied();
        match *rule {
            Rule::Release { idle, live, ended } => {
                let (Some(idle_row), Some(live_row), Some(ended_row)) = (find(idle), find(live), find(ended)) else {
                    failures.push(format!("release: a step is missing ({idle}, {live} or {ended})"));
                    continue;
                };
                let loaded = live_row.footprint.saturating_sub(idle_row.footprint);
                let kept = ended_row.footprint.saturating_sub(idle_row.footprint);
                if loaded >= LOADED_MIB * MIB && kept * 2 > loaded {
                    failures.push(format!(
                        "release: {live} added {} MiB and {ended} still holds {} MiB of it (at most half is allowed)",
                        mib(loaded),
                        mib(kept)
                    ));
                }
            }
            Rule::Flat { from, to, footprint } => {
                let (Some(a), Some(b)) = (find(from), find(to)) else {
                    failures.push(format!("flat: a step is missing ({from} or {to})"));
                    continue;
                };
                if b.footprint > a.footprint + footprint * MIB {
                    failures.push(format!(
                        "flat: the footprint grew {} MiB from {from} to {to} (at most {footprint} MiB)",
                        signed(a.footprint, b.footprint)
                    ));
                }
                if let (Some(x), Some(y)) = (a.metal, b.metal) {
                    if y > x + METAL_MIB * MIB {
                        failures.push(format!(
                            "flat: the Metal allocation grew {} MiB from {from} to {to} (at most {METAL_MIB} MiB)",
                            signed(x, y)
                        ));
                    }
                }
            }
        }
    }
    failures
}

/// Prints the table and fails with the failures, if any.
pub fn report(text: &str, rules: &[Rule]) -> Result<()> {
    let rows = parse(text);
    let table = table(&rows);
    eprintln!("{table}");
    // A run's page in the Actions UI shows the table.
    if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(summary) {
            let _ = writeln!(file, "```\n{table}\n```");
        }
    }
    let failures = check(&rows, rules);
    for failure in &failures {
        eprintln!("::error::{failure}");
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("memory is not flat: {}", failures.join("; ")))
    }
}

/// The host's calls on the real models (desktop/src/voice/memory.rs).
pub const VOICE_RULES: &[Rule] = &[
    Rule::Release { idle: "idle", live: "call-live", ended: "call-ended" },
    Rule::Flat { from: "call-ended", to: "calls-many", footprint: FOOTPRINT_MIB },
    Rule::Flat { from: "call-ended", to: "changes-ended", footprint: FOOTPRINT_MIB },
    Rule::Flat { from: "call-ended", to: "preload-input-ended", footprint: FOOTPRINT_MIB },
    Rule::Flat { from: "call-ended", to: "preload-output-ended", footprint: FOOTPRINT_MIB },
    Rule::Flat { from: "call-ended", to: "preload-both-ended", footprint: FOOTPRINT_MIB },
];

/// The probe app's long session (test/fixtures/memory.html).
pub const APP_RULES: &[Rule] = &[
    Rule::Release { idle: "idle", live: "models-live", ended: "models-ended" },
    Rule::Flat { from: "models-ended", to: "calls-many", footprint: FOOTPRINT_MIB },
    Rule::Flat { from: "calls-many", to: "card-cycle", footprint: FOOTPRINT_MIB },
    Rule::Flat { from: "card-cycle", to: "devices-cycle", footprint: FOOTPRINT_MIB },
    Rule::Flat { from: "devices-cycle", to: "devices-idle", footprint: IDLE_MIB },
    Rule::Flat { from: "devices-idle", to: "settings-ended", footprint: FOOTPRINT_MIB },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn line(phase: &str, footprint_mib: u64, metal_mib: Option<u64>) -> String {
        format!(
            "memory phase={phase} footprint={} resident={} metal={} accelerator=na",
            footprint_mib * MIB,
            footprint_mib * MIB,
            metal_mib.map_or("na".to_owned(), |m| (m * MIB).to_string())
        )
    }

    #[test]
    fn lines_are_read_wherever_they_sit_in_the_log() {
        let text = format!("noise\nsidevoice: page says: {}\n{}\n", line("idle", 100, None), line("a", 5, Some(2)));
        let rows = parse(&text);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].phase.as_str(), rows[0].footprint, rows[0].metal), ("idle", 100 * MIB, None));
        assert_eq!(rows[1].metal, Some(2 * MIB));
        // A repeated phase keeps its place and its last values.
        let again = parse(&format!("{}\n{}\n{}\n", line("x", 1, None), line("y", 2, None), line("x", 3, None)));
        assert_eq!(
            again.iter().map(|r| (r.phase.as_str(), r.footprint / MIB)).collect::<Vec<_>>(),
            [("x", 3), ("y", 2)]
        );
    }

    #[test]
    fn what_loaded_must_be_given_back() {
        let rules = [Rule::Release { idle: "idle", live: "live", ended: "ended" }];
        let ok =
            parse(&format!("{}\n{}\n{}\n", line("idle", 100, None), line("live", 700, None), line("ended", 180, None)));
        assert!(check(&ok, &rules).is_empty());
        let kept =
            parse(&format!("{}\n{}\n{}\n", line("idle", 100, None), line("live", 700, None), line("ended", 600, None)));
        assert_eq!(check(&kept, &rules).len(), 1);
        // A small load is noise.
        let small =
            parse(&format!("{}\n{}\n{}\n", line("idle", 100, None), line("live", 150, None), line("ended", 150, None)));
        assert!(check(&small, &rules).is_empty());
        assert_eq!(check(&ok[..2], &rules).len(), 1, "a missing step fails");
    }

    #[test]
    fn repeated_work_may_not_grow() {
        let rules = [Rule::Flat { from: "a", to: "b", footprint: FOOTPRINT_MIB }];
        let flat = parse(&format!("{}\n{}\n", line("a", 300, Some(10)), line("b", 340, Some(30))));
        assert!(check(&flat, &rules).is_empty());
        let grows = parse(&format!("{}\n{}\n", line("a", 300, Some(10)), line("b", 400, Some(60))));
        assert_eq!(check(&grows, &rules).len(), 2, "the footprint and the Metal allocation");
        let no_gpu = parse(&format!("{}\n{}\n", line("a", 300, None), line("b", 310, Some(900))));
        assert!(check(&no_gpu, &rules).is_empty(), "Metal is compared only where both steps have it");
    }

    #[test]
    fn the_table_is_compact() {
        let rows = parse(&format!("{}\n{}\n", line("idle", 100, None), line("live", 640, Some(300))));
        let text = table(&rows);
        assert!(text.contains("idle") && text.contains("+540"), "{text}");
        assert_eq!(text.lines().count(), 4);
    }
}
