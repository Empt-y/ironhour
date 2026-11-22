//! What windows draw, as a backend-free list of quads.
//!
//! Ported from `W3DGameWinDefaultDraw` (`W3DGameWindow.cpp`), the push button
//! and static text draws (`W3DPushButton.cpp`: `W3DGadgetPushButtonImageDraw`,
//! `...DrawOne`, `...DrawThree`, `drawButtonText`, `W3DGadgetPushButtonDraw`;
//! `W3DStaticText.cpp`), and `Image.cpp` (`parseImageCoords`,
//! `parseImageStatus`).
//!
//! - Default windows: with the `IMAGE` status, the state's first image
//!   stretched over the window plus `IMAGEOFFSET`; otherwise a one-pixel border
//!   and a filled inside in the state's colours. White with zero alpha is
//!   `WIN_COLOR_UNDEFINED` and is not drawn. `W3DNoDraw` draws nothing.
//! - Push buttons: three images (left 0, tiled middle 5, right 6; 1, 3 and 4
//!   when selected) when the middle one exists, else one image, else colours;
//!   then their text centred in the state's text colours.
//! - Static text: colours or image, then text, vertically centred and either
//!   centred (`CENTERED`) or 7 pixels in, wrapped 10 pixels short of the width.
//! - Check boxes and radio buttons: background, box (checked or not), text.
//!   Sliders: track and thumb at the value. List and combo boxes: their rows
//!   (combo boxes the chosen one, and the drop-down when open). Entry fields:
//!   the typed text.
//!
//! The state is disabled, highlighted or enabled, taken from the
//! `DISABLED`/`HILITE`/`ENABLEDDRAWDATA` lists.
//!
//! Not yet: the exact layouts of the list, combo, slider and entry draws (scroll
//! bars, column widths, cursor, the original's row heights from the font),
//! progress bars and tab controls, menu draw callbacks such as
//! `W3DMainMenuDraw`, button overlays and clocks, video buffers and greyed images.
use crate::text::{FontSpec, Fonts};
use crate::{WindowManager, state, status};
use std::collections::BTreeMap;

/// `Image` (a `MappedImage` definition).
#[derive(Clone, Debug, PartialEq)]
pub struct MappedImage {
    pub texture: String,
    pub texture_size: [u32; 2],
    /// Left, top, right, bottom in texture pixels.
    pub coords: [i32; 4],
    pub rotated: bool,
}
impl MappedImage {
    /// UVs of the top-left, top-right, bottom-right and bottom-left corners.
    pub fn uv_corners(&self) -> [[f32; 2]; 4] {
        let (w, h) = (self.texture_size[0].max(1) as f32, self.texture_size[1].max(1) as f32);
        let [l, t, r, b] = self.coords.map(|c| c as f32);
        let (l, t, r, b) = (l / w, t / h, r / w, b / h);
        let c = [[l, t], [r, t], [r, b], [l, b]];
        // Rotated 90 degrees clockwise in the texture: the screen's top-left
        // corner samples the texture's bottom-left.
        if self.rotated { [c[3], c[0], c[1], c[2]] } else { c }
    }
    /// `getImageWidth`/`getImageHeight` (swapped when rotated).
    pub fn size(&self) -> [i32; 2] {
        let s = [self.coords[2] - self.coords[0], self.coords[3] - self.coords[1]];
        if self.rotated { [s[1], s[0]] } else { s }
    }
}

