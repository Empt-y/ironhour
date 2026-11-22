//! Window layout scripts (`.wnd`).
//!
//! Ported from `GameEngine/Source/GameClient/GUI/GameWindowManagerScript.cpp`
//! (`winCreateFromScript`, `parseWindow`, `parseChildWindows`, `parseScreenRect`,
//! `parseFont`, `parseTextColor`, `parseDrawData`, `parseColor`, the layout block
//! and the field table `gameWindowFieldList`) and the bit names in
//! `GameWindowGlobal.cpp` (`WindowStatusNames`, `WindowStyleNames`).
//!
//! A script is a sequence of `KEY = value;` statements and bare structure
//! words (`STARTLAYOUTBLOCK`/`ENDLAYOUTBLOCK`, `WINDOW`, `CHILD`, `END`,
//! `ENDALLCHILDREN`). A window's `SCREENRECT` is in absolute screen pixels at
//! its `CREATIONRESOLUTION`; the window manager scales and parents it.
//! Values are kept as the original's labelled lists (`LABEL: a b, LABEL: c`),
//! with typed accessors for the common fields. Fields the parser does not know
//! are kept raw, as the original skips them to the next semicolon.

/// `WindowStatusNames`, bit 0 first.
pub const STATUS_NAMES: [&str; 26] = [
    "ACTIVE",
    "TOGGLE",
    "DRAGABLE",
    "ENABLED",
    "HIDDEN",
    "ABOVE",
    "BELOW",
    "IMAGE",
    "TABSTOP",
    "NOINPUT",
    "NOFOCUS",
    "DESTROYED",
    "BORDER",
    "SMOOTH_TEXT",
    "ONE_LINE",
    "NO_FLUSH",
    "SEE_THRU",
    "RIGHT_CLICK",
    "WRAP_CENTERED",
    "CHECK_LIKE",
    "HOTKEY_TEXT",
    "USE_OVERLAY_STATES",
    "NOT_READY",
    "FLASHING",
    "ALWAYS_COLOR",
    "ON_MOUSE_DOWN",
];
/// `WindowStyleNames`, bit 0 first.
pub const STYLE_NAMES: [&str; 16] = [
    "PUSHBUTTON",
    "RADIOBUTTON",
    "CHECKBOX",
    "VERTSLIDER",
    "HORZSLIDER",
    "SCROLLLISTBOX",
    "ENTRYFIELD",
    "STATICTEXT",
    "PROGRESSBAR",
    "USER",
    "MOUSETRACK",
    "ANIMATED",
    "TABSTOP",
    "TABCONTROL",
    "TABPANE",
    "COMBOBOX",
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WndError {
    #[error("line {0}: expected '=' after {1}")]
    MissingEquals(usize, String),
    #[error("line {0}: unterminated value for {1}")]
    Unterminated(usize, String),
    #[error("line {0}: a window must start with WINDOWTYPE and SCREENRECT")]
    BadWindowHeader(usize),
    #[error("line {0}: bad SCREENRECT")]
    BadRect(usize),
    #[error("unexpected end of file inside a window")]
    UnexpectedEof,
}

/// `[r, g, b, a]`.
pub type Color = [u8; 4];

/// One `LABEL: values` part of a value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    pub label: String,
    pub values: Vec<String>,
}

/// A `KEY = value;` statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    pub line: usize,
    pub parts: Vec<Part>,
}
impl Field {
    /// The values of the first part with this label.
    pub fn get(&self, label: &str) -> Option<&[String]> {
        self.parts.iter().find(|p| p.label.eq_ignore_ascii_case(label)).map(|p| p.values.as_slice())
    }
    /// Every part with this label, in order (`COLUMNSWIDTH` repeats).
    pub fn all<'a>(&'a self, label: &'a str) -> impl Iterator<Item = &'a [String]> {
        self.parts.iter().filter(move |p| p.label.eq_ignore_ascii_case(label)).map(|p| p.values.as_slice())
    }
    pub fn int(&self, label: &str) -> Option<i32> {
        self.get(label).and_then(|v| v.first()).and_then(|s| s.parse().ok())
    }
    /// The unlabelled value (`NAME = "x"`), or the first value.
    pub fn text(&self) -> Option<&str> {
        self.parts.first().and_then(|p| p.values.first()).map(|s| s.as_str())
    }
}

