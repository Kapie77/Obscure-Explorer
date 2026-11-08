//! Obscure 1 Xbox `.xbr` texture files: a descriptor table, a name table and the pixel data of
//! several textures, all little-endian. Pixels are swizzled (bit interleaving of x and y).
//! Ported from obscure-texture-tool (Kapie77). Only the first mip level of each texture is read.

use crate::finalexam::swizzle_offset;
use crate::{DecodeError, Texture};

fn u32_at(data: &[u8], off: usize) -> Result<u32, DecodeError> {
    match data.get(off..off + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => Err(DecodeError::Truncated { needed: off + 4, got: data.len() }),
    }
}

fn ascii(bytes: &[u8]) -> String {
    bytes.iter().filter(|b| b.is_ascii()).map(|&b| b as char).collect()
}

fn unswizzle_rgba<const BPP: usize>(
    pixels: &[u8],
    w: usize,
    h: usize,
    convert: impl Fn(&[u8]) -> [u8; 4],
) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let s = swizzle_offset(x, y, w, h) * BPP;
            if s + BPP > pixels.len() {
                continue; // missing data stays transparent
            }
            let rgba = convert(&pixels[s..s + BPP]);
            let d = (y * w + x) * 4;
            out[d..d + 4].copy_from_slice(&rgba);
        }
    }
    out
}

fn decode_r5g6b5(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
    unswizzle_rgba::<2>(pixels, w, h, |p| {
        let v = p[0] as u32 | ((p[1] as u32) << 8);
        [
            (((v >> 11) & 0x1F) * 255 / 31) as u8,
            (((v >> 5) & 0x3F) * 255 / 63) as u8,
            ((v & 0x1F) * 255 / 31) as u8,
            255,
        ]
    })
}

fn decode_a1r5g5b5(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
    unswizzle_rgba::<2>(pixels, w, h, |p| {
        let v = p[0] as u32 | ((p[1] as u32) << 8);
        [
            (((v >> 10) & 0x1F) * 255 / 31) as u8,
            (((v >> 5) & 0x1F) * 255 / 31) as u8,
            ((v & 0x1F) * 255 / 31) as u8,
            if v & 0x8000 != 0 { 255 } else { 0 },
        ]
    })
}

fn decode_a8r8g8b8(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
    // bytes B, G, R, A
    unswizzle_rgba::<4>(pixels, w, h, |p| [p[2], p[1], p[0], p[3]])
}

