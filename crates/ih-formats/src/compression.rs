//! Compressed file wrappers.
//!
//! Ported from `Libraries/Source/Compression/CompressionManager.cpp` and
//! `EAC/refdecode.cpp`. A compressed file starts with a four-byte tag and the
//! uncompressed size (u32, little-endian), then the compressed stream.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecompressError {
    #[error("{0} compression is not supported")]
    Unsupported(&'static str),
    #[error("compressed stream is truncated or corrupt")]
    Corrupt,
}

/// The compression scheme a buffer's tag names (`getCompressionType`).
pub fn compression_type(data: &[u8]) -> Option<&'static str> {
    if data.len() < 8 {
        return None;
    }
    Some(match &data[..4] {
        b"NOX\0" => "NOX LZH",
        b"ZL1\0" | b"ZL2\0" | b"ZL3\0" | b"ZL4\0" | b"ZL5\0" | b"ZL6\0" | b"ZL7\0" | b"ZL8\0" | b"ZL9\0" => "zlib",
        b"EAB\0" => "EA btree",
        b"EAH\0" => "EA huffman",
        b"EAR\0" => "RefPack",
        _ => return None,
    })
}

/// Returns the data uncompressed: borrowed when it was not compressed.
pub fn decompress(data: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, DecompressError> {
    match compression_type(data) {
        None => Ok(std::borrow::Cow::Borrowed(data)),
        Some("RefPack") => {
            let size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
            refpack_decode(&data[8..], size).map(std::borrow::Cow::Owned)
        }
        Some("zlib") => {
            // z_uncompress: a zlib stream (with header) after the 8-byte wrapper.
            miniz_oxide::inflate::decompress_to_vec_zlib(&data[8..])
                .map(std::borrow::Cow::Owned)
                .map_err(|_| DecompressError::Corrupt)
        }
        Some(other) => Err(DecompressError::Unsupported(other)),
    }
}

/// `REF_decode`. `expected` is the size from the wrapper header; the stream also
/// stores it, and the stream's value decides the output length.
pub fn refpack_decode(src: &[u8], expected: usize) -> Result<Vec<u8>, DecompressError> {
    let mut s = 0usize;
    let byte = |s: &mut usize| -> Result<u8, DecompressError> {
        let b = *src.get(*s).ok_or(DecompressError::Corrupt)?;
        *s += 1;
        Ok(b)
    };
    let ty = (byte(&mut s)? as u32) << 8 | byte(&mut s)? as u32;
    let size_bytes = if ty & 0x8000 != 0 { 4 } else { 3 };
    if ty & 0x100 != 0 {
        s += size_bytes;
    }
    let mut ulen = 0usize;
    for _ in 0..size_bytes {
        ulen = ulen << 8 | byte(&mut s)? as usize;
    }
    let mut d: Vec<u8> = Vec::with_capacity(ulen.max(expected));

    fn literal(d: &mut Vec<u8>, src: &[u8], s: &mut usize, n: usize) -> Result<(), DecompressError> {
        let bytes = src.get(*s..*s + n).ok_or(DecompressError::Corrupt)?;
        d.extend_from_slice(bytes);
        *s += n;
        Ok(())
    }
    fn copy(d: &mut Vec<u8>, back: usize, n: usize) -> Result<(), DecompressError> {
        // ref = d - 1 - offset; byte by byte so overlapping runs repeat.
        let start = d.len().checked_sub(back + 1).ok_or(DecompressError::Corrupt)?;
        for i in 0..n {
            let b = d[start + i];
            d.push(b);
        }
        Ok(())
    }

    loop {
        let first = byte(&mut s)? as usize;
        if first & 0x80 == 0 {
            // short form
            let second = byte(&mut s)? as usize;
            literal(&mut d, src, &mut s, first & 3)?;
            let back = ((first & 0x60) << 3) + second;
            copy(&mut d, back, ((first & 0x1c) >> 2) + 3)?;
        } else if first & 0x40 == 0 {
            // int form
            let second = byte(&mut s)? as usize;
            let third = byte(&mut s)? as usize;
            literal(&mut d, src, &mut s, second >> 6)?;
            let back = ((second & 0x3f) << 8) + third;
            copy(&mut d, back, (first & 0x3f) + 4)?;
        } else if first & 0x20 == 0 {
            // very int form
            let second = byte(&mut s)? as usize;
            let third = byte(&mut s)? as usize;
            let fourth = byte(&mut s)? as usize;
            literal(&mut d, src, &mut s, first & 3)?;
            let back = (((first & 0x10) >> 4) << 16) + (second << 8) + third;
            copy(&mut d, back, (((first & 0x0c) >> 2) << 8) + fourth + 5)?;
        } else {
            let run = ((first & 0x1f) << 2) + 4;
            if run <= 112 {
                literal(&mut d, src, &mut s, run)?;
            } else {
                // eof, with 0..3 trailing literals
                literal(&mut d, src, &mut s, first & 3)?;
                break;
            }
        }
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refpack_literals_and_back_reference() {
        // header 0x10FB, size 8; literal block of 4 "abcd"; short form copying "abcd"
        // (offset 3 => back 4 bytes) with 0 literals and length 4; eof.
        let mut s = vec![0x10, 0xFB, 0, 0, 8];
        s.push(0xE0); // literal, run ((0)<<2)+4 = 4
        s.extend_from_slice(b"abcd");
        // short: first = (len-3)<<2 | offset_hi<<5 | literals; len 4 => 1<<2
        s.push(0x04);
        s.push(3);
        s.push(0xFC); // eof, 0 literals
        let out = refpack_decode(&s, 8).unwrap();
        assert_eq!(out, b"abcdabcd");
    }

    #[test]
    fn overlapping_copy_repeats() {
        let mut s = vec![0x10, 0xFB, 0, 0, 6];
        s.push(0xE0);
        s.extend_from_slice(b"xyz!");
        // copy 2 bytes from 1 back: "!!"? back=0 => repeats last byte
        s.push(0x00); // len 3, 0 literals
        s.push(0);
        s.push(0xFC);
        assert_eq!(refpack_decode(&s, 7).unwrap(), b"xyz!!!!");
    }
}