/// Every `MappedImage` definition, by lower-case name.
pub fn mapped_images(defs: &ih_ini::definitions::Definitions) -> BTreeMap<String, MappedImage> {
    let mut out = BTreeMap::new();
    for name in defs.names("MappedImage") {
        let mut m = MappedImage { texture: String::new(), texture_size: [0, 0], coords: [0; 4], rotated: false };
        for b in defs.all("MappedImage", name) {
            for f in b.fields() {
                let v = f.values();
                let first = v.first().cloned().unwrap_or_default();
                let num = |s: &str| s.parse::<i64>().unwrap_or(0);
                match f.name.as_str() {
                    "Texture" => m.texture = first,
                    "TextureWidth" => m.texture_size[0] = num(&first).max(0) as u32,
                    "TextureHeight" => m.texture_size[1] = num(&first).max(0) as u32,
                    "Coords" => {
                        for t in &v {
                            if let Some((k, x)) = t.split_once(':') {
                                let i = match k.to_ascii_lowercase().as_str() {
                                    "left" => 0,
                                    "top" => 1,
                                    "right" => 2,
                                    "bottom" => 3,
                                    _ => continue,
                                };
                                m.coords[i] = num(x) as i32;
                            }
                        }
                    }
                    "Status" => m.rotated = v.iter().any(|s| s.eq_ignore_ascii_case("ROTATED_90_CLOCKWISE")),
                    _ => {}
                }
            }
        }
        out.insert(name.to_ascii_lowercase(), m);
    }
    out
}

/// `HeaderTemplate` fonts by lower-case name (`HeaderTemplate.ini`).
pub fn header_templates(defs: &ih_ini::definitions::Definitions) -> BTreeMap<String, FontSpec> {
    let mut out = BTreeMap::new();
    for name in defs.names("HeaderTemplate") {
        let mut f = FontSpec { name: "Arial".into(), points: 12, bold: false };
        for b in defs.all("HeaderTemplate", name) {
            for fld in b.fields() {
                let v = fld.values();
                let first = v.first().cloned().unwrap_or_default();
                match fld.name.as_str() {
                    "Font" => f.name = first.trim_matches('"').to_string(),
                    "Point" => f.points = first.parse().unwrap_or(12),
                    "Bold" => f.bold = first.eq_ignore_ascii_case("yes"),
                    _ => {}
                }
            }
        }
        out.insert(name.to_ascii_lowercase(), f);
    }
    out
}

/// What the draw list needs besides the windows.
pub struct Resources {
    pub images: BTreeMap<String, MappedImage>,
    pub headers: BTreeMap<String, FontSpec>,
    /// Text labels (`GUI:Start`) to display text.
    pub strings: Box<dyn Fn(&str) -> String>,
    pub fonts: Fonts,
    /// `DefaultWindowFont` (`Language.ini`).
    pub default_font: FontSpec,
}

/// One thing to draw, in screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawCmd {
    /// A textured quad (a mapped image or glyphs from `text::ATLAS`), tinted.
    Image { texture: String, uv: [[f32; 2]; 4], rect: [f32; 4], color: [u8; 4] },
    /// A filled rectangle (`winFillRect`).
    Fill { rect: [i32; 4], color: [u8; 4] },
}

const UNDEFINED: [u8; 4] = [255, 255, 255, 0];

/// A draw-data state: 0 enabled, 1 disabled, 2 highlighted.
fn state_key(status_bits: u32, state_bits: u32) -> (&'static str, usize) {
    if status_bits & status::ENABLED == 0 {
        ("DISABLEDDRAWDATA", 1)
    } else if state_bits & state::HILITED != 0 {
        ("HILITEDRAWDATA", 2)
    } else {
        ("ENABLEDDRAWDATA", 0)
    }
}

fn image(out: &mut Vec<DrawCmd>, img: &MappedImage, rect: [i32; 4], clip: Option<[i32; 4]>) {
    let mut r = rect.map(|v| v as f32);
    let mut uv = img.uv_corners();
    if let Some(c) = clip {
        // Crop an unrotated image to the clip region (setClipRegion).
        let c = c.map(|v| v as f32);
        if img.rotated || r[0] >= c[2] || r[2] <= c[0] {
            return;
        }
        let (w, du) = (r[2] - r[0], uv[1][0] - uv[0][0]);
        if r[2] > c[2] {
            let cut = (r[2] - c[2]) / w * du;
            uv[1][0] -= cut;
            uv[2][0] -= cut;
            r[2] = c[2];
        }
    }
    out.push(DrawCmd::Image { texture: img.texture.clone(), uv, rect: r, color: [255; 4] });
}

