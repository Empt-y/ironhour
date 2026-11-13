//! Texture images: Targa (`.tga`) and DirectDraw Surface (`.dds`), decoded to RGBA8.
//!
//! The game loads textures through `WW3D2/textureloader.cpp`: for a name ending in
//! `.tga` it first tries the `.dds` file of the same name (`Begin_Compressed_Load`,
//! `ddsfile.cpp`) and falls back to the Targa (`WWLib/TARGA.CPP`). DDS files are
//! DXT1/DXT3/DXT5 or uncompressed; Targas are true-colour, grey or colour-mapped,
//! optionally RLE-compressed.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TextureError {
    #[error("unsupported texture: {0}")]
    Unsupported(String),
    #[error("truncated texture data")]
    Truncated,
}

type R<T> = Result<T, TextureError>;

/// An RGBA8 image in the row order WW3D uploads (see [`decode_tga`] and
/// [`decode_dds`]), with its mip chain (level 0 first). Mips are only present when the
/// file stores them (DDS).
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
    /// The source stores an alpha channel that is not entirely opaque.
    pub has_alpha: bool,
}

impl Image {
    pub fn rgba(&self) -> &[u8] {
        &self.levels[0]
    }
}

/// Decodes by content: `DDS ` magic means DDS, anything else is tried as Targa.
pub fn decode(data: &[u8]) -> R<Image> {
    if data.starts_with(b"DDS ") { decode_dds(data) } else { decode_tga(data) }
}

// ---------------------------------------------------------------------------
// Targa

/// Decodes a Targa with WW3D's row order: memory row 0 is the bottom of the image.
pub fn decode_tga(d: &[u8]) -> R<Image> {
    if d.len() < 18 {
        return Err(TextureError::Truncated);
    }
    let id_len = d[0] as usize;
    let cmap_type = d[1];
    let image_type = d[2];
    let cmap_first = u16::from_le_bytes([d[3], d[4]]) as usize;
    let cmap_len = u16::from_le_bytes([d[5], d[6]]) as usize;
    let cmap_bits = d[7];
    let width = u16::from_le_bytes([d[12], d[13]]) as u32;
    let height = u16::from_le_bytes([d[14], d[15]]) as u32;
    let bpp = d[16];
    let desc = d[17];
    let top_down = desc & 0x20 != 0;
    let right_to_left = desc & 0x10 != 0;

    let mut pos = 18 + id_len;
    let palette: Vec<[u8; 4]> = if cmap_type == 1 {
        let entry = (cmap_bits as usize).div_ceil(8);
        let bytes = d.get(pos..pos + cmap_len * entry).ok_or(TextureError::Truncated)?;
        pos += cmap_len * entry;
        bytes.chunks_exact(entry).map(|p| pixel(p, cmap_bits)).collect::<R<_>>()?
    } else {
        Vec::new()
    };

    let rle = image_type >= 9;
    let kind = image_type & 7;
    if !matches!(kind, 1..=3) {
        return Err(TextureError::Unsupported(format!("TGA image type {image_type}")));
    }
    let bytes_pp = (bpp as usize).div_ceil(8);
    let count = width as usize * height as usize;
    let mut raw: Vec<u8> = Vec::with_capacity(count * bytes_pp);
    if rle {
        while raw.len() < count * bytes_pp {
            let h = *d.get(pos).ok_or(TextureError::Truncated)?;
            pos += 1;
            let n = (h & 0x7F) as usize + 1;
            if h & 0x80 != 0 {
                let px = d.get(pos..pos + bytes_pp).ok_or(TextureError::Truncated)?;
                pos += bytes_pp;
                for _ in 0..n {
                    raw.extend_from_slice(px);
                }
            } else {
                let px = d.get(pos..pos + n * bytes_pp).ok_or(TextureError::Truncated)?;
                pos += n * bytes_pp;
                raw.extend_from_slice(px);
            }
        }
        raw.truncate(count * bytes_pp);
    } else {
        raw.extend_from_slice(d.get(pos..pos + count * bytes_pp).ok_or(TextureError::Truncated)?);
    }

    let mut out = vec![0u8; count * 4];
    let mut has_alpha = false;
    for (i, px) in raw.chunks_exact(bytes_pp).enumerate() {
        let c = match kind {
            1 => {
                let idx = if bytes_pp == 1 { px[0] as usize } else { u16::from_le_bytes([px[0], px[1]]) as usize };
                *palette.get(idx.wrapping_sub(cmap_first)).ok_or(TextureError::Truncated)?
            }
            3 => [px[0], px[0], px[0], if bpp == 16 { px[1] } else { 255 }],
            _ => pixel(px, bpp)?,
        };
        has_alpha |= c[3] != 255;
        // WWLib's Targa::Load arranges every image with its origin at the bottom-left:
        // memory row 0 is the image's bottom row. Bottom-up files (the usual kind) keep
        // their stored order; top-down files are flipped. Textures are uploaded in that
        // order, so v = 0 samples the bottom row.
        let (x, y) = (i % width as usize, i / width as usize);
        let x = if right_to_left { width as usize - 1 - x } else { x };
        let y = if top_down { height as usize - 1 - y } else { y };
        let o = (y * width as usize + x) * 4;
        out[o..o + 4].copy_from_slice(&c);
    }
    Ok(Image { width, height, levels: vec![out], has_alpha })
}

