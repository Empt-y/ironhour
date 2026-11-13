//! Extracts the INI schema from the original C++ source.
//!
//! Usage: `cargo run -p ih-schemagen -- <GeneralsMD/Code> <output.rs>`
//!
//! Collects:
//! - every `FieldParse` table (file-scope and function-local),
//! - every `buildFieldParse` function and the order in which it chains parent
//!   builders and adds tables (`MultiIniFieldParse::add` appends; first match wins),
//! - module class -> module data class (`MAKE_STANDARD_MODULE_*` macros and
//!   inheritance), and the module names registered with the module factory,
//! - name lists (`const char *X[]`) and lookup lists (`LookupListRec X[]`) used as
//!   field parser user data.
//!
//! The result is written as Rust source with static tables.

mod cpp;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;

#[derive(Debug, Clone)]
struct Field {
    /// `None` for a catch-all entry (NULL token with a parser).
    name: Option<String>,
    parser: String,
    user: String,
}

#[derive(Debug, Clone)]
struct Table {
    id: String,
    file: String,
    fields: Vec<Field>,
}

#[derive(Debug, Clone)]
enum Step {
    Parent(String),
    Add(String),
}

#[derive(Default)]
struct Db {
    tables: BTreeMap<String, Table>,
    /// buildFieldParse function -> ordered steps.
    builders: HashMap<String, Vec<Step>>,
    /// Function name -> table it returns (e.g. `X::getFieldParse`).
    getters: HashMap<String, String>,
    /// Function name -> its local field tables, in declaration order.
    function_tables: HashMap<String, Vec<String>>,
    class_parents: HashMap<String, Vec<String>>,
    module_data: HashMap<String, String>,
    registered: Vec<(String, &'static str)>,
    name_lists: BTreeMap<String, Vec<String>>,
    lookup_lists: BTreeMap<String, Vec<(String, String)>>,
}

static TABLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"FieldParse\s+([\w:]+)\s*\[\s*\w*\s*\]\s*=\s*\{").unwrap());
static NAMES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:const\s+)?char\s*\*\s*(?:const\s+)?([\w:]+)\s*\[\s*[\w+\- ]*\s*\]\s*=\s*\{").unwrap()
});
static LOOKUP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"LookupListRec\s+([\w:]+)\s*\[\s*\w*\s*\]\s*=\s*\{").unwrap());
static PARENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([\w:]+)::buildFieldParse\s*\(\s*\w+\s*\)").unwrap());
static ADD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\w+\s*\.\s*add\s*\(\s*([\w:]+(?:\s*\(\s*\))?)").unwrap());
static RETURN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"return\s+([\w:]+)\s*;").unwrap());
static MODDATA_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"MAKE_STANDARD_MODULE_(?:MACRO_WITH_MODULE_DATA|DATA_MACRO_ABC)\s*\(\s*(\w+)\s*,\s*(\w+)\s*\)").unwrap()
});
static ADDMODULE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"addModule\s*\(\s*(\w+)\s*\)").unwrap());
static TYPEDEF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"typedef\s+(\w+)\s+(\w+)\s*;").unwrap());

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        bail!("usage: ih-schemagen <GeneralsMD/Code> <output.rs>");
    }
    let root = PathBuf::from(&args[1]);
    let mut files = Vec::new();
    for sub in ["GameEngine", "GameEngineDevice"] {
        collect(&root.join(sub), &mut files)?;
    }
    files.sort();

    let mut db = Db::default();
    for f in &files {
        let raw = std::fs::read(f).with_context(|| f.display().to_string())?;
        let src = cpp::strip(&String::from_utf8_lossy(&raw));
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        ingest(&mut db, &rel, &src);
    }
    // Name lists used as field user data also live in the WWVegas libraries
    // (e.g. TheAnimModeNames in WW3D2/rendobj.h). Only lists are taken from there.
    let mut libs = Vec::new();
    collect(&root.join("Libraries/Source/WWVegas"), &mut libs)?;
    libs.sort();
    for f in &libs {
        let raw = std::fs::read(f).with_context(|| f.display().to_string())?;
        ingest_lists(&mut db, &cpp::strip(&String::from_utf8_lossy(&raw)));
    }

    let out = generate(&db);
    std::fs::write(&args[2], out).with_context(|| args[2].clone())?;
    Ok(())
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| dir.display().to_string())? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cpp") || x.eq_ignore_ascii_case("h")) {
            out.push(p);
        }
    }
    Ok(())
}

