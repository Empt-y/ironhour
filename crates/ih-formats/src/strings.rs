//! String tables: compiled `.csf` and text `.str`.
//!
//! Ported from `GameEngine/Source/GameClient/GameText.cpp` (`parseCSF`,
//! `parseStringFile`, `readToEndOfQuote`, `translateCopy`, `stripSpaces`,
//! `removeLeadingAndTrailing`, `fetch`).
//!
//! The game loads `Data\Generals.str` if it exists, otherwise
//! `Data\<language>\Generals.csf`; a map can add `map.str`, searched after the main
//! table. Labels are matched case-insensitively. A missing label shows as
//! `MISSING: '<label>'`.

use std::collections::HashMap;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StringsError {
    #[error("not a CSF file")]
    NotCsf,
    #[error("truncated CSF file")]
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub text: String,
    /// Speech file attached to the string, if any.
    pub speech: Option<String>,
}

/// A loaded string table.
#[derive(Debug, Clone, Default)]
pub struct StringTable {
    pub entries: Vec<Entry>,
    /// Lowercased label -> first entry with that label.
    index: HashMap<String, usize>,
    /// CSF language id (0 = US English).
    pub language_id: i32,
}

impl StringTable {
    fn from_entries(entries: Vec<Entry>, language_id: i32) -> Self {
        let mut index = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            index.entry(e.label.to_ascii_lowercase()).or_insert(i);
        }
        Self { entries, index, language_id }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, label: &str) -> Option<&Entry> {
        self.index.get(&label.to_ascii_lowercase()).map(|&i| &self.entries[i])
    }

    /// Text for a label, or `None` if it is not in this table.
    pub fn text(&self, label: &str) -> Option<&str> {
        self.get(label).map(|e| e.text.as_str())
    }
}

/// Main table plus an optional per-map table, looked up in that order (`fetch`).
pub fn fetch(main: &StringTable, map: Option<&StringTable>, label: &str) -> String {
    main.text(label)
        .or_else(|| map.and_then(|m| m.text(label)))
        .map(str::to_string)
        .unwrap_or_else(|| format!("MISSING: '{label}'"))
}

const CSF_ID: u32 = u32::from_be_bytes(*b"CSF ");
const CSF_LABEL: u32 = u32::from_be_bytes(*b"LBL ");
const CSF_STRING: u32 = u32::from_be_bytes(*b"STR ");
const CSF_STRING_WITH_WAVE: u32 = u32::from_be_bytes(*b"STRW");

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> Option<u32> {
        let v = self.b.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let v = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(v)
    }
}

