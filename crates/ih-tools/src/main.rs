//! `ih-tools`: inspect game data through the same file system the engine uses.

mod coverage;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ih_vfs::{Installs, ModProfile, Vfs};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Mod profile (TOML). Defaults to the stock game.
    #[arg(long, global = true)]
    profile: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the detected game installs.
    Installs,
    /// List mounted archives in mount order.
    Archives,
    /// List files. The pattern is a folder plus a file mask, e.g. "Data\INI\Object\*.ini".
    Ls {
        #[arg(default_value = "*")]
        pattern: String,
        /// Only the folder itself, not subfolders (archives always recurse, as in the original).
        #[arg(long)]
        shallow: bool,
    },
    /// Show which source provides a file, and what it overrides.
    Which { path: String },
    /// Copy files out of the virtual file system. The pattern works as for `ls`.
    Extract { pattern: String, out: PathBuf },
    /// Print a text file to stdout.
    Cat { path: String },
    /// Print the INI files the game loads at startup, in order, with where each comes from.
    LoadOrder,
    /// Report how much of the mounted data ironhour understands (Markdown).
    Coverage {
        /// Also write the report to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Load the game's string table; look up labels, or print a summary.
    Strings {
        /// Labels to look up (case-insensitive).
        labels: Vec<String>,
        /// A map's folder, e.g. "Maps\Tournament Desert", to add its map.str.
        #[arg(long)]
        map: Option<String>,
    },
    /// Parse and check INI files: by default exactly the files the game loads, in load
    /// order, plus every per-map override file.
    Scan {
        /// Scan every *.ini under these folders instead of following the load order.
        dirs: Vec<String>,
        /// Also print the most used module class names.
        #[arg(long)]
        modules: bool,
        /// Also print how many fields use each C++ value parser.
        #[arg(long)]
        parsers: bool,
    },
}

fn mount(cli: &Cli) -> Result<Vfs> {
    let installs = Installs::locate();
    if installs.zero_hour.is_none() {
        bail!(
            "Zero Hour install not found. Set {} (and {} for base Generals).",
            ih_vfs::install::ENV_ZERO_HOUR,
            ih_vfs::install::ENV_GENERALS
        );
    }
    let profile = match &cli.profile {
        Some(p) => ModProfile::load(p)?,
        None => ModProfile::stock(),
    };
    let vfs = Vfs::for_game(&installs, &profile)?;
    for w in vfs.warnings() {
        eprintln!("warning: {}", w.0);
    }
    Ok(vfs)
}

