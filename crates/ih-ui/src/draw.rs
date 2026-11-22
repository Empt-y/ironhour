//! What windows draw, as a backend-free list of quads.
//!
//! Ported from `W3DGameWinDefaultDraw` (`W3DGameWindow.cpp`): a window with the
//! `IMAGE` status draws its state's first image (disabled, highlighted or
//! enabled) stretched over itself plus `IMAGEOFFSET`; otherwise it draws a
//! one-pixel border and a filled inside in its state's colours. A colour of
//! white with zero alpha is `WIN_COLOR_UNDEFINED` and is not drawn. `W3DNoDraw`
//! draws nothing. Mapped images follow `Image.cpp` (`parseImageCoords`,
//! `parseImageStatus`): UVs are the pixel coordinates over the declared texture
//! size, and `ROTATED_90_CLOCKWISE` turns the corners.
//!
//! Not yet: the gadgets' own draw functions (push button end caps, check box
//! and slider parts, list and combo boxes), text, the draw callbacks named by
//! menus (`W3DMainMenuDraw` and others), highlight state and video buffers.
use crate::{WindowManager, status};
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

/// One thing to draw, in screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawCmd {
    /// A mapped image stretched over `rect` (`winDrawImage`).
    Image { texture: String, uv: [[f32; 2]; 4], rect: [i32; 4] },
    /// A filled rectangle (`winFillRect`).
    Fill { rect: [i32; 4], color: [u8; 4] },
}

const UNDEFINED: [u8; 4] = [255, 255, 255, 0];

impl WindowManager {
    /// The draw list for every visible window, back to front.
    pub fn draw_list(&self, images: &BTreeMap<String, MappedImage>) -> Vec<DrawCmd> {
        let mut out = vec![];
        for id in self.draw_order() {
            let w = &self.windows[id];
            if w.script.callback("DRAWCALLBACK").is_some_and(|c| c.eq_ignore_ascii_case("W3DNoDraw")) {
                continue;
            }
            let o = self.screen_position(id);
            let s = w.size;
            let key = if w.status & status::ENABLED == 0 { "DISABLEDDRAWDATA" } else { "ENABLEDDRAWDATA" };
            let Some(d) = w.script.draw_data(key).into_iter().next() else { continue };
            if w.status & status::IMAGE != 0 {
                let offset = w.script.field("IMAGEOFFSET").map_or([0, 0], |f| {
                    let v = f
                        .parts
                        .iter()
                        .flat_map(|p| p.values.iter())
                        .filter_map(|x| x.parse().ok())
                        .collect::<Vec<i32>>();
                    [v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0)]
                });
                let Some(img) = d.image.as_ref().and_then(|n| images.get(&n.to_ascii_lowercase())) else { continue };
                let (x, y) = (o[0] + offset[0], o[1] + offset[1]);
                out.push(DrawCmd::Image {
                    texture: img.texture.clone(),
                    uv: img.uv_corners(),
                    rect: [x, y, x + s[0], y + s[1]],
                });
            } else {
                let [x0, y0, x1, y1] = [o[0], o[1], o[0] + s[0], o[1] + s[1]];
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
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ih_formats::wnd;

    #[test]
    fn images_colours_and_no_draw() {
        let src = br#"WINDOW
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
ENDALLCHILDREN
END
"#;
        let mut m = WindowManager::new([800, 600]);
        m.load(&wnd::parse(src).unwrap());
        let mut images = BTreeMap::new();
        images.insert(
            "backdrop".to_string(),
            MappedImage {
                texture: "bd.tga".into(),
                texture_size: [1024, 512],
                coords: [0, 0, 512, 256],
                rotated: false,
            },
        );
        let list = m.draw_list(&images);
        assert_eq!(list.len(), 2, "{list:?}");
        let DrawCmd::Image { texture, uv, rect } = &list[0] else { panic!() };
        assert_eq!((texture.as_str(), *rect), ("bd.tga", [0, 0, 800, 600]));
        assert_eq!(uv[2], [0.5, 0.5]);
        // The undefined border is skipped; the fill sits inside the border pixel.
        assert_eq!(list[1], DrawCmd::Fill { rect: [21, 21, 99, 59], color: [10, 20, 30, 255] });
    }

    #[test]
    fn rotated_images_turn_their_corners() {
        let m = MappedImage { texture: String::new(), texture_size: [10, 10], coords: [0, 0, 10, 5], rotated: true };
        assert_eq!(m.uv_corners()[0], [0.0, 0.5]);
    }
}
