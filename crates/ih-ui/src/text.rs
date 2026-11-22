//! Fonts and text: measuring, word wrap and glyph quads.
//!
//! Follows `Render2DSentenceClass` (`render2dsentence.cpp`), `W3DDisplayString`
//! and `GlobalLanguage::adjustFontSize`. A point size becomes a pixel height of
//! `points * 96 / 72` (the original always assumes 96 dots per inch), after
//! window and header-template fonts grow with the display:
//! `1 + (width / 800 - 1) * ResolutionFontAdjustment`, kept between 1 and 2.
//! The "Generals" font is Arial squeezed so its average character is 40% as
//! wide as the font is tall (`CreateFont` with an explicit width). Text with a
//! border colour gets a one-pixel drop shadow down and right. `&` marks a
//! hotkey and is not drawn.
//!
//! Glyphs are rasterised by `fontdue` into one atlas (white, coverage in alpha)
//! that the renderer uploads as the texture [`ATLAS`].
use std::collections::HashMap;

/// The texture name glyph quads refer to.
pub const ATLAS: &str = "__ih_ui_font_atlas";
const ATLAS_SIZE: u32 = 1024;
/// Arial's average character width over its em height (`xAvgCharWidth` 904/2048).
const ARIAL_AVERAGE_WIDTH: f32 = 904.0 / 2048.0;

/// A font as windows name it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontSpec {
    pub name: String,
    pub points: i32,
    pub bold: bool,
}

/// Where TrueType files come from: (family, bold) to file bytes.
pub type FontSource = Box<dyn Fn(&str, bool) -> Option<Vec<u8>>>;

struct Glyph {
    /// Atlas pixels: x, y, width, height.
    rect: [u32; 4],
    xmin: f32,
    ymin: f32,
    advance: f32,
}

/// Loaded fonts, the glyph atlas and its pixels.
pub struct Fonts {
    source: FontSource,
    faces: HashMap<(String, bool), Option<fontdue::Font>>,
    glyphs: HashMap<(String, bool, u32, char), Option<Glyph>>,
    /// RGBA8, `ATLAS_SIZE` square.
    pub atlas: Vec<u8>,
    pub atlas_size: u32,
    /// Set when glyphs were added since the renderer last uploaded the atlas.
    pub dirty: bool,
    cursor: [u32; 2],
    row_height: u32,
    /// `ResolutionFontAdjustment` (`Language.ini`).
    pub resolution_adjustment: f32,
}

/// A laid-out string.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub lines: Vec<String>,
    pub width: f32,
    pub line_height: f32,
}
impl Layout {
    pub fn height(&self) -> f32 {
        self.line_height * self.lines.len() as f32
    }
}

/// A glyph to draw: atlas UVs (top-left, top-right, bottom-right, bottom-left) over a pixel rect.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphQuad {
    pub rect: [f32; 4],
    pub uv: [[f32; 2]; 4],
    pub color: [u8; 4],
}

