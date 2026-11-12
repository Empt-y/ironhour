//! Just enough C++ understanding to pull data tables out of the original source:
//! comment/preprocessor stripping, scope tracking (classes, functions), and
//! brace-balanced initializer extraction.

use std::sync::LazyLock;

use regex::Regex;

/// Removes comments, `#if 0` regions and other preprocessor lines. String and char
/// literals are preserved. Line structure is kept so offsets stay meaningful.
pub fn strip(src: &str) -> String {
    let no_comments = strip_comments(src);
    let mut out = String::with_capacity(no_comments.len());
    let mut dead_depth = 0usize; // >0 while inside #if 0
    let mut if_depth = 0usize;
    let mut continuation = false;
    for line in no_comments.lines() {
        let t = line.trim_start();
        if continuation {
            continuation = t.trim_end().ends_with('\\');
            out.push('\n');
            continue;
        }
        if let Some(d) = t.strip_prefix('#') {
            let d = d.trim_start();
            let word = d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
            match word {
                "if" | "ifdef" | "ifndef" => {
                    if_depth += 1;
                    if dead_depth > 0 {
                        dead_depth += 1;
                    } else if word == "if" && d[2..].trim() == "0" {
                        dead_depth = 1;
                    }
                }
                "else" | "elif" => {
                    if dead_depth == 1 {
                        // `#if 0 ... #else` : the else branch is live.
                        dead_depth = 0;
                    }
                }
                "endif" => {
                    if_depth = if_depth.saturating_sub(1);
                    dead_depth = dead_depth.saturating_sub(1);
                }
                _ => {}
            }
            continuation = t.trim_end().ends_with('\\');
            out.push('\n');
            continue;
        }
        if dead_depth == 0 {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let q = b[i];
                out.push(q);
                i += 1;
                while i < b.len() && b[i] != q && b[i] != b'\n' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        out.push(b[i]);
                        i += 1;
                    }
                    out.push(b[i]);
                    i += 1;
                }
                if i < b.len() {
                    out.push(b[i]);
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] == b'\n' {
                        out.push(b'\n');
                    }
                    i += 1;
                }
                i += 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Index just past the brace matching the `{` at `open`.
pub fn match_brace(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    debug_assert_eq!(b[open], b'{');
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[derive(Debug, Clone)]
pub struct Class {
    pub name: String,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Function {
    /// Qualified name, e.g. `ActiveBodyModuleData::buildFieldParse`.
    pub name: String,
    pub body: String,
    /// Byte range of the body in the stripped file.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub classes: Vec<Class>,
    pub functions: Vec<Function>,
}

static CLASS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)(?:^|[\s;}])(?:class|struct)\s+(?:\w+\s+)?(\w+)\s*(?::\s*([^{;]*))?$").unwrap());
static FUNC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)([~\w]+(?:\s*::\s*[~\w]+)*)\s*\(").unwrap());

const NOT_FUNCTIONS: &[&str] = &["if", "for", "while", "switch", "catch", "return", "sizeof", "do", "else"];

/// Walks a stripped file and records classes (with base classes) and function bodies.
pub fn scan(src: &str) -> Scan {
    let mut out = Scan::default();
    walk(src, 0, src.len(), &mut Vec::new(), &mut out);
    out
}

fn walk(src: &str, start: usize, end: usize, classes: &mut Vec<String>, out: &mut Scan) {
    let b = src.as_bytes();
    let mut header_start = start;
    let mut i = start;
    while i < end {
        match b[i] {
            b'"' | b'\'' => {
                let q = b[i];
                i += 1;
                while i < end && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                continue;
            }
            b';' | b'}' => header_start = i + 1,
            b'{' => {
                let header = src[header_start..i].trim();
                let close = match_brace(src, i).unwrap_or(end).min(end);
                if header.ends_with('=') || header.ends_with(']') || header.ends_with(',') {
                    // Initializer list: not a scope.
                } else if let Some(c) = CLASS_RE.captures(header) {
                    let name = c[1].to_string();
                    let parents = c
                        .get(2)
                        .map(|m| {
                            m.as_str()
                                .split(',')
                                .filter_map(|p| p.split_whitespace().last().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    out.classes.push(Class { name: name.clone(), parents });
                    classes.push(name);
                    walk(src, i + 1, close - 1, classes, out);
                    classes.pop();
                } else if header.contains('(') {
                    // Function definition if the first call-like name is not a keyword.
                    let before_init = header.split(") :").next().unwrap_or(header);
                    if let Some(c) = FUNC_RE.captures(before_init) {
                        let raw: String = c[1].chars().filter(|c| !c.is_whitespace()).collect();
                        let last = raw.rsplit("::").next().unwrap_or(&raw);
                        if !NOT_FUNCTIONS.contains(&last) && !header.starts_with("namespace") {
                            let name = match (raw.contains("::"), classes.last()) {
                                (false, Some(cls)) => format!("{cls}::{raw}"),
                                _ => raw,
                            };
                            out.functions.push(Function {
                                name,
                                body: src[i..close].to_string(),
                                start: i,
                                end: close,
                            });
                        }
                    }
                } else if header.starts_with("namespace") || header.starts_with("extern") {
                    walk(src, i + 1, close - 1, classes, out);
                }
                i = close;
                header_start = close;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
}

/// Splits `s` on commas that are not nested inside (), {} or <> and not in strings.
pub fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                cur.push(c);
                while let Some(d) = chars.next() {
                    cur.push(d);
                    if d == '\\' {
                        if let Some(e) = chars.next() {
                            cur.push(e);
                        }
                    } else if d == c {
                        break;
                    }
                }
            }
            '(' | '{' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | '}' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Contents of every top-level `{ ... }` group inside an initializer body.
pub fn brace_groups(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = body[i..].find('{') {
        let open = i + off;
        let Some(close) = match_brace(body, open) else { break };
        out.push(body[open + 1..close - 1].to_string());
        i = close;
    }
    out
}

/// Unquotes a C string literal (simple escapes only). `None` if `s` is not one.
pub fn string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_classes_and_functions() {
        let src = strip(
            r#"
// comment with { brace
class FooModuleData : public BarModuleData
{
public:
    static void buildFieldParse(MultiIniFieldParse& p) { BarModuleData::buildFieldParse(p); }
};
#if 0
void Dead::fn() { }
#endif
void Foo::other(int a, int b) : x(1) { if (a) { } }
"#,
        );
        let s = scan(&src);
        assert_eq!(s.classes[0].name, "FooModuleData");
        assert_eq!(s.classes[0].parents, ["BarModuleData"]);
        let names: Vec<_> = s.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["FooModuleData::buildFieldParse", "Foo::other"]);
    }

    #[test]
    fn splits_entries() {
        let parts = split_top_level(r#""Name", INI::parseReal, NULL, offsetof( A, m_b )"#);
        assert_eq!(parts, ["\"Name\"", "INI::parseReal", "NULL", "offsetof( A, m_b )"]);
    }
}
