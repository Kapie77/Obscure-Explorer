//! Obscure PSP `.dic` texture packs (little-endian): palette 4 / 8 bits or RGBA8888 pixels,
//! swizzled in blocks of 16 bytes x 8 rows. Ported from obscure-texture-tool (Kapie77). Read only.

use crate::{DecodeError, Texture};

fn le32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn le16(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// Linear bytes of a swizzled image (`bpp` bits per pixel); missing data stays 0.
fn unswizzle(raw: &[u8], w: usize, h: usize, bpp: usize) -> Vec<u8> {
    let stride = w * bpp / 8;
    let padded_stride = (stride + 15) & !15;
    let row_blocks = padded_stride / 16;
    let mut linear = vec![0u8; stride * h];
    let mut dst = 0usize;
    for y in 0..h {
        for x in 0..stride {
            let block_index = x / 16 + (y / 8) * row_blocks;
            let src = block_index * 16 * 8 + (x % 16) + (y % 8) * 16;
            if src < raw.len() {
                linear[dst] = raw[src];
            }
            dst += 1;
        }
    }
    linear
}

fn palette(data: &[u8], count: usize) -> Vec<[u8; 4]> {
    (0..count)
        .map(|i| match data.get(i * 4..i * 4 + 4) {
            Some(c) => [c[0], c[1], c[2], c[3]],
            None => [0, 0, 0, 0],
        })
        .collect()
}

fn decode_pal4(pixels: &[u8], pal: &[u8], w: usize, h: usize) -> Vec<u8> {
    let linear = unswizzle(pixels, w, h, 4);
    let pal = palette(pal, 16);
    let mut out = vec![0u8; w * h * 4];
    for i in 0..w * h {
        // low nibble first
        let b = linear.get(i / 2).copied().unwrap_or(0);
        let idx = if i % 2 == 0 { b & 0x0F } else { b >> 4 };
        out[i * 4..i * 4 + 4].copy_from_slice(&pal[idx as usize]);
    }
    out
}

fn decode_pal8(pixels: &[u8], pal: &[u8], w: usize, h: usize) -> Vec<u8> {
    let linear = unswizzle(pixels, w, h, 8);
    let pal = palette(pal, 256);
    let mut out = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let idx = linear.get(i).copied().unwrap_or(0) as usize;
        out[i * 4..i * 4 + 4].copy_from_slice(&pal[idx]);
    }
    out
}