/// A BGR(A) Targa pixel of 15/16/24/32 bits to RGBA.
fn pixel(p: &[u8], bits: u8) -> R<[u8; 4]> {
    Ok(match bits {
        15 | 16 => {
            let v = u16::from_le_bytes([p[0], p[1]]);
            let x5 = |s: u16| ((((v >> s) & 31) * 255 + 15) / 31) as u8;
            let a = if bits == 16 && v & 0x8000 == 0 { 0 } else { 255 };
            [x5(10), x5(5), x5(0), a]
        }
        24 => [p[2], p[1], p[0], 255],
        32 => [p[2], p[1], p[0], p[3]],
        8 => [p[0], p[0], p[0], 255],
        b => return Err(TextureError::Unsupported(format!("{b}-bit Targa pixels"))),
    })
}

// ---------------------------------------------------------------------------
// DDS

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x2_0000;

/// Decodes a DDS in stored row order (row 0 first), as Direct3D uploads it.
pub fn decode_dds(d: &[u8]) -> R<Image> {
    if d.len() < 128 {
        return Err(TextureError::Truncated);
    }
    let u = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let height = u(12);
    let width = u(16);
    let mip_count = u(28).max(1);
    let pf_flags = u(80);
    let fourcc = &d[84..88];
    let bit_count = u(88);
    let masks = [u(92), u(96), u(100), u(104)];

    #[derive(Clone, Copy)]
    enum Fmt {
        Dxt1,
        Dxt3,
        Dxt5,
        Masked { bytes: usize },
    }
    let fmt = if pf_flags & DDPF_FOURCC != 0 {
        match fourcc {
            b"DXT1" => Fmt::Dxt1,
            b"DXT2" | b"DXT3" => Fmt::Dxt3,
            b"DXT4" | b"DXT5" => Fmt::Dxt5,
            other => return Err(TextureError::Unsupported(format!("DDS FourCC {}", String::from_utf8_lossy(other)))),
        }
    } else if pf_flags & (DDPF_RGB | DDPF_LUMINANCE) != 0 && matches!(bit_count, 8 | 16 | 24 | 32) {
        Fmt::Masked { bytes: bit_count as usize / 8 }
    } else {
        return Err(TextureError::Unsupported(format!("DDS pixel format flags {pf_flags:#x}")));
    };
    let alpha_mask = if pf_flags & DDPF_ALPHAPIXELS != 0 { masks[3] } else { 0 };

    let mut pos = 128;
    let mut levels = Vec::new();
    let mut has_alpha = false;
    let (mut w, mut h) = (width.max(1), height.max(1));
    for _ in 0..mip_count {
        let size = match fmt {
            Fmt::Dxt1 => (w.div_ceil(4) * h.div_ceil(4) * 8) as usize,
            Fmt::Dxt3 | Fmt::Dxt5 => (w.div_ceil(4) * h.div_ceil(4) * 16) as usize,
            Fmt::Masked { bytes } => (w * h) as usize * bytes,
        };
        let Some(src) = d.get(pos..pos + size) else {
            if levels.is_empty() {
                return Err(TextureError::Truncated);
            }
            break; // a short mip chain: keep the levels that are present
        };
        pos += size;
        let rgba = match fmt {
            Fmt::Dxt1 => decode_blocks(src, w, h, 8, |b, out| dxt1_block(b, out, true)),
            Fmt::Dxt3 => decode_blocks(src, w, h, 16, dxt3_block),
            Fmt::Dxt5 => decode_blocks(src, w, h, 16, dxt5_block),
            Fmt::Masked { bytes } => {
                let lum = pf_flags & DDPF_LUMINANCE != 0;
                src.chunks_exact(bytes)
                    .flat_map(|p| {
                        let mut v = 0u32;
                        for (i, b) in p.iter().enumerate() {
                            v |= u32::from(*b) << (8 * i);
                        }
                        let ch = |m: u32| expand(v, m);
                        let r = ch(masks[0]);
                        let (g, b) = if lum { (r, r) } else { (ch(masks[1]), ch(masks[2])) };
                        let a = if alpha_mask != 0 { ch(alpha_mask) } else { 255 };
                        [r, g, b, a]
                    })
                    .collect()
            }
        };
        has_alpha |= rgba.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
        levels.push(rgba);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    Ok(Image { width: width.max(1), height: height.max(1), levels, has_alpha })
}

/// Scales the bits under `mask` to 0..=255.
fn expand(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    (((v & mask) >> shift) * 255 / max) as u8
}

fn decode_blocks(src: &[u8], w: u32, h: u32, block: usize, f: impl Fn(&[u8], &mut [[u8; 4]; 16])) -> Vec<u8> {
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let mut out = vec![0u8; (w * h * 4) as usize];
    let mut px = [[0u8; 4]; 16];
    for by in 0..bh {
        for bx in 0..bw {
            let i = (by * bw + bx) as usize * block;
            f(&src[i..i + block], &mut px);
            for (j, c) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + j as u32 % 4, by * 4 + j as u32 / 4);
                if x < w && y < h {
                    let o = ((y * w + x) * 4) as usize;
                    out[o..o + 4].copy_from_slice(c);
                }
            }
        }
    }
    out
}