impl WindowManager {
    fn draw_image_named(&self, res: &Resources, name: Option<&String>) -> Option<MappedImage> {
        name.and_then(|n| res.images.get(&n.to_ascii_lowercase())).cloned()
    }

    /// The window's font: its header template's, else its own, else the default.
    fn font(&self, id: usize, res: &Resources) -> FontSpec {
        let w = &self.windows[id].script;
        if let Some(t) = w.field("HEADERTEMPLATE").and_then(|f| f.text())
            && let Some(f) = res.headers.get(&t.to_ascii_lowercase())
        {
            return f.clone();
        }
        w.font()
            .filter(|f| !f.name.is_empty())
            .map_or_else(|| res.default_font.clone(), |f| FontSpec { name: f.name, points: f.size, bold: f.bold })
    }

    /// The window's display text: set text, else its `TEXT` label translated.
    pub fn display_text(&self, id: usize, res: &Resources) -> String {
        let w = &self.windows[id];
        if let Some(t) = &w.text {
            return t.clone();
        }
        w.script.text().map(|l| (res.strings)(l)).unwrap_or_default()
    }

    /// Text in a box: `centered` horizontally, vertically centred, wrapped to `wrap`.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        out: &mut Vec<DrawCmd>,
        res: &mut Resources,
        id: usize,
        rect: [i32; 4],
        centered: bool,
        margin: i32,
        wrap: i32,
    ) {
        self.text_str(out, res, id, rect, centered, margin, wrap, None);
    }

    /// `text` with explicit text (`None`: the window's own).
    #[allow(clippy::too_many_arguments)]
    fn text_str(
        &self,
        out: &mut Vec<DrawCmd>,
        res: &mut Resources,
        id: usize,
        rect: [i32; 4],
        centered: bool,
        margin: i32,
        wrap: i32,
        text: Option<&str>,
    ) {
        let text = text.map_or_else(|| self.display_text(id, res), str::to_string);
        if text.is_empty() {
            return;
        }
        let w = &self.windows[id];
        let Some(colors) = w.script.text_colors() else { return };
        let (color, drop) = match state_key(w.status, w.state).1 {
            1 => colors.disabled,
            2 => colors.hilite,
            _ => colors.enabled,
        };
        if color == UNDEFINED {
            return;
        }
        let font = self.font(id, res);
        let px = res.fonts.pixel_height(font.points, self.display[0]);
        let layout = res.fonts.layout(&font, px, &text, wrap as f32);
        let (bw, bh) = (rect[2] - rect[0], rect[3] - rect[1]);
        let x = if centered { rect[0] + bw / 2 - layout.width as i32 / 2 } else { rect[0] + margin };
        let y = rect[1] + bh / 2 - layout.height() as i32 / 2;
        let drop = (drop != UNDEFINED).then_some(drop);
        for g in res.fonts.draw(&font, px, &layout, [x as f32, y as f32], centered, color, drop) {
            out.push(DrawCmd::Image { texture: crate::text::ATLAS.into(), uv: g.uv, rect: g.rect, color: g.color });
        }
    }

    fn colors(out: &mut Vec<DrawCmd>, rect: [i32; 4], d: &ih_formats::wnd::DrawData) {
        let [x0, y0, x1, y1] = rect;
        // winOpenRect: a one-pixel border.
        if d.border != UNDEFINED {
            for r in [[x0, y0, x1, y0 + 1], [x0, y1 - 1, x1, y1], [x0, y0, x0 + 1, y1], [x1 - 1, y0, x1, y1]] {
                out.push(DrawCmd::Fill { rect: r, color: d.border });
            }
        }
        if d.color != UNDEFINED {
            out.push(DrawCmd::Fill { rect: [x0 + 1, y0 + 1, x1 - 1, y1 - 1], color: d.color });
        }
    }

    fn image_offset(&self, id: usize) -> [i32; 2] {
        self.windows[id].script.field("IMAGEOFFSET").map_or([0, 0], |f| {
            let v = f.parts.iter().flat_map(|p| p.values.iter()).filter_map(|x| x.parse().ok()).collect::<Vec<i32>>();
            [v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0)]
        })
    }

    fn push_button(&self, out: &mut Vec<DrawCmd>, res: &mut Resources, id: usize, rect: [i32; 4]) {
        let w = &self.windows[id];
        let (key, _) = state_key(w.status, w.state);
        let data = w.script.draw_data(key);
        let get = |i: usize| self.draw_image_named(res, data.get(i).and_then(|d| d.image.as_ref()));
        let selected = w.state & state::SELECTED != 0;
        let off = self.image_offset(id);
        let [x0, y0, x1, y1] = rect;
        let size = [x1 - x0, y1 - y0];
        if w.status & status::IMAGE != 0 {
            let enabled_middle =
                self.draw_image_named(res, w.script.draw_data("ENABLEDDRAWDATA").get(5).and_then(|d| d.image.as_ref()));
            if enabled_middle.is_some() {
                // W3DGadgetPushButtonImageDrawThree.
                let (l, m, r) = if selected { (get(1), get(3), get(4)) } else { (get(0), get(5), get(6)) };
                if let (Some(l), Some(m), Some(r)) = (l, m, r) {
                    let (ls, rs) = (l.size(), r.size());
                    let left_end = [x0 + ls[0] + off[0], y0 + size[1] + off[1]];
                    let right_start = [x0 + size[0] - rs[0] + off[0], y0 + off[1]];
                    let center = right_start[0] - left_end[0];
                    if center <= 0 {
                        let mid = x0 + off[0] + size[0] / 2;
                        image(out, &l, [x0 + off[0], y0 + off[1], mid, left_end[1]], None);
                        image(out, &r, [mid, right_start[1], x0 + size[0], right_start[1] + size[1]], None);
                    } else {
                        let mw = m.size()[0].max(1);
                        let mut sx = left_end[0];
                        let (sy, ey) = (y0 + off[1], y0 + off[1] + size[1] + off[1]);
                        for _ in 0..center / mw {
                            image(out, &m, [sx, sy, sx + mw, ey], None);
                            sx += mw;
                        }
                        if right_start[0] - sx > 0 {
                            image(out, &m, [sx, sy, sx + mw, ey], Some([sx, sy, right_start[0], ey]));
                        }
                        image(out, &l, [x0 + off[0], y0 + off[1], left_end[0], left_end[1]], None);
                        image(
                            out,
                            &r,
                            [right_start[0], right_start[1], right_start[0] + rs[0], right_start[1] + size[1]],
                            None,
                        );
                    }
                }
            } else if let Some(img) = if selected { get(1) } else { get(0) } {
                // W3DGadgetPushButtonImageDrawOne.
                image(out, &img, [x0 + off[0], y0 + off[1], x1 + off[0], y1 + off[1]], None);
            }
        } else if let Some(d) = data.first() {
            Self::colors(out, rect, d);
        }
        self.text(out, res, id, rect, true, 0, size[0]);
    }

    fn static_text(&self, out: &mut Vec<DrawCmd>, res: &mut Resources, id: usize, rect: [i32; 4]) {
        let w = &self.windows[id];
        let (key, _) = state_key(w.status, w.state);
        if let Some(d) = w.script.draw_data(key).first() {
            if w.status & status::IMAGE != 0 {
                if let Some(img) = self.draw_image_named(res, d.image.as_ref()) {
                    image(out, &img, rect, None);
                }
            } else {
                Self::colors(out, rect, d);
            }
        }
        let centered = w.script.field("STATICTEXTDATA").and_then(|f| f.int("CENTERED")).unwrap_or(0) != 0;
        self.text(out, res, id, rect, centered, 7, rect[2] - rect[0] - 10);
    }

    fn default_window(&self, out: &mut Vec<DrawCmd>, res: &Resources, id: usize, rect: [i32; 4]) {
        let w = &self.windows[id];
        let (key, _) = state_key(w.status, w.state);
        let Some(d) = w.script.draw_data(key).into_iter().next() else { return };
        if w.status & status::IMAGE != 0 {
            let off = self.image_offset(id);
            if let Some(img) = self.draw_image_named(res, d.image.as_ref()) {
                image(out, &img, [rect[0] + off[0], rect[1] + off[1], rect[2] + off[0], rect[3] + off[1]], None);
            }
        } else {
            Self::colors(out, rect, &d);
        }
    }

    /// Check boxes and radio buttons: background (image 0), the box (1 unchecked,
    /// 2 checked) as a square at the left, then the text after it.
    fn check_box(&self, out: &mut Vec<DrawCmd>, res: &mut Resources, id: usize, rect: [i32; 4]) {
        self.default_window(out, res, id, rect);
        let w = &self.windows[id];
        let (key, _) = state_key(w.status, w.state);
        let data = w.script.draw_data(key);
        let checked = w.state & state::SELECTED != 0;
        let h = rect[3] - rect[1];
        let boxr = [rect[0], rect[1], rect[0] + h, rect[3]];
        if let Some(d) = data.get(if checked { 2 } else { 1 }) {
            match (w.status & status::IMAGE != 0, self.draw_image_named(res, d.image.as_ref())) {
                (true, Some(img)) => image(out, &img, boxr, None),
                _ => Self::colors(out, boxr, d),
            }
        }
        self.text(out, res, id, rect, false, h + 4, 0);
    }

    /// Sliders: the track, then a thumb (`SLIDERTHUMB...DRAWDATA`) at the value.
    fn slider(&self, out: &mut Vec<DrawCmd>, res: &mut Resources, id: usize, rect: [i32; 4]) {
        self.default_window(out, res, id, rect);
        let w = &self.windows[id];
        let g = &w.gadget;
        let key =
            if w.status & status::ENABLED == 0 { "SLIDERTHUMBDISABLEDDRAWDATA" } else { "SLIDERTHUMBENABLEDDRAWDATA" };
        let Some(d) = w.script.draw_data(key).into_iter().next() else { return };
        let span = (g.max - g.min).max(1);
        let f = (g.value - g.min) as f32 / span as f32;
        let thumb = if w.kind == "VERTSLIDER" {
            let side = rect[2] - rect[0];
            let y = rect[3] - side - ((rect[3] - rect[1] - side) as f32 * f) as i32;
            [rect[0], y, rect[2], y + side]
        } else {
            let side = rect[3] - rect[1];
            let x = rect[0] + ((rect[2] - rect[0] - side) as f32 * f) as i32;
            [x, rect[1], x + side, rect[3]]
        };
        match (w.status & status::IMAGE != 0, self.draw_image_named(res, d.image.as_ref())) {
            (true, Some(img)) => image(out, &img, thumb, None),
            _ => Self::colors(out, thumb, &d),
        }
    }

    /// List rows, `ROW_HEIGHT` apart; the selected one framed.
    fn rows(&self, out: &mut Vec<DrawCmd>, res: &mut Resources, id: usize, rect: [i32; 4]) {
        let rh = crate::input::ROW_HEIGHT;
        let rows = self.windows[id].gadget.rows.clone();
        let selected = self.windows[id].gadget.selected;
        for (i, row) in rows.iter().enumerate() {
            let y = rect[1] + rh * i as i32;
            if y + rh > rect[3] {
                break;
            }
            let r = [rect[0], y, rect[2], y + rh];
            if selected == Some(i) {
                Self::colors(
                    out,
                    r,
                    &ih_formats::wnd::DrawData { image: None, color: [255, 255, 255, 0], border: [255, 255, 0, 255] },
                );
            }
            let text = row.join("  ");
            self.text_str(out, res, id, r, false, 4, 0, Some(&text));
        }
    }

    /// The draw list for every visible window, back to front.
    pub fn draw_list(&self, res: &mut Resources) -> Vec<DrawCmd> {
        let mut out = vec![];
        for id in self.draw_order() {
            let w = &self.windows[id];
            if w.script.callback("DRAWCALLBACK").is_some_and(|c| c.eq_ignore_ascii_case("W3DNoDraw")) {
                continue;
            }
            let o = self.screen_position(id);
            let rect = [o[0], o[1], o[0] + w.size[0], o[1] + w.size[1]];
            if let Some(t) = &w.image {
                let r = w.image_rect;
                out.push(DrawCmd::Image {
                    texture: t.clone(),
                    uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    rect: [(o[0] + r[0]) as f32, (o[1] + r[1]) as f32, (o[0] + r[2]) as f32, (o[1] + r[3]) as f32],
                    color: [255; 4],
                });
                continue;
            }
            match w.kind.as_str() {
                "PUSHBUTTON" => self.push_button(&mut out, res, id, rect),
                "STATICTEXT" => self.static_text(&mut out, res, id, rect),
                "CHECKBOX" | "RADIOBUTTON" => self.check_box(&mut out, res, id, rect),
                "ENTRYFIELD" => {
                    self.default_window(&mut out, res, id, rect);
                    let t = self.windows[id].gadget.entry.clone();
                    self.text_str(&mut out, res, id, rect, false, 4, 0, Some(&t));
                }
                "HORZSLIDER" | "VERTSLIDER" => self.slider(&mut out, res, id, rect),
                "SCROLLLISTBOX" => {
                    self.default_window(&mut out, res, id, rect);
                    self.rows(&mut out, res, id, [rect[0], rect[1], rect[2], rect[3]]);
                }
                "COMBOBOX" => {
                    self.default_window(&mut out, res, id, rect);
                    let g = &self.windows[id].gadget;
                    let shown = g.selected.and_then(|i| g.rows.get(i)).and_then(|r| r.first()).cloned();
                    if let Some(t) = shown {
                        self.text_str(&mut out, res, id, rect, false, 4, 0, Some(&t));
                    }
                    if g.open {
                        let h = crate::input::ROW_HEIGHT * g.rows.len() as i32;
                        let list = [rect[0], rect[3], rect[2], rect[3] + h];
                        out.push(DrawCmd::Fill { rect: list, color: [0, 0, 0, 230] });
                        self.rows(&mut out, res, id, list);
                    }
                }
                _ => self.default_window(&mut out, res, id, rect),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ih_formats::wnd;

    fn resources() -> Resources {
        let mut images = BTreeMap::new();
        let img =
            |c: [i32; 4]| MappedImage { texture: "t.tga".into(), texture_size: [256, 256], coords: c, rotated: false };
        images.insert(
            "backdrop".to_string(),
            MappedImage { texture_size: [1024, 512], coords: [0, 0, 512, 256], ..img([0; 4]) },
        );
        images.insert("left".to_string(), img([0, 0, 10, 30]));
        images.insert("mid".to_string(), img([10, 0, 30, 30]));
        images.insert("right".to_string(), img([30, 0, 40, 30]));
        Resources {
            images,
            headers: BTreeMap::new(),
            strings: Box::new(|l| l.to_string()),
            fonts: Fonts::new(Box::new(|_, _| None)),
            default_font: FontSpec { name: "Times New Roman".into(), points: 14, bold: false },
        }
    }

    const SRC: &[u8] = br#"WINDOW
WINDOWTYPE = USER;
SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 400 300, CREATIONRESOLUTION: 400 300;
NAME = "T.wnd:Back";
STATUS = ENABLED+IMAGE;
ENABLEDDRAWDATA = IMAGE: Backdrop, COLOR: 0 0 0 255, BORDERCOLOR: 0 0 0 255;
CHILD
WINDOW
WINDOWTYPE = USER;
SCREENRECT = UPPERLEFT: 10 10, BOTTOMRIGHT: 50 30, CREATIONRESOLUTION: 400 300;
NAME = "T.wnd:Box";
STATUS = ENABLED;
ENABLEDDRAWDATA = IMAGE: NoImage, COLOR: 10 20 30 255, BORDERCOLOR: 255 255 255 0;
END
WINDOW
WINDOWTYPE = USER;
SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 5 5, CREATIONRESOLUTION: 400 300;
NAME = "T.wnd:Nothing";
STATUS = ENABLED;
DRAWCALLBACK = "W3DNoDraw";
ENABLEDDRAWDATA = IMAGE: NoImage, COLOR: 1 1 1 255, BORDERCOLOR: 1 1 1 255;
END
WINDOW
WINDOWTYPE = PUSHBUTTON;
SCREENRECT = UPPERLEFT: 100 100, BOTTOMRIGHT: 175 130, CREATIONRESOLUTION: 400 300;
NAME = "T.wnd:Button";
STATUS = ENABLED+IMAGE;
ENABLEDDRAWDATA = IMAGE: Left, COLOR: 0 0 0 0, BORDERCOLOR: 0 0 0 0,
                  IMAGE: NoImage, COLOR: 0 0 0 0, BORDERCOLOR: 0 0 0 0,
                  IMAGE: NoImage, COLOR: 0 0 0 0, BORDERCOLOR: 0 0 0 0,
                  IMAGE: NoImage, COLOR: 0 0 0 0, BORDERCOLOR: 0 0 0 0,
                  IMAGE: NoImage, COLOR: 0 0 0 0, BORDERCOLOR: 0 0 0 0,
                  IMAGE: Mid, COLOR: 0 0 0 0, BORDERCOLOR: 0 0 0 0,
                  IMAGE: Right, COLOR: 0 0 0 0, BORDERCOLOR: 0 0 0 0;
END
ENDALLCHILDREN
END
"#;

    #[test]
    fn images_colours_no_draw_and_three_part_buttons() {
        let mut m = WindowManager::new([800, 600]);
        m.load(&wnd::parse(SRC).unwrap());
        let list = m.draw_list(&mut resources());
        let DrawCmd::Image { texture, uv, rect, .. } = &list[0] else { panic!() };
        assert_eq!((texture.as_str(), *rect), ("t.tga", [0.0, 0.0, 800.0, 600.0]));
        assert_eq!(uv[2], [0.5, 0.5]);
        // The undefined border is skipped; the fill sits inside the border pixel.
        assert_eq!(list[1], DrawCmd::Fill { rect: [21, 21, 99, 59], color: [10, 20, 30, 255] });
        // Button 200..350 wide 150: left 10, right 10, middle 130 = 6 whole tiles
        // of 20 plus a cropped 10.
        let rects: Vec<[f32; 4]> = list[2..]
            .iter()
            .map(|c| match c {
                DrawCmd::Image { rect, .. } => *rect,
                _ => panic!(),
            })
            .collect();
        assert_eq!(rects.len(), 9);
        assert_eq!(rects[6], [330.0, 200.0, 340.0, 260.0], "cropped middle");
        assert_eq!(rects[7], [200.0, 200.0, 210.0, 260.0], "left");
        assert_eq!(rects[8], [340.0, 200.0, 350.0, 260.0], "right");
    }

    #[test]
    fn rotated_images_turn_their_corners() {
        let m = MappedImage { texture: String::new(), texture_size: [10, 10], coords: [0, 0, 10, 5], rotated: true };
        assert_eq!(m.uv_corners()[0], [0.0, 0.5]);
        assert_eq!(m.size(), [5, 10]);
    }
}