/// `SCREENRECT`, in pixels at the creation resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenRect {
    pub upper_left: [i32; 2],
    pub bottom_right: [i32; 2],
    pub creation_resolution: [i32; 2],
}

/// `FONT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Font {
    pub name: String,
    pub size: i32,
    pub bold: bool,
}

/// One `WinDrawData` entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrawData {
    /// `None` for `NoImage`.
    pub image: Option<String>,
    pub color: Color,
    pub border: Color,
}

/// Text colours: (colour, border) for enabled, disabled and highlighted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextColors {
    pub enabled: (Color, Color),
    pub disabled: (Color, Color),
    pub hilite: (Color, Color),
}

/// A `WINDOW` and its children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    /// `WINDOWTYPE`: `USER`, `PUSHBUTTON`, `STATICTEXT`, ...
    pub kind: String,
    pub rect: ScreenRect,
    pub line: usize,
    /// Every other statement, in file order.
    pub fields: Vec<Field>,
    pub children: Vec<Window>,
}

fn nonempty_name(s: &str) -> Option<String> {
    (!s.is_empty() && !s.eq_ignore_ascii_case("[None]")).then(|| s.to_string())
}
fn color(v: &[String]) -> Color {
    let mut c = [0u8; 4];
    for (i, s) in v.iter().take(4).enumerate() {
        c[i] = s.parse::<i32>().unwrap_or(0).clamp(0, 255) as u8;
    }
    c
}
fn bits(f: Option<&Field>, names: &[&str]) -> u32 {
    let Some(f) = f else { return 0 };
    f.parts
        .iter()
        .flat_map(|p| p.values.iter())
        .flat_map(|v| v.split('+'))
        .filter_map(|n| names.iter().position(|x| x.eq_ignore_ascii_case(n.trim())))
        .fold(0, |m, i| m | (1 << i))
}

impl Window {
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key.eq_ignore_ascii_case(key))
    }
    /// `NAME`, e.g. `MainMenu.wnd:ButtonSinglePlayer`.
    pub fn name(&self) -> Option<&str> {
        self.field("NAME").and_then(|f| f.text()).filter(|s| !s.is_empty())
    }
    /// `STATUS` bits (`STATUS_NAMES`).
    pub fn status(&self) -> u32 {
        bits(self.field("STATUS"), &STATUS_NAMES)
    }
    /// `STYLE` bits (`STYLE_NAMES`).
    pub fn style(&self) -> u32 {
        bits(self.field("STYLE"), &STYLE_NAMES)
    }
    /// A callback name (`SYSTEMCALLBACK`, `INPUTCALLBACK`, `TOOLTIPCALLBACK`,
    /// `DRAWCALLBACK`); `[None]` is none.
    pub fn callback(&self, key: &str) -> Option<String> {
        self.field(key).and_then(|f| f.text()).and_then(nonempty_name)
    }
    pub fn font(&self) -> Option<Font> {
        let f = self.field("FONT")?;
        Some(Font {
            name: f.get("NAME").and_then(|v| v.first()).cloned().unwrap_or_default(),
            size: f.int("SIZE").unwrap_or(0),
            bold: f.int("BOLD").unwrap_or(0) != 0,
        })
    }
    /// `TEXT`: a string table label.
    pub fn text(&self) -> Option<&str> {
        self.field("TEXT").and_then(|f| f.text()).filter(|s| !s.is_empty())
    }
    pub fn tooltip(&self) -> Option<&str> {
        self.field("TOOLTIPTEXT").and_then(|f| f.text()).filter(|s| !s.is_empty())
    }
    pub fn text_colors(&self) -> Option<TextColors> {
        let f = self.field("TEXTCOLOR")?;
        let pair = |a: &str, b: &str| (f.get(a).map_or([0; 4], color), f.get(b).map_or([0; 4], color));
        Some(TextColors {
            enabled: pair("ENABLED", "ENABLEDBORDER"),
            disabled: pair("DISABLED", "DISABLEDBORDER"),
            hilite: pair("HILITE", "HILITEBORDER"),
        })
    }
    /// A `*DRAWDATA` field: its entries in order (`parseDrawData`).
    pub fn draw_data(&self, key: &str) -> Vec<DrawData> {
        let Some(f) = self.field(key) else { return vec![] };
        let mut out: Vec<DrawData> = vec![];
        for p in &f.parts {
            match p.label.to_ascii_uppercase().as_str() {
                "IMAGE" => out.push(DrawData {
                    image: p.values.first().filter(|n| !n.eq_ignore_ascii_case("NoImage")).cloned(),
                    ..Default::default()
                }),
                "COLOR" => {
                    if let Some(d) = out.last_mut() {
                        d.color = color(&p.values);
                    }
                }
                "BORDERCOLOR" => {
                    if let Some(d) = out.last_mut() {
                        d.border = color(&p.values);
                    }
                }
                _ => {}
            }
        }
        out
    }
    /// This window and every descendant, depth first.
    pub fn walk(&self) -> Vec<&Window> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }
}

