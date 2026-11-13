//! Checks parsed INI blocks against the extracted schema.
//!
//! An unknown field or module is a fatal load error in the original game
//! (`INI_UNKNOWN_TOKEN`), so for data the original accepts this should report
//! nothing. It also records which value parsers the data actually uses, which drives
//! porting order.

use std::collections::BTreeMap;

use crate::schema::{Shape, schema};
use crate::structure::SINGLE_LINE_BLOCKS;
use crate::{Block, Document, Item};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// No field of this name in the block's tables.
    UnknownField { path: String, field: String, line: u32 },
    /// `Behavior = <Class>` names a module that is not registered.
    UnknownModule { class: String, line: u32 },
    /// A top-level block type with no table mapping.
    NoSchema { kind: String, line: u32 },
    /// The structure table and the schema disagree about whether a field opens a block.
    StructureMismatch { path: String, field: String, parser: &'static str, line: u32 },
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Issue::UnknownField { path, field, line } => write!(f, "line {line}: unknown field '{field}' in {path}"),
            Issue::UnknownModule { class, line } => write!(f, "line {line}: unknown module '{class}'"),
            Issue::NoSchema { kind, line } => write!(f, "line {line}: no schema for block '{kind}'"),
            Issue::StructureMismatch { path, field, parser, line } => {
                write!(f, "line {line}: '{field}' in {path} parsed with {parser}: block/field mismatch")
            }
        }
    }
}

/// One field occurrence that resolved to a definition, for value-level checks.
#[derive(Debug, Clone, Copy)]
pub struct Resolved<'a> {
    pub table: &'static crate::schema::TableDef,
    pub def: &'static crate::schema::FieldDef,
    pub field: &'a crate::Field,
}

/// Accumulated results over any number of files.
#[derive(Debug, Default)]
pub struct Report {
    pub issues: Vec<(String, Issue)>,
    pub fields: u64,
    /// C++ parser name -> number of fields in the data parsed with it.
    pub parser_use: BTreeMap<&'static str, u64>,
    /// Module class -> number of uses.
    pub module_use: BTreeMap<String, u64>,
}

impl Report {
    /// Checks a document; `on_field` sees every one-line field with its definition.
    pub fn validate_with<'a>(&mut self, file: &str, doc: &'a Document, on_field: &mut dyn FnMut(Resolved<'a>)) {
        let s = schema();
        for b in &doc.blocks {
            if SINGLE_LINE_BLOCKS.contains(&b.kind.as_str()) {
                continue;
            }
            match s.top_level(&b.kind) {
                Some(tables) => self.walk(file, b, tables, &b.kind, on_field),
                None => self.issue(file, Issue::NoSchema { kind: b.kind.clone(), line: b.line }),
            }
        }
    }

    pub fn validate(&mut self, file: &str, doc: &Document) {
        self.validate_with(file, doc, &mut |_| {});
    }

    fn issue(&mut self, file: &str, i: Issue) {
        self.issues.push((file.to_string(), i));
    }

    fn walk<'a>(
        &mut self,
        file: &str,
        block: &'a Block,
        tables: &[&str],
        path: &str,
        on_field: &mut dyn FnMut(Resolved<'a>),
    ) {
        let s = schema();
        for item in &block.items {
            self.fields += 1;
            let (name, line) = match item {
                Item::Field(f) => (&f.name, f.line),
                Item::Block(b) => (&b.kind, b.line),
            };
            let Some((table, def)) = s.find_entry(tables, name) else {
                self.issue(file, Issue::UnknownField { path: path.to_string(), field: name.clone(), line });
                continue;
            };
            *self.parser_use.entry(def.parser).or_default() += 1;
            let mismatch =
                || Issue::StructureMismatch { path: path.to_string(), field: name.clone(), parser: def.parser, line };
            match (item, s.shape(def)) {
                (Item::Field(field), Shape::Field) => on_field(Resolved { table, def, field }),
                (Item::Block(b), Shape::Block(t)) => self.walk(file, b, t, &format!("{path}.{}", b.kind), on_field),
                (Item::Block(b), Shape::Module) => {
                    let class = b.args().into_iter().next().unwrap_or_default();
                    match s.module(&class) {
                        Some(m) => {
                            *self.module_use.entry(class.clone()).or_default() += 1;
                            self.walk(file, b, m.tables, &format!("{path}.{}({class})", b.kind), on_field);
                        }
                        None => self.issue(file, Issue::UnknownModule { class, line }),
                    }
                }
                _ => self.issue(file, mismatch()),
            }
        }
    }
}