/// Splits "dir\mask" into ("dir", "mask"). A pattern without wildcards is a folder.
fn split_pattern(pattern: &str) -> (String, String) {
    match pattern.rfind(['\\', '/']) {
        Some(i) if pattern[i + 1..].contains(['*', '?']) => (pattern[..i].to_string(), pattern[i + 1..].to_string()),
        _ if pattern.contains(['*', '?']) => (String::new(), pattern.to_string()),
        _ => (pattern.to_string(), "*".to_string()),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Installs => {
            let i = Installs::locate();
            println!("Zero Hour: {}", i.zero_hour.as_ref().map_or("not found".into(), |p| p.display().to_string()));
            println!("Generals:  {}", i.generals.as_ref().map_or("not found".into(), |p| p.display().to_string()));
        }
        Cmd::Archives => {
            let vfs = mount(&cli)?;
            for (i, m) in vfs.archives().iter().enumerate() {
                println!(
                    "{:3} {} {:6} files  {}",
                    i,
                    if m.overwrite { "mod " } else { "    " },
                    m.archive.entries().len(),
                    m.archive.path().display()
                );
            }
            let loose = vfs.loose_root().map_or(0, |l| l.len());
            println!(
                "{} archives, {} reachable archive files, {} loose files",
                vfs.archives().len(),
                vfs.archive_file_count(),
                loose
            );
        }
        Cmd::Ls { pattern, shallow } => {
            let vfs = mount(&cli)?;
            let (dir, mask) = split_pattern(pattern);
            let files = vfs.list(&dir, &mask, !shallow);
            for f in &files {
                println!("{f}");
            }
            eprintln!("{} files", files.len());
        }
        Cmd::Which { path } => {
            let vfs = mount(&cli)?;
            let sources = vfs.all_sources(path);
            if sources.is_empty() {
                bail!("{path}: not found");
            }
            for (i, s) in sources.iter().enumerate() {
                println!("{} {}", if i == 0 { "used:      " } else { "overridden:" }, s.describe());
            }
        }
        Cmd::Extract { pattern, out } => {
            let vfs = mount(&cli)?;
            let files = if vfs.exists(pattern) {
                vec![pattern.clone()]
            } else {
                let (dir, mask) = split_pattern(pattern);
                vfs.list(&dir, &mask, true)
            };
            if files.is_empty() {
                bail!("{pattern}: no files");
            }
            let mut n = 0;
            for f in &files {
                let Some(bytes) = vfs.read(f)? else { continue };
                let dest = out.join(f.replace('\\', std::path::MAIN_SEPARATOR_STR));
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).with_context(|| parent.display().to_string())?;
                }
                std::fs::write(&dest, bytes).with_context(|| dest.display().to_string())?;
                n += 1;
            }
            eprintln!("extracted {n} files to {}", out.display());
        }
        Cmd::Cat { path } => {
            let vfs = mount(&cli)?;
            let bytes = vfs.read(path)?.with_context(|| format!("{path}: not found"))?;
            print!("{}", String::from_utf8_lossy(&bytes));
        }
        Cmd::Strings { labels, map } => {
            let vfs = mount(&cli)?;
            let (main, from) = load_strings(&vfs, &Installs::locate().language)?;
            let map_table = match map {
                Some(m) => {
                    let p = format!("{}\\map.str", m.trim_end_matches(['\\', '/']));
                    let bytes = vfs.read(&p)?.with_context(|| format!("{p}: not found"))?;
                    Some(ih_formats::strings::parse_str(&bytes))
                }
                None => None,
            };
            if labels.is_empty() {
                let speech = main.entries.iter().filter(|e| e.speech.is_some()).count();
                println!("{from}: {} strings ({speech} with speech), language id {}", main.len(), main.language_id);
                if let Some(t) = &map_table {
                    println!("map.str: {} strings", t.len());
                }
            }
            for l in labels {
                println!("{l} = {:?}", ih_formats::strings::fetch(&main, map_table.as_ref(), l));
            }
        }
        Cmd::Coverage { out } => {
            let vfs = mount(&cli)?;
            let language = Installs::locate().language;
            let (strings, _) = load_strings(&vfs, &language)?;
            let name = match &cli.profile {
                Some(p) => ModProfile::load(p)?.name,
                None => ModProfile::stock().name,
            };
            let text = coverage::run(&vfs, &language, &name, &strings)?;
            print!("{text}");
            if let Some(path) = out {
                std::fs::write(path, &text).with_context(|| path.display().to_string())?;
            }
        }
        Cmd::LoadOrder => {
            use ih_ini::load_order::{BEFORE_MODS, STARTUP, plan};
            let vfs = mount(&cli)?;
            let (files, missing) = plan(&vfs, &Installs::locate().language, STARTUP);
            for (i, p) in files.iter().enumerate() {
                let mods =
                    if STARTUP[..BEFORE_MODS].iter().any(|s| std::ptr::eq(s, p.step)) { " (before -mod)" } else { "" };
                let from = vfs.resolve(&p.path).map(|s| s.describe()).unwrap_or_default();
                println!("{:4} {:<24} {}{mods}\n       {from}", i + 1, p.step.subsystem, p.path);
            }
            for m in &missing {
                println!("MISSING {:?} ({})", m.source, m.cpp);
            }
        }
        Cmd::Scan { dirs, modules, parsers } => {
            scan(&mount(&cli)?, &Installs::locate().language, dirs, *modules, *parsers)?
        }
    }
    Ok(())
}

/// The main string table, as `GameTextManager::init` picks it: a loose
/// `Data\Generals.str` if present, otherwise `Data\<language>\Generals.csf`.
fn load_strings(vfs: &Vfs, language: &str) -> Result<(ih_formats::strings::StringTable, String)> {
    if let Some(b) = vfs.read("Data\\Generals.str")? {
        return Ok((ih_formats::strings::parse_str(&b), "Data\\Generals.str".into()));
    }
    let p = format!("Data\\{language}\\Generals.csf");
    let b = vfs.read(&p)?.with_context(|| format!("{p}: not found"))?;
    Ok((ih_formats::strings::parse_csf(&b)?, p))
}