fn ingest(db: &mut Db, file: &str, src: &str) {
    let scan = cpp::scan(src);
    for c in &scan.classes {
        db.class_parents.entry(c.name.clone()).or_insert_with(|| c.parents.clone());
    }
    // `typedef ModuleData ClientUpdateModuleData;` behaves like a subclass for our purposes.
    for c in TYPEDEF_RE.captures_iter(src) {
        db.class_parents.entry(c[2].to_string()).or_insert_with(|| vec![c[1].to_string()]);
    }
    for c in MODDATA_RE.captures_iter(src) {
        db.module_data.insert(c[1].to_string(), c[2].to_string());
    }
    if file.ends_with("ModuleFactory.cpp") {
        let kind = if file.contains("W3DDevice") { "W3D" } else { "Engine" };
        for c in ADDMODULE_RE.captures_iter(src) {
            db.registered.push((c[1].to_string(), kind));
        }
    }

    // Enclosing function of a byte offset, if any.
    let owner =
        |pos: usize| scan.functions.iter().filter(|f| f.start <= pos && pos < f.end).min_by_key(|f| f.end - f.start);

    // Field tables.
    let mut local_tables: HashMap<String, Vec<(String, String)>> = HashMap::new(); // func -> (local, id)
    for m in TABLE_RE.captures_iter(src) {
        let whole = m.get(0).unwrap();
        let open = whole.end() - 1;
        let Some(close) = cpp::match_brace(src, open) else { continue };
        let decl = m[1].to_string();
        let id = match owner(whole.start()) {
            Some(f) => {
                local_tables.entry(f.name.clone()).or_default().push((decl.clone(), format!("{}::{decl}", f.name)));
                format!("{}::{decl}", f.name)
            }
            None => decl,
        };
        let fields = parse_entries(&src[open + 1..close - 1]);
        db.tables.entry(id.clone()).or_insert(Table { id, file: file.to_string(), fields });
    }

    ingest_lists(db, src);

    // Builders and getters.
    for f in &scan.functions {
        let locals = local_tables.get(&f.name);
        let resolve_local = |name: &str| {
            locals
                .and_then(|l| l.iter().find(|(n, _)| n == name))
                .map(|(_, id)| id.clone())
                .unwrap_or_else(|| name.to_string())
        };
        if let Some(l) = locals {
            db.function_tables.entry(f.name.clone()).or_insert_with(|| l.iter().map(|(_, id)| id.clone()).collect());
        }
        // Any function that assembles a MultiIniFieldParse (buildFieldParse, OCL nugget
        // parsers, ...) is recorded with its steps.
        if f.name.ends_with("::buildFieldParse") || ADD_RE.is_match(&f.body) && f.body.contains("FieldParse") {
            let mut steps: Vec<(usize, Step)> = Vec::new();
            for c in PARENT_RE.captures_iter(&f.body) {
                steps.push((c.get(0).unwrap().start(), Step::Parent(format!("{}::buildFieldParse", &c[1]))));
            }
            for c in ADD_RE.captures_iter(&f.body) {
                let arg: String = c[1].chars().filter(|c| !c.is_whitespace()).collect();
                let target = match arg.strip_suffix("()") {
                    Some(getter) => format!("getter:{getter}"),
                    None => resolve_local(&arg),
                };
                steps.push((c.get(0).unwrap().start(), Step::Add(target)));
            }
            steps.sort_by_key(|(p, _)| *p);
            db.builders.insert(f.name.clone(), steps.into_iter().map(|(_, s)| s).collect());
        }
        if let Some(c) = RETURN_RE.captures(&f.body)
            && locals.is_some_and(|l| l.iter().any(|(n, _)| *n == c[1]))
        {
            db.getters.insert(f.name.clone(), resolve_local(&c[1]));
        }
    }
}

/// Strips a class qualifier: `Foo::TheNames` -> `TheNames`. User data refers to lists
/// by their unqualified or qualified name; both forms are looked up.
fn short(name: &str) -> String {
    name.to_string()
}

