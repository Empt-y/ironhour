//! Line reader and tokeniser.
//!
//! Ported from `GameEngine/Source/Common/INI/INI.cpp` (`INI::readLine`) and the
//! `strtok`-based token functions.
//!
//! `readLine` reads bytes up to `\n`, at most [`MAX_LINE`] of them. If the limit is
//! hit, the rest of the physical line is read as the *next* line. `;` ends the
//! usable text (it is replaced by NUL, so everything after it is ignored). Bytes
//! 1..=31 become spaces, which turns `\r` and tabs into separators. A NUL byte also
//! ends the usable text.

/// `INI_MAX_CHARS_PER_LINE` in `INI.h`.
pub const MAX_LINE: usize = 1028;

/// Separator sets used by the original (`INI::INI` constructor).
pub const SEPS: &[u8] = b" \n\r\t=";
pub const SEPS_PERCENT: &[u8] = b" \n\r\t=%";
pub const SEPS_COLON: &[u8] = b" \n\r\t=:";
pub const SEPS_QUOTE: &[u8] = b"\"\n=";

/// One logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// 1-based line number as the original counts them.
    pub number: u32,
    /// Usable text: up to the first NUL (comment start), control bytes as spaces.
    /// These are exactly the bytes the original feeds into the INI CRC.
    pub text: Vec<u8>,
}

/// Splits a file into logical lines the way `INI::readLine` does.
pub fn lines(data: &[u8]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut number = 0u32;
    let mut eof = false;
    while !eof {
        let mut buf: Vec<u8> = Vec::new();
        while buf.len() < MAX_LINE {
            let Some(&c) = data.get(pos) else {
                eof = true;
                break;
            };
            pos += 1;
            if c == b'\n' {
                break;
            }
            let c = match c {
                b';' => 0,
                1..=31 => b' ',
                other => other,
            };
            // The original keeps writing past the NUL, but nothing after it is read.
            buf.push(c);
        }
        number += 1;
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        out.push(Line { number, text: buf });
    }
    out
}

/// `strtok`-style tokeniser over one line.
#[derive(Debug, Clone)]
pub struct Tokens<'a> {
    text: &'a [u8],
    pos: usize,
}

impl<'a> Tokens<'a> {
    pub fn new(text: &'a [u8]) -> Self {
        Self { text, pos: 0 }
    }

    /// Next token using `seps`. Like `strtok`, leading separators are skipped and
    /// the one separator after the token is consumed.
    pub fn next_with(&mut self, seps: &[u8]) -> Option<&'a [u8]> {
        let t = self.text;
        while self.pos < t.len() && seps.contains(&t[self.pos]) {
            self.pos += 1;
        }
        if self.pos >= t.len() {
            return None;
        }
        let start = self.pos;
        while self.pos < t.len() && !seps.contains(&t[self.pos]) {
            self.pos += 1;
        }
        let tok = &t[start..self.pos];
        if self.pos < t.len() {
            self.pos += 1; // strtok overwrites the delimiter with NUL and resumes after it
        }
        Some(tok)
    }

    pub fn next_token(&mut self) -> Option<&'a [u8]> {
        self.next_with(SEPS)
    }

    /// Unread remainder (after the consumed delimiter).
    pub fn rest(&self) -> &'a [u8] {
        &self.text[self.pos.min(self.text.len())..]
    }
}

/// Bytes to a `String`, one char per byte (Latin-1), so nothing is lost.
pub fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(data: &[u8]) -> Vec<String> {
        lines(data).into_iter().map(|l| text(&l.text)).collect()
    }

    #[test]
    fn comments_controls_and_eof() {
        assert_eq!(texts(b"Object Foo ; comment\r\n\tSide = America\nEnd"), ["Object Foo ", " Side = America", "End"]);
        // A trailing newline produces one final empty line, as readLine does at EOF.
        assert_eq!(texts(b"End\n"), ["End", ""]);
    }

    #[test]
    fn long_lines_wrap_into_the_next_line() {
        let mut data = vec![b'a'; MAX_LINE + 5];
        data.push(b'\n');
        let ls = lines(&data);
        assert_eq!(ls[0].text.len(), MAX_LINE);
        assert_eq!(ls[1].text, b"aaaaa");
        assert_eq!(ls[1].number, 2);
    }

    #[test]
    fn strtok_semantics() {
        let mut t = Tokens::new(b"  Behavior = AIUpdateInterface ModuleTag_03");
        assert_eq!(t.next_token(), Some(&b"Behavior"[..]));
        assert_eq!(t.rest(), b"= AIUpdateInterface ModuleTag_03");
        assert_eq!(t.next_token(), Some(&b"AIUpdateInterface"[..]));
        assert_eq!(t.next_token(), Some(&b"ModuleTag_03"[..]));
        assert_eq!(t.next_token(), None);
        let mut t = Tokens::new(b"Damage = X:40%");
        t.next_token();
        assert_eq!(t.next_with(SEPS_COLON), Some(&b"X"[..]));
        assert_eq!(t.next_with(SEPS_PERCENT), Some(&b"40"[..]));
    }
}