/// A parsed layout script.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub version: u32,
    /// `LAYOUTINIT`, `LAYOUTUPDATE`, `LAYOUTSHUTDOWN`.
    pub init: Option<String>,
    pub update: Option<String>,
    pub shutdown: Option<String>,
    pub windows: Vec<Window>,
}
impl Layout {
    pub fn find(&self, name: &str) -> Option<&Window> {
        self.windows.iter().flat_map(|w| w.walk()).find(|w| w.name().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }
    pub fn count(&self) -> usize {
        self.windows.iter().map(|w| w.walk().len()).sum()
    }
}

enum Token {
    Word(String, usize),
    Statement(Field),
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
    line: usize,
}
impl Lexer<'_> {
    fn skip_space(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            if self.s[self.i] == b'\n' {
                self.line += 1;
            }
            self.i += 1;
        }
    }
    fn next(&mut self) -> Result<Option<Token>, WndError> {
        self.skip_space();
        if self.i >= self.s.len() {
            return Ok(None);
        }
        let start = self.i;
        while self.i < self.s.len() && !self.s[self.i].is_ascii_whitespace() && !matches!(self.s[self.i], b'=' | b';') {
            self.i += 1;
        }
        let word = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        let line = self.line;
        if word.is_empty() {
            // A stray separator.
            self.i += 1;
            return self.next();
        }
        if matches!(
            word.as_str(),
            "WINDOW" | "CHILD" | "END" | "ENDALLCHILDREN" | "STARTLAYOUTBLOCK" | "ENDLAYOUTBLOCK"
        ) {
            return Ok(Some(Token::Word(word, line)));
        }
        self.skip_space();
        if self.s.get(self.i) != Some(&b'=') {
            // An unknown word: like the original, skip to the next semicolon
            // (which swallows the statement after stray text such as Contra's
            // `TOOLTIPTEXT = "..."; (China)`).
            while self.i < self.s.len() && self.s[self.i] != b';' {
                if self.s[self.i] == b'\n' {
                    self.line += 1;
                }
                self.i += 1;
            }
            if self.i >= self.s.len() {
                return Err(WndError::MissingEquals(line, word));
            }
            self.i += 1;
            return self.next();
        }
        self.i += 1;
        // readUntilSemicolon, keeping quoted text whole.
        let vstart = self.i;
        let mut quoted = false;
        while self.i < self.s.len() && (quoted || self.s[self.i] != b';') {
            match self.s[self.i] {
                b'"' => quoted = !quoted,
                b'\n' => self.line += 1,
                _ => {}
            }
            self.i += 1;
        }
        if self.i >= self.s.len() {
            return Err(WndError::Unterminated(line, word));
        }
        let raw = String::from_utf8_lossy(&self.s[vstart..self.i]).into_owned();
        self.i += 1;
        Ok(Some(Token::Statement(Field { key: word, line, parts: parse_parts(&raw) })))
    }
}

/// Split a value into `LABEL: values` parts (commas outside quotes separate parts).
fn parse_parts(raw: &str) -> Vec<Part> {
    let mut pieces = vec![];
    let (mut cur, mut quoted) = (String::new(), false);
    for c in raw.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => pieces.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    pieces.push(cur);
    pieces
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let p = p.trim();
            // A label is a leading word followed by ':' (outside quotes).
            let (label, rest) = match p.find(':') {
                Some(i) if !p[..i].contains('"') && !p[..i].contains(' ') => (p[..i].to_string(), &p[i + 1..]),
                _ => (String::new(), p),
            };
            Part { label, values: split_values(rest) }
        })
        .collect()
}
fn split_values(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut v = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                v.push(c);
            }
            out.push(v);
        } else {
            let mut v = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                v.push(c);
                chars.next();
            }
            out.push(v);
        }
    }
    out
}