fn scan(vfs: &Vfs, language: &str, dirs: &[String], show_modules: bool, show_parsers: bool) -> Result<()> {
    use std::collections::BTreeMap;

    let mut files = Vec::new();
    if dirs.is_empty() {
        use ih_ini::load_order::{ON_DEMAND, STARTUP, map_files, plan};
        let (startup, missing) = plan(vfs, language, STARTUP);
        for m in &missing {
            println!("missing required file: {:?} ({})", m.source, m.cpp);
        }
        let (later, _) = plan(vfs, language, ON_DEMAND);
        files.extend(startup.into_iter().chain(later).map(|p| p.path));
        files.extend(map_files(vfs));
        println!("load order: {} files, {} required files missing", files.len(), missing.len());
    } else {
        for d in dirs {
            files.extend(vfs.list(d, "*.ini", true));
        }
    }
    // Some files load twice (ShellMenuScheme); check each once.
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(ih_vfs::path::key(f)));
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut nested: BTreeMap<String, usize> = BTreeMap::new();
    let mut module_classes: BTreeMap<String, usize> = BTreeMap::new();
    let (mut lines, mut fields, mut problems, mut bad_files) = (0u64, 0u64, 0usize, 0usize);
    let mut report = ih_ini::validate::Report::default();
    let (mut values_ok, mut invalid_count) = (0u64, 0u64);
    let mut invalid: Vec<String> = Vec::new();
    let mut unported: BTreeMap<&'static str, u64> = BTreeMap::new();

    for f in &files {
        let Some(bytes) = vfs.read(f)? else { continue };
        let doc = ih_ini::parse(&bytes);
        report.validate_with(f, &doc, &mut |r| match ih_ini::values::parse_field(r.table, r.def, &r.field.rest) {
            Ok(_) => values_ok += 1,
            Err(ih_ini::values::ValueError::Unimplemented(p)) => *unported.entry(p).or_default() += 1,
            Err(ih_ini::values::ValueError::Invalid(msg)) => {
                if invalid.len() < 40 || std::env::var_os("IH_SCAN_ALL").is_some() {
                    invalid.push(format!(
                        "{f}: line {}: {} = {}: {msg}",
                        r.field.line,
                        r.field.name,
                        r.field.rest.trim()
                    ));
                }
                invalid_count += 1;
            }
        });
        lines += u64::from(doc.line_count);
        if !doc.diagnostics.is_empty() {
            bad_files += 1;
            for d in doc.diagnostics.iter().take(5) {
                println!("{f}: {d}");
            }
            if doc.diagnostics.len() > 5 {
                println!("{f}: ... {} more", doc.diagnostics.len() - 5);
            }
            problems += doc.diagnostics.len();
        }
        for b in &doc.blocks {
            *kinds.entry(b.kind.clone()).or_default() += 1;
            b.walk(&mut |blk, depth| {
                fields += blk.fields().count() as u64;
                if depth > 0 {
                    *nested.entry(blk.kind.clone()).or_default() += 1;
                    if matches!(blk.kind.as_str(), "Behavior" | "Body" | "Draw" | "ClientUpdate")
                        && let Some(class) = blk.args().into_iter().next()
                    {
                        *module_classes.entry(class).or_default() += 1;
                    }
                }
            });
        }
    }

    println!("\n{} files, {lines} lines, {fields} fields", files.len());
    println!("{} top-level blocks of {} kinds:", kinds.values().sum::<usize>(), kinds.len());
    let mut v: Vec<_> = kinds.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for chunk in v.chunks(4) {
        println!("  {}", chunk.iter().map(|(k, n)| format!("{k:<28}{n:>6}")).collect::<Vec<_>>().join(""));
    }
    println!("{} nested blocks of {} kinds", nested.values().sum::<usize>(), nested.len());
    println!("{} distinct module classes used", module_classes.len());
    if show_modules {
        let mut m: Vec<_> = module_classes.into_iter().collect();
        m.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (k, n) in m {
            println!("  {k:<40}{n:>6}");
        }
    }
    if problems == 0 {
        println!("structure: OK, no problems");
    } else {
        println!("structure: {problems} problems in {bad_files} files");
    }

    // Schema check.
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for (file, issue) in &report.issues {
        let kind = format!("{issue:?}").split([' ', '{']).next().unwrap_or("").to_string();
        let n = by_kind.entry(kind).or_default();
        *n += 1;
        if *n <= 15 || std::env::var_os("IH_SCAN_ALL").is_some() {
            println!("{file}: {issue}");
        }
    }
    if report.issues.is_empty() {
        println!("schema: OK, all {} fields known", report.fields);
    } else {
        let summary: Vec<_> = by_kind.iter().map(|(k, n)| format!("{n} {k}")).collect();
        println!("schema: {} issues in {} fields ({})", report.issues.len(), report.fields, summary.join(", "));
    }
    for line in &invalid {
        println!("{line}");
    }
    let unported_total: u64 = unported.values().sum();
    let total = values_ok + invalid_count + unported_total;
    println!(
        "values: {values_ok} parsed, {invalid_count} invalid, {unported_total} with parsers not yet ported ({:.1}% of {total} done, {} parsers left)",
        100.0 * values_ok as f64 / total.max(1) as f64,
        unported.len()
    );
    if show_parsers {
        let mut u: Vec<_> = unported.iter().collect();
        u.sort_by(|a, b| b.1.cmp(a.1));
        println!("parsers not yet ported:");
        for (p, n) in u {
            println!("  {n:>8}  {p}");
        }
    }
    if show_parsers {
        let mut v: Vec<_> = report.parser_use.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!("{} value parsers used:", v.len());
        for (p, n) in v {
            println!("  {n:>8}  {p}");
        }
    }
    Ok(())
}
