//! `ih-tools coverage`: how much of a data set ironhour understands.
//!
//! Follows the game's own load order, then reports structure, schema and value
//! parsing results, references to definitions that do not exist, the module classes
//! the data uses, and files that ship but are never loaded.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;

use anyhow::Result;
use ih_ini::load_order::{ON_DEMAND, STARTUP, load_directory, map_files, plan};
use ih_ini::validate::Report;
use ih_ini::values::{RefKind, Value, ValueError, parse_field};
use ih_vfs::Vfs;

/// Uses, distinct names, and undefined names with their first use.
type KindStats = (u64, BTreeSet<String>, BTreeMap<String, String>);

/// Which reference kinds a top-level block defines (by its first argument).
fn defines(kind: &str) -> Option<RefKind> {
    Some(match kind {
        "Object" | "ObjectReskin" => RefKind::Object,
        "Weapon" => RefKind::Weapon,
        "FXList" => RefKind::FxList,
        "ObjectCreationList" => RefKind::ObjectCreationList,
        "ParticleSystem" => RefKind::ParticleSystem,
        "Armor" => RefKind::Armor,
        "DamageFX" => RefKind::DamageFx,
        "Upgrade" => RefKind::Upgrade,
        "SpecialPower" => RefKind::SpecialPower,
        "Science" => RefKind::Science,
        "AudioEvent" | "MusicTrack" | "DialogEvent" => RefKind::Audio,
        "MappedImage" => RefKind::MappedImage,
        "CommandButton" => RefKind::CommandButton,
        "Locomotor" => RefKind::Locomotor,
        "CrateData" => RefKind::Crate,
        _ => return None,
    })
}

/// Name matching as the original's lookups do it: `NAMEKEY` is case-sensitive;
/// mapped images and string labels are matched case-insensitively.
fn key(kind: RefKind, name: &str) -> String {
    match kind {
        RefKind::MappedImage | RefKind::Label => name.to_ascii_lowercase(),
        _ => name.to_string(),
    }
}

fn collect_refs(v: &Value, out: &mut Vec<(RefKind, String)>) {
    match v {
        Value::Ref { kind, name: Some(n) } => out.push((*kind, n.clone())),
        Value::Tuple(items) => items.iter().for_each(|i| collect_refs(i, out)),
        _ => {}
    }
}