/// `parseScreenRect` reads tokens split on spaces, commas, colons and `=`, so
/// missing commas (as in the stock LAN lobby) do not matter.
fn parse_rect(f: &Field) -> Option<ScreenRect> {
    let tokens: Vec<String> = f
        .parts
        .iter()
        .flat_map(|p| std::iter::once(p.label.clone()).chain(p.values.iter().cloned()))
        .flat_map(|t| t.split(':').map(str::to_string).collect::<Vec<_>>())
        .filter(|t| !t.is_empty())
        .collect();
    let two = |label: &str| {
        let i = tokens.iter().position(|t| t.eq_ignore_ascii_case(label))?;
        Some([tokens.get(i + 1)?.parse().ok()?, tokens.get(i + 2)?.parse().ok()?])
    };
    Some(ScreenRect {
        upper_left: two("UPPERLEFT")?,
        bottom_right: two("BOTTOMRIGHT")?,
        creation_resolution: two("CREATIONRESOLUTION")?,
    })
}

/// `parseWindow`: `WINDOWTYPE`, `SCREENRECT`, fields, children until `END`.
fn parse_window(lex: &mut Lexer, line: usize) -> Result<Window, WndError> {
    let header = |t: Option<Token>, key: &str| match t {
        Some(Token::Statement(f)) if f.key == key => Some(f),
        _ => None,
    };
    let kind = header(lex.next()?, "WINDOWTYPE").ok_or(WndError::BadWindowHeader(line))?;
    let rect_field = header(lex.next()?, "SCREENRECT").ok_or(WndError::BadWindowHeader(line))?;
    let rect = parse_rect(&rect_field).ok_or(WndError::BadRect(rect_field.line))?;
    let mut w = Window { kind: kind.text().unwrap_or_default().to_string(), rect, line, ..Default::default() };
    loop {
        match lex.next()? {
            None => return Err(WndError::UnexpectedEof),
            Some(Token::Word(word, l)) => match word.as_str() {
                "END" => return Ok(w),
                "CHILD" => {
                    // parseChildWindows: windows until ENDALLCHILDREN (or END).
                    loop {
                        match lex.next()? {
                            None => return Err(WndError::UnexpectedEof),
                            Some(Token::Word(c, cl)) if c == "WINDOW" => w.children.push(parse_window(lex, cl)?),
                            Some(Token::Word(c, _)) if c == "ENDALLCHILDREN" || c == "END" => break,
                            // Default colours between children are ignored here.
                            Some(_) => {}
                        }
                    }
                }
                _ => {
                    let _ = l;
                }
            },
            Some(Token::Statement(f)) => w.fields.push(f),
        }
    }
}

