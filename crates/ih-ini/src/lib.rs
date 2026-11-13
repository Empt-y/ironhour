//! INI files in the original game's dialect.
//!
//! This layer turns a file into a tree of blocks and fields without interpreting
//! values, following `INI::load` and `INI::initFromINIMulti` in
//! `GameEngine/Source/Common/INI/INI.cpp`:
//!
//! - a top-level line starts with one of the 62 block keywords (case-sensitive);
//!   anything else is an error;
//! - a block body runs until a line whose first token is `End` (case-insensitive);
//! - some fields open nested blocks, per [`structure`];
//! - values stay as raw text, because each field's parser tokenises them with its
//!   own separator set.
//!
//! The original aborts the load on the first error. Here errors are collected as
//! [`Diagnostic`]s and parsing resumes, so a whole mod can be checked at once.

pub mod lexer;
pub mod load_order;
pub mod schema;
pub mod structure;
pub mod validate;
pub mod values;

use lexer::{Line, Tokens};
use structure::Ctx;

/// A block: a top-level definition or a nested one.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Keyword or opening field name, e.g. `Object` or `Behavior`.
    pub kind: String,
    /// Raw text after the keyword (after the one delimiter `strtok` consumes).
    pub rest: String,
    pub line: u32,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Field(Field),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub rest: String,
    pub line: u32,
}

impl Block {
    /// Tokens after the keyword, split with the default separators.
    pub fn args(&self) -> Vec<String> {
        split_default(&self.rest)
    }

    pub fn fields(&self) -> impl Iterator<Item = &Field> {
        self.items.iter().filter_map(|i| match i {
            Item::Field(f) => Some(f),
            Item::Block(_) => None,
        })
    }

    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.items.iter().filter_map(|i| match i {
            Item::Block(b) => Some(b),
            Item::Field(_) => None,
        })
    }

    /// Visits this block and every nested block, depth first.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Block, usize)) {
        fn rec<'a>(b: &'a Block, depth: usize, f: &mut impl FnMut(&'a Block, usize)) {
            f(b, depth);
            for c in b.blocks() {
                rec(c, depth + 1, f);
            }
        }
        rec(self, 0, f);
    }
}

impl Field {
    pub fn values(&self) -> Vec<String> {
        split_default(&self.rest)
    }
}

fn split_default(s: &str) -> Vec<String> {
    let bytes: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
    let mut t = Tokens::new(&bytes);
    let mut out = Vec::new();
    while let Some(tok) = t.next_token() {
        out.push(lexer::text(tok));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// A top-level line that does not start with a block keyword
    /// (`INI_UNKNOWN_TOKEN` in the original).
    UnknownBlock(String),
    /// A block still open at end of file (`INI_MISSING_END_TOKEN`).
    MissingEnd { kind: String, opened_at: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: u32,
    pub kind: DiagnosticKind,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            DiagnosticKind::UnknownBlock(t) => write!(f, "line {}: unknown block '{t}'", self.line),
            DiagnosticKind::MissingEnd { kind, opened_at } => {
                write!(f, "line {}: '{kind}' opened at line {opened_at} has no End", self.line)
            }
        }
    }
}

/// A parsed file.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub diagnostics: Vec<Diagnostic>,
    pub line_count: u32,
}

/// Parses one INI file.
pub fn parse(data: &[u8]) -> Document {
    let lines = lexer::lines(data);
    let mut doc = Document { line_count: lines.last().map_or(0, |l| l.number), ..Default::default() };
    let mut i = 0usize;
    while i < lines.len() {
        let line = &lines[i];
        i += 1;
        let mut t = Tokens::new(&line.text);
        let Some(first) = t.next_token() else { continue };
        let kind = lexer::text(first);
        if !structure::is_block_type(&kind) {
            doc.diagnostics.push(Diagnostic { line: line.number, kind: DiagnosticKind::UnknownBlock(kind) });
            continue;
        }
        let mut block = Block { kind, rest: lexer::text(t.rest()), line: line.number, items: Vec::new() };
        if let Some(ctx) = structure::top_level_ctx(&block.kind) {
            parse_body(&lines, &mut i, ctx, &mut block, &mut doc.diagnostics);
        }
        doc.blocks.push(block);
    }
    doc
}