/// Parses a compiled string file.
///
/// Tags are stored as little-endian integers built from big-endian character
/// constants, so `"CSF "` appears on disk as `" FSC"`. Each label holds one or more
/// strings; only the first is used. String text is UTF-16 with every code unit
/// bit-inverted. A malformed tag ends parsing with what was read so far, as the
/// original does.
pub fn parse_csf(data: &[u8]) -> Result<StringTable, StringsError> {
    let mut r = Reader { b: data, pos: 0 };
    let id = r.u32().ok_or(StringsError::Truncated)?;
    if id != CSF_ID {
        return Err(StringsError::NotCsf);
    }
    let version = r.u32().ok_or(StringsError::Truncated)?;
    let _labels = r.u32().ok_or(StringsError::Truncated)?;
    let _strings = r.u32().ok_or(StringsError::Truncated)?;
    let _skip = r.u32().ok_or(StringsError::Truncated)?;
    let langid = r.u32().ok_or(StringsError::Truncated)? as i32;
    let language_id = if version >= 2 { langid } else { 0 };

    let mut entries = Vec::new();
    'labels: while let Some(tag) = r.u32() {
        if tag != CSF_LABEL {
            break;
        }
        let (Some(count), Some(len)) = (r.u32(), r.u32()) else { break };
        let Some(label) = r.bytes(len as usize) else { break };
        let label = latin1(label);
        let mut text = String::new();
        let mut speech = None;
        for n in 0..count {
            let Some(kind) = r.u32() else { break 'labels };
            if kind != CSF_STRING && kind != CSF_STRING_WITH_WAVE {
                break 'labels;
            }
            let Some(len) = r.u32() else { break 'labels };
            let Some(raw) = r.bytes(len as usize * 2) else { break 'labels };
            if n == 0 {
                let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| !u16::from_le_bytes(*c)).collect();
                // The original stops at the first NUL after inverting.
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                text = strip_spaces(&String::from_utf16_lossy(&units[..end]));
            }
            if kind == CSF_STRING_WITH_WAVE {
                let Some(len) = r.u32() else { break 'labels };
                let Some(wave) = r.bytes(len as usize) else { break 'labels };
                if n == 0 && len > 0 {
                    speech = Some(latin1(wave));
                }
            }
        }
        entries.push(Entry { label, text, speech });
    }
    Ok(StringTable::from_entries(entries, language_id))
}

/// Parses a text string file (`Generals.str`, `map.str`).
///
/// Format: a label line, then a `"quoted string"` (which may span lines and may be
/// followed by `= speechfile`), then `END`. Lines starting with `//` are comments.
pub fn parse_str(data: &[u8]) -> StringTable {
    let mut pos = 0usize;
    let mut entries = Vec::new();
    while let Some(line) = read_line(data, &mut pos) {
        let label = trim(&line);
        if label.is_empty() || label.starts_with(b"//") {
            continue;
        }
        let mut entry = Entry { label: latin1(label), text: String::new(), speech: None };
        let mut got_string = false;
        loop {
            let Some(line) = read_line(data, &mut pos) else {
                entries.push(entry);
                return StringTable::from_entries(entries, 0);
            };
            let l = trim(&line);
            if l.first() == Some(&b'"') {
                let mut first = l[1..].to_vec();
                first.push(b'\n');
                let (raw, speech) = read_to_end_of_quote(&first, data, &mut pos);
                if !got_string {
                    entry.text = strip_spaces(&translate_copy(&raw));
                    entry.speech = (!speech.is_empty()).then_some(speech);
                    got_string = true;
                }
            } else if l.eq_ignore_ascii_case(b"END") {
                break;
            }
        }
        entries.push(entry);
    }
    StringTable::from_entries(entries, 0)
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// `GameTextManager::readLine`: bytes up to and excluding `\n`; `None` at EOF.
fn read_line(data: &[u8], pos: &mut usize) -> Option<Vec<u8>> {
    if *pos >= data.len() {
        return None;
    }
    let start = *pos;
    while *pos < data.len() && data[*pos] != b'\n' {
        *pos += 1;
    }
    let line = data[start..*pos].to_vec();
    if *pos < data.len() {
        *pos += 1;
    }
    Some(line)
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `removeLeadingAndTrailing`.
fn trim(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|&c| !is_space(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|&c| !is_space(c)).map_or(start, |e| e + 1);
    &b[start..end.max(start)]
}

/// `readToEndOfQuote`: reads until an unescaped `"`, turning line breaks and other
/// whitespace into spaces, then reads the optional speech file name from the rest of
/// that line. A name ending in a digit gets `e` appended, as the original does.
fn read_to_end_of_quote(first: &[u8], data: &[u8], pos: &mut usize) -> (Vec<u8>, String) {
    let mut first = first.iter().copied();
    let mut next = |pos: &mut usize| -> Option<u8> {
        first.next().or_else(|| {
            let c = data.get(*pos).copied();
            *pos += 1;
            c
        })
    };
    let mut out = Vec::new();
    let mut slash = false;
    loop {
        let Some(mut ch) = next(pos) else { return (out, String::new()) };
        if ch == b'\n' {
            slash = false;
            ch = b' ';
        } else if ch == b'\\' {
            slash = !slash;
        } else if ch == b'"' && !slash {
            break;
        } else {
            slash = false;
        }
        if is_space(ch) {
            ch = b' ';
        }
        out.push(ch);
    }
    // Speech file: skip spaces and '=', then [A-Za-z0-9_]+, ignore the rest of the line.
    let mut wave = String::new();
    let mut state = 0;
    while let Some(ch) = next(pos) {
        if ch == b'\n' {
            break;
        }
        if state == 0 {
            if is_space(ch) || ch == b'=' {
                continue;
            }
            state = 1;
        }
        if state == 1 {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                wave.push(ch as char);
                continue;
            }
            state = 2;
        }
    }
    if wave.ends_with(|c: char| c.is_ascii_digit()) {
        wave.push('e');
    }
    (out, wave)
}

/// `translateCopy`: backslash escapes `\\ \' \" \? \t \n`; any other escaped
/// character is kept as itself.
fn translate_copy(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut slash = false;
    for &c in raw {
        if slash {
            slash = false;
            out.push(match c {
                b't' => '\t',
                b'n' => '\n',
                other => other as char,
            });
        } else if c == b'\\' {
            slash = true;
        } else {
            out.push(c as char);
        }
    }
    out
}

/// `stripSpaces`: drops leading spaces, collapses runs of spaces, and removes spaces
/// before and after line breaks and tabs.
fn strip_spaces(s: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    let mut last: char = '\0';
    let mut skip_all = true;
    for ch in s.chars() {
        if ch == ' ' && (last == ' ' || skip_all) {
            continue;
        }
        if ch == '\n' || ch == '\t' {
            if last == ' ' {
                out.pop();
            }
            skip_all = true;
            out.push(ch);
            last = ch;
            continue;
        }
        out.push(ch);
        last = ch;
        skip_all = false;
    }
    if last == ' ' {
        out.pop();
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csf(entries: &[(&str, &str, Option<&str>)]) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [CSF_ID, 3, entries.len() as u32, entries.len() as u32, 0, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for (label, text, wave) in entries {
            b.extend_from_slice(&CSF_LABEL.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&(label.len() as u32).to_le_bytes());
            b.extend_from_slice(label.as_bytes());
            let tag = if wave.is_some() { CSF_STRING_WITH_WAVE } else { CSF_STRING };
            b.extend_from_slice(&tag.to_le_bytes());
            let units: Vec<u16> = text.encode_utf16().collect();
            b.extend_from_slice(&(units.len() as u32).to_le_bytes());
            for u in units {
                b.extend_from_slice(&(!u).to_le_bytes());
            }
            if let Some(w) = wave {
                b.extend_from_slice(&(w.len() as u32).to_le_bytes());
                b.extend_from_slice(w.as_bytes());
            }
        }
        b
    }

    #[test]
    fn csf_round_trip() {
        let t = parse_csf(&csf(&[("GUI:Ok", "  OK  ", None), ("NAME:Tank", "Crusader", Some("vtank1"))])).unwrap();
        assert_eq!(t.text("gui:ok"), Some("OK"));
        assert_eq!(t.get("NAME:TANK").unwrap().speech.as_deref(), Some("vtank1"));
        assert_eq!(fetch(&t, None, "Nope"), "MISSING: 'Nope'");
    }

    #[test]
    fn str_format() {
        let src = b"// comment\nGUI:Hello\n\"Hello \\\"world\\\"\\n  next\" = speech1\nEND\n\nMAP:Two\n\"line one\nline two\"\nEND\n";
        let t = parse_str(src);
        assert_eq!(t.len(), 2);
        assert_eq!(t.text("GUI:Hello"), Some("Hello \"world\"\nnext"));
        assert_eq!(t.get("GUI:Hello").unwrap().speech.as_deref(), Some("speech1e"));
        assert_eq!(t.text("map:two"), Some("line one line two"));
    }

    #[test]
    fn space_stripping() {
        assert_eq!(strip_spaces("  a   b \n  c  "), "a b\nc");
    }
}