fn rgb565(v: u16) -> [u8; 4] {
    let r = ((v >> 11) & 31) as u32;
    let g = ((v >> 5) & 63) as u32;
    let b = (v & 31) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8, 255]
}

/// DXT1 colour block. With `one_bit_alpha`, colour index 3 is transparent black when
/// `c0 <= c1`, as Direct3D decodes DXT1.
fn dxt1_block(b: &[u8], out: &mut [[u8; 4]; 16], one_bit_alpha: bool) {
    let c0v = u16::from_le_bytes([b[0], b[1]]);
    let c1v = u16::from_le_bytes([b[2], b[3]]);
    let (c0, c1) = (rgb565(c0v), rgb565(c1v));
    let mix = |a: u8, b: u8, wa: u32, wb: u32, d: u32| ((a as u32 * wa + b as u32 * wb) / d) as u8;
    let mut pal = [c0, c1, [0; 4], [0; 4]];
    if c0v > c1v || !one_bit_alpha {
        for k in 0..3 {
            pal[2][k] = mix(c0[k], c1[k], 2, 1, 3);
            pal[3][k] = mix(c0[k], c1[k], 1, 2, 3);
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for k in 0..3 {
            pal[2][k] = mix(c0[k], c1[k], 1, 1, 2);
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let idx = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = pal[((idx >> (2 * i)) & 3) as usize];
    }
}

fn dxt3_block(b: &[u8], out: &mut [[u8; 4]; 16]) {
    dxt1_block(&b[8..16], out, false);
    for (i, px) in out.iter_mut().enumerate() {
        let nyb = (b[i / 2] >> (4 * (i % 2))) & 0xF;
        px[3] = nyb * 17;
    }
}

fn dxt5_block(b: &[u8], out: &mut [[u8; 4]; 16]) {
    dxt1_block(&b[8..16], out, false);
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = ((7 - i) as u32 * a0 + i as u32 * a1) / 7;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = ((5 - i) as u32 * a0 + i as u32 * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, v) in b[2..8].iter().enumerate() {
        bits |= u64::from(*v) << (8 * i);
    }
    for (i, px) in out.iter_mut().enumerate() {
        px[3] = pal[((bits >> (3 * i)) & 7) as usize] as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tga_bottom_up_24bit() {
        // 2x2, bottom-up file: the first stored row is the bottom row.
        let mut d = vec![0u8; 18];
        d[2] = 2;
        d[12] = 2;
        d[14] = 2;
        d[16] = 24;
        // bottom row: blue, green; top row: red, white (BGR order)
        d.extend([255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
        let img = decode_tga(&d).unwrap();
        // WW3D order: memory row 0 is the bottom row, as stored.
        assert_eq!(&img.rgba()[0..4], &[0, 0, 255, 255]); // bottom-left blue first
        assert_eq!(&img.rgba()[8..12], &[255, 0, 0, 255]); // then top-left red
        assert!(!img.has_alpha);
    }

    #[test]
    fn tga_rle_32bit() {
        let mut d = vec![0u8; 18];
        d[2] = 10;
        d[12] = 3;
        d[14] = 1;
        d[16] = 32;
        d[17] = 0x28; // top-down, 8 alpha bits
        d.extend([0x82, 10, 20, 30, 128]); // run of 3 identical pixels
        let img = decode_tga(&d).unwrap();
        assert_eq!(img.rgba(), &[30, 20, 10, 128, 30, 20, 10, 128, 30, 20, 10, 128]);
        assert!(img.has_alpha);
    }

    #[test]
    fn dxt1_transparency() {
        let mut out = [[0u8; 4]; 16];
        // c0 <= c1 selects the 3-colour + transparent palette; all indices 3.
        dxt1_block(&[0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], &mut out, true);
        assert_eq!(out[0], [0, 0, 0, 0]);
    }
}