/// `winCreateFromScript`'s parse.
pub fn parse(bytes: &[u8]) -> Result<Layout, WndError> {
    let mut lex = Lexer { s: bytes, i: 0, line: 1 };
    let mut layout = Layout::default();
    while let Some(t) = lex.next()? {
        match t {
            Token::Statement(f) => match f.key.as_str() {
                "FILE_VERSION" => layout.version = f.text().and_then(|v| v.parse().ok()).unwrap_or(0),
                "LAYOUTINIT" => layout.init = f.text().and_then(nonempty_name),
                "LAYOUTUPDATE" => layout.update = f.text().and_then(nonempty_name),
                "LAYOUTSHUTDOWN" => layout.shutdown = f.text().and_then(nonempty_name),
                _ => {}
            },
            Token::Word(w, line) if w == "WINDOW" => layout.windows.push(parse_window(&mut lex, line)?),
            Token::Word(..) => {}
        }
    }
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"FILE_VERSION = 2;
STARTLAYOUTBLOCK
  LAYOUTINIT = MenuInit;
  LAYOUTUPDATE = [None];
ENDLAYOUTBLOCK
WINDOW
  WINDOWTYPE = USER;
  SCREENRECT = UPPERLEFT: 0 0,
               BOTTOMRIGHT: 800 600,
               CREATIONRESOLUTION: 800 600;
  NAME = "Menu.wnd:Parent";
  STATUS = ENABLED+IMAGE;
  STYLE = USER;
  SYSTEMCALLBACK = "MenuSystem";
  INPUTCALLBACK = "[None]";
  FONT = NAME: "Times New Roman", SIZE: 14, BOLD: 1;
  ENABLEDDRAWDATA = IMAGE: Backdrop, COLOR: 0 0 128 255, BORDERCOLOR: 254 254 254 255,
                    IMAGE: NoImage, COLOR: 1 2 3 4, BORDERCOLOR: 5 6 7 8;
  CHILD
  WINDOW
    WINDOWTYPE = PUSHBUTTON;
    SCREENRECT = UPPERLEFT: 10 20,
                 BOTTOMRIGHT: 110 50,
                 CREATIONRESOLUTION: 800 600;
    NAME = "Menu.wnd:ButtonStart";
    STATUS = ENABLED+IMAGE+TABSTOP;
    STYLE = PUSHBUTTON+MOUSETRACK;
    TEXT = "GUI:Start";
    TEXTCOLOR = ENABLED: 255 255 255 255, ENABLEDBORDER: 0 0 0 255,
                DISABLED: 1 1 1 1, DISABLEDBORDER: 2 2 2 2,
                HILITE: 3 3 3 3, HILITEBORDER: 4 4 4 4;
    LISTBOXDATA = LENGTH: 5, COLUMNS: 2, COLUMNSWIDTH: 40, COLUMNSWIDTH: 60;
  END
  ENDALLCHILDREN
END
"#;

    #[test]
    fn parses_a_layout_with_children() {
        let l = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!((l.version, l.init.as_deref(), l.update.as_deref()), (2, Some("MenuInit"), None));
        assert_eq!(l.count(), 2);
        let p = &l.windows[0];
        assert_eq!(p.kind, "USER");
        assert_eq!(p.rect.bottom_right, [800, 600]);
        assert_eq!(p.name(), Some("Menu.wnd:Parent"));
        assert_eq!(p.status(), (1 << 3) | (1 << 7));
        assert_eq!(p.callback("SYSTEMCALLBACK").as_deref(), Some("MenuSystem"));
        assert_eq!(p.callback("INPUTCALLBACK"), None);
        assert_eq!(p.font(), Some(Font { name: "Times New Roman".into(), size: 14, bold: true }));
        let d = p.draw_data("ENABLEDDRAWDATA");
        assert_eq!(d.len(), 2);
        assert_eq!(
            d[0],
            DrawData { image: Some("Backdrop".into()), color: [0, 0, 128, 255], border: [254, 254, 254, 255] }
        );
        assert_eq!(d[1].image, None);
        let b = l.find("menu.wnd:buttonstart").unwrap();
        assert_eq!(b.rect.upper_left, [10, 20]);
        assert_eq!(b.style(), 1 | (1 << 10));
        assert_eq!(b.text(), Some("GUI:Start"));
        assert_eq!(b.text_colors().unwrap().hilite, ([3; 4], [4; 4]));
        let lb = b.field("LISTBOXDATA").unwrap();
        assert_eq!(lb.int("LENGTH"), Some(5));
        assert_eq!(lb.all("COLUMNSWIDTH").count(), 2);
    }

    #[test]
    fn stray_words_skip_to_the_next_semicolon_and_rects_need_no_commas() {
        let src = b"FILE_VERSION = 2;
WINDOW
WINDOWTYPE = USER;
SCREENRECT = UPPERLEFT: 1 2
 BOTTOMRIGHT: 3 4
 CREATIONRESOLUTION: 800 600;
TEXT = \"A\"; (China)
TOOLTIPDELAY = -1;
NAME = \"x\";
END
";
        let l = parse(src).unwrap();
        let w = &l.windows[0];
        assert_eq!(w.rect.bottom_right, [3, 4]);
        assert!(w.field("TOOLTIPDELAY").is_none(), "swallowed, as in the original");
        assert_eq!(w.name(), Some("x"));
        assert_eq!(parse(b"FILE_VERSION 2"), Err(WndError::MissingEquals(1, "FILE_VERSION".into())));
    }
}