impl Fonts {
    pub fn new(source: FontSource) -> Self {
        Self {
            source,
            faces: HashMap::new(),
            glyphs: HashMap::new(),
            atlas: vec![0; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
            atlas_size: ATLAS_SIZE,
            dirty: true,
            cursor: [1, 1],
            row_height: 0,
            resolution_adjustment: 0.7,
        }
    }

    /// `adjustFontSize` for a display width, then points to pixels.
    pub fn pixel_height(&self, points: i32, display_width: i32) -> u32 {
        let f = (1.0 + (display_width as f32 / 800.0 - 1.0) * self.resolution_adjustment).clamp(1.0, 2.0);
        let adjusted = (points as f32 * f).floor() as i32;
        // MulDiv(points, 96, 72), rounded.
        ((adjusted * 96 + 36) / 72).max(1) as u32
    }

    /// The face for a family; "Generals" is Arial (squeezed when drawn).
    fn face(&mut self, name: &str, bold: bool) -> Option<&fontdue::Font> {
        let family = if name.eq_ignore_ascii_case("Generals") { "Arial" } else { name };
        let key = (family.to_ascii_lowercase(), bold);
        if !self.faces.contains_key(&key) {
            let font = (self.source)(family, bold)
                .or_else(|| (self.source)(family, false))
                .or_else(|| (self.source)("Arial", bold))
                .and_then(|b| fontdue::Font::from_bytes(b, fontdue::FontSettings::default()).ok());
            self.faces.insert(key.clone(), font);
        }
        self.faces[&key].as_ref()
    }

    fn squeeze(name: &str) -> f32 {
        if name.eq_ignore_ascii_case("Generals") { 0.40 / ARIAL_AVERAGE_WIDTH } else { 1.0 }
    }

    fn glyph(&mut self, name: &str, bold: bool, px: u32, c: char) -> Option<&Glyph> {
        let key = (name.to_ascii_lowercase(), bold, px, c);
        if !self.glyphs.contains_key(&key) {
            let g = self.rasterize(name, bold, px, c);
            self.glyphs.insert(key.clone(), g);
        }
        self.glyphs[&key].as_ref()
    }

    fn rasterize(&mut self, name: &str, bold: bool, px: u32, c: char) -> Option<Glyph> {
        let (metrics, bitmap) = self.face(name, bold)?.rasterize(c, px as f32);
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        if self.cursor[0] + w + 1 >= ATLAS_SIZE {
            self.cursor = [1, self.cursor[1] + self.row_height + 1];
            self.row_height = 0;
        }
        if self.cursor[1] + h + 1 >= ATLAS_SIZE {
            // Full: start again (glyphs already handed out stay valid this frame).
            self.cursor = [1, 1];
            self.row_height = 0;
            self.glyphs.clear();
            self.atlas.fill(0);
        }
        let [x, y] = self.cursor;
        for row in 0..h {
            for col in 0..w {
                let a = bitmap[(row * w + col) as usize];
                let i = (((y + row) * ATLAS_SIZE + x + col) * 4) as usize;
                self.atlas[i..i + 4].copy_from_slice(&[255, 255, 255, a]);
            }
        }
        self.cursor[0] += w + 1;
        self.row_height = self.row_height.max(h);
        self.dirty = true;
        Some(Glyph {
            rect: [x, y, w, h],
            xmin: metrics.xmin as f32,
            ymin: metrics.ymin as f32,
            advance: metrics.advance_width,
        })
    }

    /// `tmHeight`: ascent plus descent.
    pub fn line_height(&mut self, name: &str, bold: bool, px: u32) -> f32 {
        self.face(name, bold)
            .and_then(|f| f.horizontal_line_metrics(px as f32))
            .map_or(px as f32, |m| (m.ascent - m.descent).ceil())
    }
    fn ascent(&mut self, name: &str, bold: bool, px: u32) -> f32 {
        self.face(name, bold).and_then(|f| f.horizontal_line_metrics(px as f32)).map_or(px as f32, |m| m.ascent.round())
    }

    /// The width of one line.
    pub fn width(&mut self, name: &str, bold: bool, px: u32, text: &str) -> f32 {
        let s = Self::squeeze(name);
        text.chars().filter(|&c| c != '&').map(|c| self.glyph(name, bold, px, c).map_or(0.0, |g| g.advance) * s).sum()
    }

    /// Splits text into lines no wider than `wrap` (0: no wrapping), at spaces
    /// and line breaks.
    pub fn layout(&mut self, font: &FontSpec, px: u32, text: &str, wrap: f32) -> Layout {
        let mut lines = vec![];
        for para in text.split('\n') {
            if wrap <= 0.0 {
                lines.push(para.to_string());
                continue;
            }
            let mut line = String::new();
            for word in para.split(' ') {
                let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if !line.is_empty() && self.width(&font.name, font.bold, px, &candidate) > wrap {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_string();
                } else {
                    line = candidate;
                }
            }
            lines.push(line);
        }
        let width = lines.iter().map(|l| self.width(&font.name, font.bold, px, l)).fold(0.0, f32::max);
        let line_height = self.line_height(&font.name, font.bold, px);
        Layout { lines, width, line_height }
    }

    /// Glyph quads for a layout whose top-left is `pos`; `centered` centres
    /// each line within the layout's width. The drop shadow (when given) comes
    /// first, one pixel down and right.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        font: &FontSpec,
        px: u32,
        layout: &Layout,
        pos: [f32; 2],
        centered: bool,
        color: [u8; 4],
        drop: Option<[u8; 4]>,
    ) -> Vec<GlyphQuad> {
        let s = Self::squeeze(&font.name);
        let ascent = self.ascent(&font.name, font.bold, px);
        let inv = 1.0 / ATLAS_SIZE as f32;
        let mut out = vec![];
        for (offset, col) in drop.map(|d| ([1.0, 1.0], d)).into_iter().chain(std::iter::once(([0.0, 0.0], color))) {
            for (i, line) in layout.lines.iter().enumerate() {
                let lw = self.width(&font.name, font.bold, px, line);
                let mut x = pos[0] + offset[0] + if centered { ((layout.width - lw) / 2.0).floor() } else { 0.0 };
                let baseline = pos[1] + offset[1] + ascent + layout.line_height * i as f32;
                for c in line.chars().filter(|&c| c != '&') {
                    let Some(g) = self.glyph(&font.name, font.bold, px, c) else { continue };
                    let [gx, gy, gw, gh] = g.rect;
                    if gw > 0 && gh > 0 {
                        let left = (x + g.xmin * s).round();
                        let top = (baseline - g.ymin - gh as f32).round();
                        let (u0, v0, u1, v1) =
                            (gx as f32 * inv, gy as f32 * inv, (gx + gw) as f32 * inv, (gy + gh) as f32 * inv);
                        out.push(GlyphQuad {
                            rect: [left, top, left + gw as f32 * s, top + gh as f32],
                            uv: [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
                            color: col,
                        });
                    }
                    x += g.advance * s;
                }
            }
        }
        out
    }
}

/// The usual places for a family's TrueType file: the Windows font folder,
/// common Linux and macOS folders, then the game's own `GenArial.ttf` in
/// `fallback_dir` for any family that is missing.
pub fn system_font_source(fallback_dir: Option<std::path::PathBuf>) -> FontSource {
    Box::new(move |family: &str, bold: bool| {
        let files: &[&str] = match (family.to_ascii_lowercase().as_str(), bold) {
            ("arial", false) => &["arial.ttf", "Arial.ttf", "LiberationSans-Regular.ttf"],
            ("arial", true) => &["arialbd.ttf", "Arial Bold.ttf", "LiberationSans-Bold.ttf"],
            ("times new roman", false) => &["times.ttf", "Times New Roman.ttf", "LiberationSerif-Regular.ttf"],
            ("times new roman", true) => &["timesbd.ttf", "Times New Roman Bold.ttf", "LiberationSerif-Bold.ttf"],
            ("courier" | "courier new", false) => &["cour.ttf", "Courier New.ttf", "LiberationMono-Regular.ttf"],
            ("courier" | "courier new", true) => &["courbd.ttf", "Courier New Bold.ttf", "LiberationMono-Bold.ttf"],
            _ => &[],
        };
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let dirs = [
            std::path::PathBuf::from(windir).join("Fonts"),
            "/usr/share/fonts/truetype/msttcorefonts".into(),
            "/usr/share/fonts/truetype/liberation".into(),
            "/Library/Fonts".into(),
            "/System/Library/Fonts/Supplemental".into(),
        ];
        for d in &dirs {
            for f in files {
                if let Ok(b) = std::fs::read(d.join(f)) {
                    return Some(b);
                }
            }
        }
        fallback_dir.as_ref().and_then(|d| std::fs::read(d.join("GenArial.ttf")).ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Option<Fonts> {
        let src = system_font_source(None);
        src("Arial", false)?;
        Some(Fonts::new(src))
    }

    #[test]
    fn sizes_follow_the_resolution_rule() {
        let f = Fonts::new(Box::new(|_, _| None));
        assert_eq!(f.pixel_height(12, 800), 16);
        // 1024 wide: 1 + 0.28 * 0.7 = 1.196, 12 * 1.196 = 14.35 -> 14 points -> 19 px.
        assert_eq!(f.pixel_height(12, 1024), 19);
        assert_eq!(f.pixel_height(12, 4000), 32, "capped at twice");
    }

    #[test]
    fn wraps_and_squeezes() {
        let Some(mut f) = fonts() else { return };
        let spec = FontSpec { name: "Arial".into(), points: 12, bold: false };
        let one = f.layout(&spec, 16, "Skirmish Game Options", 0.0);
        assert_eq!(one.lines.len(), 1);
        let wrapped = f.layout(&spec, 16, "Skirmish Game Options", one.width * 0.6);
        assert!(wrapped.lines.len() >= 2 && wrapped.width <= one.width * 0.6 + 0.5);
        let generals = f.width("Generals", false, 16, "Skirmish");
        let arial = f.width("Arial", false, 16, "Skirmish");
        assert!((generals / arial - 0.40 / ARIAL_AVERAGE_WIDTH).abs() < 0.01);
        assert_eq!(f.width("Arial", false, 16, "&Back"), f.width("Arial", false, 16, "Back"));
        let quads = f.draw(&spec, 16, &one, [10.0, 10.0], false, [255; 4], Some([0, 0, 0, 255]));
        let n = "Skirmish Game Options".chars().filter(|c| *c != ' ').count();
        assert_eq!(quads.len(), 2 * n, "shadow and text");
        assert!(f.dirty);
    }
}