fn parse_entries(body: &str) -> Vec<Field> {
    let mut out = Vec::new();
    for g in cpp::brace_groups(body) {
        let parts = cpp::split_top_level(&g);
        if parts.len() < 2 {
            continue;
        }
        let parser = parts[1].trim().to_string();
        let user = parts.get(2).map(|s| s.trim().to_string()).unwrap_or_default();
        match cpp::string_literal(&parts[0]) {
            Some(name) => out.push(Field { name: Some(name), parser, user }),
            None => {
                if parser != "0" && parser != "NULL" && !parser.is_empty() {
                    out.push(Field { name: None, parser, user });
                }
            }
        }
    }
    out
}

impl Db {
    /// Module data class for a module class, following base classes.
    ///
    /// If no ancestor declares one, `Module::friend_newModuleData` applies: a plain
    /// `ModuleData` parsed with no field table, so only `End` is accepted.
    fn data_class(&self, module: &str) -> Option<String> {
        let mut cur = module.to_string();
        for _ in 0..32 {
            if let Some(d) = self.module_data.get(&cur) {
                return Some(d.clone());
            }
            if cur == "Module" {
                return Some("ModuleData".to_string());
            }
            cur = self.class_parents.get(&cur)?.first()?.clone();
        }
        None
    }

    /// Builder function for a data class, following base classes.
    fn builder_for(&self, class: &str) -> Option<String> {
        let mut cur = class.to_string();
        for _ in 0..32 {
            let b = format!("{cur}::buildFieldParse");
            if self.builders.contains_key(&b) {
                return Some(b);
            }
            cur = self.class_parents.get(&cur)?.first()?.clone();
        }
        None
    }

    /// Ordered table ids a builder adds, with parents expanded.
    fn expand(&self, builder: &str, out: &mut Vec<String>, problems: &mut BTreeSet<String>, depth: usize) {
        if depth > 32 {
            problems.insert(format!("builder recursion at {builder}"));
            return;
        }
        let Some(steps) = self.builders.get(builder) else {
            // `X::buildFieldParse` may be inherited without being redefined.
            let class = builder.trim_end_matches("::buildFieldParse");
            match self.class_parents.get(class).and_then(|p| p.first()).and_then(|p| self.builder_for(p)) {
                Some(b) => self.expand(&b, out, problems, depth + 1),
                None if class == "ModuleData" => {}
                None => {
                    problems.insert(format!("no builder for {class}"));
                }
            }
            return;
        };
        for s in steps {
            match s {
                Step::Parent(p) => self.expand(p, out, problems, depth + 1),
                Step::Add(t) => {
                    let id = match t.strip_prefix("getter:") {
                        Some(g) => match self.getters.get(g).or_else(|| {
                            // Unqualified call from inside a class method: try the class.
                            let class = builder.rsplit_once("::").map(|(c, _)| c)?;
                            self.getters.get(&format!("{class}::{g}"))
                        }) {
                            Some(id) => id.clone(),
                            None => {
                                problems.insert(format!("{builder}: unresolved getter {g}"));
                                continue;
                            }
                        },
                        None => t.clone(),
                    };
                    if self.tables.contains_key(&id) {
                        out.push(id);
                    } else {
                        problems.insert(format!("{builder}: unknown table {id}"));
                    }
                }
            }
        }
    }
}

fn q(s: &str) -> String {
    format!("{s:?}")
}