/// Reads fields until `End`. Returns false if the file ran out first.
fn parse_body(lines: &[Line], i: &mut usize, ctx: Ctx, block: &mut Block, diags: &mut Vec<Diagnostic>) -> bool {
    loop {
        let Some(line) = lines.get(*i) else {
            let last = lines.last().map_or(0, |l| l.number);
            diags.push(Diagnostic {
                line: last,
                kind: DiagnosticKind::MissingEnd { kind: block.kind.clone(), opened_at: block.line },
            });
            return false;
        };
        *i += 1;
        let mut t = Tokens::new(&line.text);
        let Some(first) = t.next_token() else { continue };
        if first.eq_ignore_ascii_case(b"End") {
            return true;
        }
        let name = lexer::text(first);
        let rest = lexer::text(t.rest());
        match structure::nested_ctx(ctx, &name) {
            Some(child_ctx) => {
                let mut child = Block { kind: name, rest, line: line.number, items: Vec::new() };
                let closed = parse_body(lines, i, child_ctx, &mut child, diags);
                block.items.push(Item::Block(child));
                if !closed {
                    // Out of lines: report the outer block too, as the original would.
                    diags.push(Diagnostic {
                        line: line.number,
                        kind: DiagnosticKind::MissingEnd { kind: block.kind.clone(), opened_at: block.line },
                    });
                    return false;
                }
            }
            None => block.items.push(Item::Field(Field { name, rest, line: line.number })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"; comment line
Object AmericaTankCrusader
  Side = America ; trailing comment
  Draw = W3DTankDraw ModuleTag_01
    DefaultConditionState
      Model = AVCrusader
    End
    ConditionState = REALLYDAMAGED
      Model = AVCrusader_D
    END
  End
  WeaponSet
    Conditions = None
    Weapon = PRIMARY CrusaderTankGun
  End
  Behavior = AIUpdateInterface ModuleTag_03
    Turret
      TurretTurnRate = 180
    End
  End
end
LODPreset = HIGH Intel 2000 GF4 512
FXList FX_Boom
  Sound
    Name = ExplosionSmall
  End
  ParticleSystem
    Name = SmokeSmall
  End
End
";

    #[test]
    fn nested_structure() {
        let doc = parse(SAMPLE);
        assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
        assert_eq!(doc.blocks.len(), 3);
        let obj = &doc.blocks[0];
        assert_eq!(obj.kind, "Object");
        assert_eq!(obj.args(), ["AmericaTankCrusader"]);
        assert_eq!(obj.fields().next().unwrap().values(), ["America"]);
        let kinds: Vec<_> = obj.blocks().map(|b| b.kind.as_str()).collect();
        assert_eq!(kinds, ["Draw", "WeaponSet", "Behavior"]);
        let draw = obj.blocks().next().unwrap();
        assert_eq!(draw.args(), ["W3DTankDraw", "ModuleTag_01"]);
        assert_eq!(draw.blocks().count(), 2);
        assert_eq!(doc.blocks[1].kind, "LODPreset");
        assert!(doc.blocks[1].items.is_empty());
        assert_eq!(doc.blocks[2].blocks().count(), 2);
    }

    #[test]
    fn errors_are_reported_and_parsing_resumes() {
        let doc = parse(b"Bogus Thing\nWeapon W\n  Damage = 1\nEnd\nObject X\n  Side = A\n");
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.diagnostics.len(), 2);
        assert!(matches!(doc.diagnostics[0].kind, DiagnosticKind::UnknownBlock(ref t) if t == "Bogus"));
        assert!(matches!(doc.diagnostics[1].kind, DiagnosticKind::MissingEnd { opened_at: 5, .. }));
    }
}