pub fn run(vfs: &Vfs, language: &str, profile: &str, strings: &ih_formats::strings::StringTable) -> Result<String> {
    let (startup, missing) = plan(vfs, language, STARTUP);
    let (later, _) = plan(vfs, language, ON_DEMAND);
    let maps = map_files(vfs);
    let mut files: Vec<String> = startup.iter().chain(&later).map(|p| p.path.clone()).collect();
    files.extend(maps.iter().cloned());
    let mut seen = HashSet::new();
    files.retain(|f| seen.insert(ih_vfs::path::key(f)));

    let mut report = Report::default();
    let (mut lines, mut blocks, mut structure) = (0u64, 0u64, 0usize);
    let (mut ok, mut invalid, mut unported_total) = (0u64, 0u64, 0u64);
    let mut invalid_examples = Vec::new();
    let mut unported: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut defined: HashSet<(RefKind, String)> = HashSet::new();
    let mut refs: Vec<(RefKind, String, String)> = Vec::new(); // kind, name, where

    for f in &files {
        let Some(bytes) = vfs.read(f)? else { continue };
        let doc = ih_ini::parse(&bytes);
        lines += u64::from(doc.line_count);
        blocks += doc.blocks.len() as u64;
        structure += doc.diagnostics.len();
        for b in &doc.blocks {
            if let (Some(k), Some(name)) = (defines(&b.kind), b.args().into_iter().next()) {
                defined.insert((k, key(k, &name)));
            }
        }
        report.validate_with(f, &doc, &mut |r| match parse_field(r.table, r.def, &r.field.rest) {
            Ok(v) => {
                ok += 1;
                let mut found = Vec::new();
                collect_refs(&v, &mut found);
                for (k, n) in found {
                    refs.push((k, n, format!("{f}:{}", r.field.line)));
                }
            }
            Err(ValueError::Unimplemented(p)) => {
                unported_total += 1;
                *unported.entry(p).or_default() += 1;
            }
            Err(ValueError::Invalid(msg)) => {
                invalid += 1;
                if invalid_examples.len() < 20 {
                    invalid_examples.push(format!("{f}:{} {}: {msg}", r.field.line, r.field.name));
                }
            }
        });
    }

    // Labels resolve against the string table rather than INI definitions.
    for e in &strings.entries {
        defined.insert((RefKind::Label, e.label.to_ascii_lowercase()));
    }

    // Dangling references, per kind.
    let mut by_kind: BTreeMap<RefKind, KindStats> = BTreeMap::new();
    for (k, n, at) in &refs {
        let e = by_kind.entry(*k).or_default();
        e.0 += 1;
        e.1.insert(n.clone());
        if !defined.contains(&(*k, key(*k, n))) {
            e.2.entry(n.clone()).or_insert_with(|| at.clone());
        }
    }

    // Files that ship but are never loaded.
    let mut loaded: HashSet<String> = files.iter().map(|f| ih_vfs::path::key(f)).collect();
    loaded.extend(load_directory(vfs, "Data\\INI\\Object").iter().map(|f| ih_vfs::path::key(f)));
    let mut dead: Vec<String> = vfs
        .list("Data\\INI", "*.ini", true)
        .into_iter()
        .chain(vfs.list(&format!("Data\\{language}"), "*.ini", true))
        .filter(|f| !loaded.contains(&ih_vfs::path::key(f)))
        .collect();
    dead.sort_by(|a, b| ih_vfs::path::cmp_nocase(a, b));

    // Report.
    let mut o = String::new();
    let total = ok + invalid + unported_total;
    let pct = |n: u64| 100.0 * n as f64 / total.max(1) as f64;
    let schema_issues = report.issues.len();
    let _ = writeln!(o, "## Coverage: {profile}\n");
    let _ = writeln!(o, "| Check | Result |\n|---|---|");
    let _ = writeln!(
        o,
        "| Load order | {} startup files, {} on demand, {} map override files; {} required files missing |",
        startup.len(),
        later.len(),
        maps.len(),
        missing.len()
    );
    let _ = writeln!(o, "| INI parsed | {} files, {lines} lines, {blocks} definitions |", files.len());
    let _ = writeln!(o, "| Structure (blocks and `End`) | {structure} problems |");
    let _ = writeln!(o, "| Schema (fields and modules known) | {} fields, {schema_issues} unknown |", report.fields);
    let _ = writeln!(
        o,
        "| Values | {ok} parsed ({:.1}%), {invalid} invalid, {unported_total} use {} parsers not yet ported |",
        pct(ok),
        unported.len()
    );
    let dangling: usize = by_kind.values().map(|e| e.2.len()).sum();
    let _ = writeln!(o, "| References | {} total, {dangling} names that are never defined |", refs.len());
    let _ = writeln!(o, "| Module classes used | {} (simulation not implemented yet: M4) |", report.module_use.len());
    let _ = writeln!(o, "| Files never loaded | {} |", dead.len());

    if !missing.is_empty() {
        let _ = writeln!(o, "\n### Required files missing\n");
        for m in &missing {
            let _ = writeln!(o, "- {:?} ({})", m.source, m.cpp);
        }
    }
    if !invalid_examples.is_empty() {
        let _ = writeln!(o, "\n### Invalid values (first {})\n", invalid_examples.len());
        for e in &invalid_examples {
            let _ = writeln!(o, "- {e}");
        }
    }
    let _ = writeln!(o, "\n### References by kind\n\n| Kind | Uses | Distinct names | Undefined |\n|---|---|---|---|");
    for (k, (uses, names, missing)) in &by_kind {
        let _ = writeln!(o, "| {k:?} | {uses} | {} | {} |", names.len(), missing.len());
    }
    if dangling > 0 {
        let _ = writeln!(
            o,
            "\nUndefined names (first use shown; the original treats these as null, or shows `MISSING:` for labels):\n"
        );
        for (k, (_, _, missing)) in &by_kind {
            for (n, at) in missing.iter().take(15) {
                let _ = writeln!(o, "- {k:?} `{n}` at {at}");
            }
            if missing.len() > 15 {
                let _ = writeln!(o, "- {k:?}: {} more", missing.len() - 15);
            }
        }
    }
    if !unported.is_empty() {
        let mut u: Vec<_> = unported.into_iter().collect();
        u.sort_by_key(|x| std::cmp::Reverse(x.1));
        let _ = writeln!(o, "\n### Value parsers not yet ported\n");
        let _ = writeln!(o, "{}", u.iter().map(|(p, n)| format!("`{p}` ({n})")).collect::<Vec<_>>().join(", "));
    }
    let _ = writeln!(o, "\n### Files never loaded\n");
    for d in &dead {
        let _ = writeln!(o, "- {d}");
    }
    let mut m: Vec<_> = report.module_use.into_iter().collect();
    m.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let _ = writeln!(o, "\n### Module classes by use\n");
    let _ = writeln!(o, "{}", m.iter().map(|(c, n)| format!("{c} ({n})")).collect::<Vec<_>>().join(", "));
    Ok(o)
}
