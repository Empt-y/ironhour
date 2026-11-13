//! Team (house) colour applied to textures named `ZHC...`.
//!
//! Ported from `W3DDevice/GameClient/W3DAssetManager.cpp`
//! (`Recolor_Texture_One_Time`, `Remap_Palette`, `remapAlphaTexture32Bit`,
//! `remapPalette32Bit`) and `WW3D2/colorspace.h` (`RGB_To_HSV`, `HSV_To_RGB`).
//!
//! - `ZHCA...`: every pixel whose alpha is below 255 takes the house colour's hue and
//!   has its saturation scaled by the house colour's saturation (`DO_HUE_SHIFT` is
//!   defined); the alpha channel is then forced opaque.
//! - `ZHCD...`: the first 16 pixels of the top row are a palette, replaced by the
//!   house colour at 16 brightness steps.
//! - Other `ZHC` names are left unchanged.
//!
//! The recoloured top level becomes a new texture whose mipmaps are regenerated.

use ih_formats::texture::Image;

/// `houseColorScale`.
const SCALE: [u16; 16] = [255, 239, 223, 211, 195, 174, 167, 151, 135, 123, 107, 91, 79, 63, 47, 35];

pub fn applies(texture_name: &str) -> bool {
    texture_name.len() >= 4 && texture_name[..3].eq_ignore_ascii_case("ZHC")
}

/// Returns the recoloured image, or `None` when this name is not recoloured.
pub fn recolor(texture_name: &str, img: &Image, color: u32) -> Option<Image> {
    if !applies(texture_name) || texture_name.starts_with('!') {
        return None;
    }
    let mode = texture_name.as_bytes()[3].to_ascii_uppercase();
    let mut px = img.levels[0].clone();
    match mode {
        b'A' => recolor_alpha(&mut px, color),
        b'D' => recolor_palette(&mut px, color),
        _ => return None,
    }
    Some(Image { width: img.width, height: img.height, levels: vec![px], has_alpha: mode != b'A' && img.has_alpha })
}

fn recolor_alpha(px: &mut [u8], color: u32) {
    let house =
        [((color >> 16) & 0xff) as f32 / 255.0, ((color >> 8) & 0xff) as f32 / 255.0, (color & 0xff) as f32 / 255.0];
    let house_hsv = rgb_to_hsv(house);
    for p in px.as_chunks_mut::<4>().0.iter_mut() {
        if p[3] != 255 {
            let mut hsv = rgb_to_hsv([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]);
            hsv[0] = house_hsv[0];
            hsv[1] *= house_hsv[1];
            let rgb = hsv_to_rgb(hsv);
            // REAL_TO_INT truncates.
            p[0] = (rgb[0] * 255.0) as u8;
            p[1] = (rgb[1] * 255.0) as u8;
            p[2] = (rgb[2] * 255.0) as u8;
        }
        p[3] = 255;
    }
}

fn recolor_palette(px: &mut [u8], color: u32) {
    let c = [((color >> 16) & 0xff) as f32, ((color >> 8) & 0xff) as f32, (color & 0xff) as f32];
    for (i, s) in SCALE.iter().enumerate() {
        let Some(p) = px.get_mut(i * 4..i * 4 + 4) else { break };
        for k in 0..3 {
            // scale * component / 255 / 255, converted back to 0..=255.
            p[k] = ((*s as f32 * c[k] / 255.0 / 255.0) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        p[3] = 255;
    }
}

/// `RGB_To_HSV`: hue in degrees (-1 when grey), saturation and value 0..=1.
fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let s = if max != 0.0 { (max - min) / max } else { 0.0 };
    if s == 0.0 {
        return [-1.0, s, max];
    }
    let d = max - min;
    let mut h = if rgb[0] == max {
        (rgb[1] - rgb[2]) / d
    } else if rgb[1] == max {
        2.0 + (rgb[2] - rgb[0]) / d
    } else {
        4.0 + (rgb[0] - rgb[1]) / d
    };
    h *= 60.0;
    if h < 0.0 {
        h += 360.0;
    }
    [h, s, max]
}

/// `HSV_To_RGB`.
fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let [mut h, s, v] = hsv;
    if s == 0.0 {
        return [v, v, v];
    }
    if h == 360.0 {
        h = 0.0;
    }
    h /= 60.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        5 => [v, p, q],
        // Grey pixels keep hue -1 when the house colour is grey: floor gives -1 and
        // the original's switch has no case for it, leaving rgb unset (black).
        _ => [0.0, 0.0, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trip() {
        for c in [[1.0, 0.0, 0.0], [0.2, 0.4, 0.8], [0.5, 0.5, 0.1]] {
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-5, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn alpha_mode_takes_house_hue() {
        // A grey pixel marked for house colour becomes the house hue at its own value.
        let img = Image { width: 1, height: 1, levels: vec![vec![128, 128, 128, 0]], has_alpha: true };
        let out = recolor("ZHCA_Test.tga", &img, 0x0000ff).unwrap();
        let p = &out.levels[0];
        assert_eq!(p[3], 255);
        // Saturation of a grey pixel is 0, so it stays grey: only coloured pixels shift.
        assert_eq!((p[0], p[1], p[2]), (128, 128, 128));
        let img = Image { width: 1, height: 1, levels: vec![vec![200, 100, 100, 0]], has_alpha: true };
        let p = recolor("zhca_x.tga", &img, 0x0000ff).unwrap().levels[0].clone();
        assert!(p[2] > p[0] && p[2] > p[1], "{p:?}");
    }
}