pub fn parse_xbr(data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    let table_size = u32_at(data, 0)? as usize;
    let file_size = u32_at(data, 4)? as usize;
    let data_offset = u32_at(data, 8)? as usize;
    if table_size == 0 || table_size % 20 != 0 {
        return Err(DecodeError::Invalid("invalid XBR descriptor table".into()));
    }
    let count = table_size / 20;
    if count > 4096 {
        return Err(DecodeError::Invalid(format!("implausible texture count {count}")));
    }

    // descriptors: flag, offset (relative to the pixel data), ?, format word, ?
    let mut entries: Vec<(usize, u32)> = Vec::with_capacity(count);
    for i in 0..count {
        let off = 0x0C + i * 20;
        entries.push((u32_at(data, off + 4)? as usize, u32_at(data, off + 12)?));
    }

    // names: NUL-terminated strings after the descriptors and a block of metadata
    let metadata_len = (count + 2) * 8 + 4;
    let mut p = 0x0C + table_size + metadata_len;
    let mut names: Vec<String> = Vec::new();
    while names.len() < count && p < data.len() {
        let mut end = p;
        while end < data.len() && data[end] != 0 {
            end += 1;
        }
        let name = ascii(&data[p..end]);
        if name == "SYMBOLTABLE" || name.is_empty() {
            break;
        }
        names.push(name);
        p = end + 1;
    }

    let file_end = if file_size == 0 { data.len() } else { file_size.min(data.len()) };
    let mut out = Vec::new();
    let mut first_err: Option<DecodeError> = None;
    for (i, &(rel, fmt)) in entries.iter().enumerate() {
        let start = data_offset.saturating_add(rel);
        let next = if i + 1 >= count { file_end } else { data_offset.saturating_add(entries[i + 1].0) };
        let end = next.min(data.len());
        let name = names.get(i).cloned().unwrap_or_else(|| format!("tex_{i:03}"));
        if start >= end {
            continue;
        }
        let color = (fmt >> 8) & 0xFF;
        let mips = (fmt >> 16) & 0xF;
        let (size_u, size_v) = ((fmt >> 20) & 0xF, (fmt >> 24) & 0xF);
        if size_u > 13 || size_v > 13 {
            if first_err.is_none() {
                first_err = Some(DecodeError::Invalid(format!("implausible size in '{name}'")));
            }
            continue;
        }
        let (w, h) = (1u32 << size_u, 1u32 << size_v);
        let pixels = &data[start..end];
        let decoded = match color {
            0x05 => Ok((decode_r5g6b5(pixels, w as usize, h as usize), "R5G6B5")),
            0x02 => Ok((decode_a1r5g5b5(pixels, w as usize, h as usize), "A1R5G5B5")),
            0x06 => Ok((decode_a8r8g8b8(pixels, w as usize, h as usize), "A8R8G8B8")),
            other => Err(DecodeError::Unsupported(format!("Xbox texture format 0x{other:02X} in '{name}'"))),
        };
        match decoded {
            Ok((rgba, label)) => out.push(Texture {
                name,
                width: w,
                height: h,
                rgba,
                format: format!("Xbox {label}, {mips} mip(s)"),
            }),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if out.is_empty() {
        if let Some(e) = first_err {
            return Err(e);
        }
        return Err(DecodeError::Invalid("no textures found in the XBR file".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 2x2 A8R8G8B8 texture named "tex".
    fn sample(color: u32, pixels: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..4].copy_from_slice(&20u32.to_le_bytes()); // table size
        d[4..8].copy_from_slice(&((128 + pixels.len()) as u32).to_le_bytes());
        d[8..12].copy_from_slice(&128u32.to_le_bytes()); // data offset
        // descriptor at 0x0C: flag, offset 0, ?, format word, ?
        let fmt = (color << 8) | (1 << 16) | (1 << 20) | (1 << 24);
        d[0x0C + 12..0x0C + 16].copy_from_slice(&fmt.to_le_bytes());
        // names after the descriptor (20 bytes) and the metadata ((1 + 2) * 8 + 4 = 28 bytes)
        let p = 0x0C + 20 + 28;
        d[p..p + 4].copy_from_slice(b"tex\0");
        d[p + 4..p + 16].copy_from_slice(b"SYMBOLTABLE\0");
        d.extend_from_slice(pixels);
        d
    }

    #[test]
    fn a8r8g8b8_is_bgra_in_memory() {
        // blue, green, red, white (bytes B,G,R,A)
        let px = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        let t = parse_xbr(&sample(0x06, &px)).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "tex");
        assert_eq!((t[0].width, t[0].height), (2, 2));
        assert_eq!(
            t[0].rgba,
            [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn r5g6b5_and_a1r5g5b5() {
        // pure red in R5G6B5 = 0xF800, in A1R5G5B5 with alpha = 0xFC00
        let t = parse_xbr(&sample(0x05, &[0x00, 0xF8, 0, 0, 0, 0, 0, 0])).unwrap();
        assert_eq!(&t[0].rgba[0..4], &[255, 0, 0, 255]);
        let t = parse_xbr(&sample(0x02, &[0x00, 0xFC, 0, 0, 0, 0, 0, 0])).unwrap();
        assert_eq!(&t[0].rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(t[0].rgba[7], 0); // alpha bit clear
    }

    #[test]
    fn rejects_bad_files() {
        assert!(parse_xbr(&[1, 2, 3]).is_err());
        let mut d = sample(0x06, &[0u8; 16]);
        d[0..4].copy_from_slice(&21u32.to_le_bytes()); // not a multiple of 20
        assert!(parse_xbr(&d).is_err());
        assert!(matches!(
            parse_xbr(&sample(0x7F, &[0u8; 16])),
            Err(DecodeError::Unsupported(_))
        ));
    }
}