fn ascii(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Checks the first entry: count, name and a sane header (sizes are powers of two, bpp 4 / 8 / 32).
pub fn looks_like_psp_dic(data: &[u8]) -> bool {
    let Some(count) = le32(data, 0) else { return false };
    if !(1..=4096).contains(&count) {
        return false;
    }
    let Some(name_len) = le32(data, 4) else { return false };
    let name_len = name_len as usize;
    if !(1..=48).contains(&name_len) {
        return false;
    }
    let Some(name) = data.get(8..8 + name_len) else { return false };
    if !name.iter().all(|b| (0x20..0x7F).contains(b)) {
        return false;
    }
    let p = 8 + name_len;
    let (Some(w), Some(h), Some(&bpp), Some(pal_size)) =
        (le16(data, p), le16(data, p + 2), data.get(p + 6), le32(data, p + 12))
    else {
        return false;
    };
    let pow2 = |v: u16| v != 0 && v <= 1024 && v & (v - 1) == 0;
    pow2(w) && pow2(h) && matches!(bpp, 4 | 8 | 32) && pal_size <= 1024
}

pub fn parse_psp_dic(data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    let count = le32(data, 0).ok_or(DecodeError::Truncated { needed: 4, got: data.len() })? as usize;
    if count == 0 || count > 4096 {
        return Err(DecodeError::Invalid(format!("implausible texture count {count}")));
    }
    let mut out = Vec::new();
    let mut first_err: Option<DecodeError> = None;
    let mut off = 4usize;
    for _ in 0..count {
        let Some(name_len) = le32(data, off) else { break };
        let name_len = name_len as usize;
        off += 4;
        let Some(name_bytes) = data.get(off..off.saturating_add(name_len)) else { break };
        let name = ascii(name_bytes);
        off += name_len;
        let (Some(w), Some(h), Some(&bpp), Some(pal_size)) =
            (le16(data, off), le16(data, off + 2), data.get(off + 6), le32(data, off + 12))
        else {
            break;
        };
        let (w, h, pal_size) = (w as usize, h as usize, pal_size as usize);
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            break;
        }
        let (rgba, label, next) = match bpp {
            4 | 8 => {
                let pal_off = off + 16;
                let pixels_off = pal_off + pal_size + 4;
                let size = w * h * bpp as usize / 8;
                let pal = data.get(pal_off..(pal_off + pal_size).min(data.len())).unwrap_or(&[]);
                let pixels = data.get(pixels_off..(pixels_off + size).min(data.len())).unwrap_or(&[]);
                let rgba = if bpp == 4 { decode_pal4(pixels, pal, w, h) } else { decode_pal8(pixels, pal, w, h) };
                (rgba, if bpp == 4 { "PAL4" } else { "PAL8" }, pixels_off + size)
            }
            32 => {
                let pixels_off = off + 20;
                let size = w * h * 4;
                let pixels = data.get(pixels_off..(pixels_off + size).min(data.len())).unwrap_or(&[]);
                (unswizzle(pixels, w, h, 32), "RGBA8888", pixels_off + size)
            }
            other => {
                if first_err.is_none() {
                    first_err = Some(DecodeError::Unsupported(format!("PSP texture with {other} bits per pixel")));
                }
                break;
            }
        };
        out.push(Texture {
            name,
            width: w as u32,
            height: h as u32,
            rgba,
            format: format!("PSP {label} swizzled"),
        });
        off = next;
    }
    if out.is_empty() {
        return Err(first_err.unwrap_or_else(|| DecodeError::Invalid("no textures found in the PSP .dic".into())));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(d: &mut Vec<u8>, w: u16, h: u16, entries: u16, bpp: u8, pal_size: u32) {
        d.extend(1u32.to_le_bytes()); // count
        d.extend(3u32.to_le_bytes()); // name length
        d.extend(b"abc");
        let mut hd = [0u8; 16];
        hd[0..2].copy_from_slice(&w.to_le_bytes());
        hd[2..4].copy_from_slice(&h.to_le_bytes());
        hd[4..6].copy_from_slice(&entries.to_le_bytes());
        hd[6] = bpp;
        hd[12..16].copy_from_slice(&pal_size.to_le_bytes());
        d.extend(hd);
    }

    #[test]
    fn rgba8888_texture() {
        // 4x8 pixels of 4 bytes = one 16 x 8 block, so the layout is linear
        let mut d = Vec::new();
        header(&mut d, 4, 8, 0, 32, 0);
        d.extend([0u8; 4]); // the 4 bytes between the header and the pixels
        let mut px = vec![0u8; 4 * 8 * 4];
        px[0..4].copy_from_slice(&[10, 20, 30, 40]);
        d.extend(px);
        assert!(looks_like_psp_dic(&d));
        let t = parse_psp_dic(&d).unwrap();
        assert_eq!(t[0].name, "abc");
        assert_eq!((t[0].width, t[0].height), (4, 8));
        assert_eq!(&t[0].rgba[0..4], &[10, 20, 30, 40]);
    }

    #[test]
    fn pal4_texture_uses_the_low_nibble_first() {
        let mut d = Vec::new();
        header(&mut d, 32, 8, 16, 4, 64);
        let mut pal = vec![0u8; 64];
        pal[4..8].copy_from_slice(&[1, 2, 3, 4]); // color 1
        pal[8..12].copy_from_slice(&[5, 6, 7, 8]); // color 2
        d.extend(pal);
        d.extend([0u8; 4]);
        let mut px = vec![0u8; 32 * 8 / 2];
        px[0] = 0x21; // pixel 0 -> color 1, pixel 1 -> color 2
        d.extend(px);
        let t = parse_psp_dic(&d).unwrap();
        assert_eq!(&t[0].rgba[0..4], &[1, 2, 3, 4]);
        assert_eq!(&t[0].rgba[4..8], &[5, 6, 7, 8]);
    }

    #[test]
    fn rejects_other_data() {
        assert!(!looks_like_psp_dic(&[0, 0, 0, 5, 0, 0, 0, 0]));
        assert!(parse_psp_dic(&[1, 0]).is_err());
    }
}