fn generate(db: &Db) -> String {
    let mut problems = BTreeSet::new();
    let mut o = String::new();
    o.push_str(
        "// @generated by ih-schemagen from the EA Zero Hour source (GeneralsMD/Code).\n\
         // Do not edit by hand; rerun the generator instead.\n\
         #![allow(clippy::all)]\n\n\
         use super::{FieldDef, ModuleDef, TableDef};\n\n",
    );

    // Tables.
    o.push_str("pub static TABLES: &[TableDef] = &[\n");
    for t in db.tables.values() {
        let _ = writeln!(o, "    TableDef {{ id: {}, file: {}, fields: &[", q(&t.id), q(&t.file));
        for f in &t.fields {
            let name = f.name.as_deref().map_or("None".into(), |n| format!("Some({})", q(n)));
            let _ = writeln!(o, "        FieldDef {{ name: {name}, parser: {}, user: {} }},", q(&f.parser), q(&f.user));
        }
        o.push_str("    ] },\n");
    }
    o.push_str("];\n\n");

    // Modules.
    o.push_str("pub static MODULES: &[ModuleDef] = &[\n");
    let mut seen = BTreeSet::new();
    for (name, origin) in &db.registered {
        if !seen.insert(name.clone()) {
            continue;
        }
        let data = db.data_class(name);
        let mut tables = Vec::new();
        match data.as_deref().and_then(|d| db.builder_for(d)) {
            Some(b) => db.expand(&b, &mut tables, &mut problems, 0),
            None => {
                problems.insert(format!("module {name}: no data class or builder"));
            }
        }
        let list = tables.iter().map(|t| q(t)).collect::<Vec<_>>().join(", ");
        let _ = writeln!(
            o,
            "    ModuleDef {{ name: {}, origin: {}, data_class: {}, tables: &[{list}] }},",
            q(name),
            q(origin),
            q(data.as_deref().unwrap_or(""))
        );
    }
    o.push_str("];\n\n");

    // Parser function -> tables its nested block is parsed with. A field whose parser
    // appears here opens a nested block (or, for module names, see ModuleDef).
    o.push_str("pub static NESTED: &[(&str, &[&str])] = &[\n");
    let mut funcs: BTreeSet<&String> = db.function_tables.keys().collect();
    funcs.extend(db.builders.keys());
    for f in funcs {
        if f.ends_with("::buildFieldParse") {
            continue; // module data builders are reached through MODULES
        }
        let mut tables = Vec::new();
        if db.builders.contains_key(f.as_str()) {
            // Generic helpers (e.g. INI::initFromINI) also call `.add`; their
            // unresolvable arguments are not schema problems.
            db.expand(f, &mut tables, &mut BTreeSet::new(), 0);
        } else {
            tables = db.function_tables[f].clone();
        }
        if tables.is_empty() {
            continue;
        }
        let list = tables.iter().map(|t| q(t)).collect::<Vec<_>>().join(", ");
        let _ = writeln!(o, "    ({}, &[{list}]),", q(f));
    }
    o.push_str("];\n\n");

    // Name lists and lookup lists.
    o.push_str("pub static NAME_LISTS: &[(&str, &[&str])] = &[\n");
    for (k, v) in &db.name_lists {
        let _ = writeln!(o, "    ({}, &[{}]),", q(k), v.iter().map(|s| q(s)).collect::<Vec<_>>().join(", "));
    }
    o.push_str("];\n\n");
    o.push_str("pub static LOOKUP_LISTS: &[(&str, &[(&str, &str)])] = &[\n");
    for (k, v) in &db.lookup_lists {
        let pairs = v.iter().map(|(a, b)| format!("({}, {})", q(a), q(b))).collect::<Vec<_>>().join(", ");
        let _ = writeln!(o, "    ({}, &[{pairs}]),", q(k));
    }
    o.push_str("];\n");

    eprintln!(
        "{} tables, {} builders, {} getters, {} modules registered, {} name lists, {} lookup lists",
        db.tables.len(),
        db.builders.len(),
        db.getters.len(),
        seen.len(),
        db.name_lists.len(),
        db.lookup_lists.len()
    );
    for p in &problems {
        eprintln!("problem: {p}");
    }
    o
}

/// Name lists (`const char *X[]`) and lookup lists (`LookupListRec X[]`).
fn ingest_lists(db: &mut Db, src: &str) {
    for m in NAMES_RE.captures_iter(src) {
        let open = m.get(0).unwrap().end() - 1;
        let Some(close) = cpp::match_brace(src, open) else { continue };
        let names: Vec<String> =
            cpp::split_top_level(&src[open + 1..close - 1]).iter().filter_map(|s| cpp::string_literal(s)).collect();
        if !names.is_empty() {
            db.name_lists.entry(short(&m[1])).or_insert(names);
        }
    }
    for m in LOOKUP_RE.captures_iter(src) {
        let open = m.get(0).unwrap().end() - 1;
        let Some(close) = cpp::match_brace(src, open) else { continue };
        let pairs: Vec<(String, String)> = cpp::brace_groups(&src[open + 1..close - 1])
            .iter()
            .filter_map(|g| {
                let parts = cpp::split_top_level(g);
                Some((cpp::string_literal(parts.first()?)?, parts.get(1)?.trim().to_string()))
            })
            .collect();
        db.lookup_lists.entry(short(&m[1])).or_insert(pairs);
    }
}
