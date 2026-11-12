//! `ih-tools`: inspect game data through the same file system the engine uses.

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
    /// Parse every INI file under a folder and report structure problems and usage.
    Scan {
        /// Folders to scan (recursively).
        #[arg(default_values_t = ["Data\\INI".to_string(), "Data\\English".to_string(), "Maps".to_string()])]
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
        Cmd::Scan { dirs, modules, parsers } => scan(&mount(&cli)?, dirs, *modules, *parsers)?,
    }
    Ok(())
}

fn scan(vfs: &Vfs, dirs: &[String], show_modules: bool, show_parsers: bool) -> Result<()> {
    use std::collections::BTreeMap;

    // Files shipped in the archives that no code path in the original loads.
    // Replaced by the real load-order manifest in M1.
    const NEVER_LOADED: &[&str] = &["data\\ini\\default\\buttonsets.ini"];

    let mut files = Vec::new();
    for d in dirs {
        files.extend(vfs.list(d, "*.ini", true));
    }
    files.retain(|f| {
        let skip = NEVER_LOADED.contains(&ih_vfs::path::key(f).as_str());
        if skip {
            println!("{f}: skipped (never loaded by the game)");
        }
        !skip
    });
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut nested: BTreeMap<String, usize> = BTreeMap::new();
    let mut module_classes: BTreeMap<String, usize> = BTreeMap::new();
    let (mut lines, mut fields, mut problems, mut bad_files) = (0u64, 0u64, 0usize, 0usize);
    let mut report = ih_ini::validate::Report::default();

    for f in &files {
        let Some(bytes) = vfs.read(f)? else { continue };
        let doc = ih_ini::parse(&bytes);
        report.validate(f, &doc);
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
