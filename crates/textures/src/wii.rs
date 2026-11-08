//! Obscure Wii textures: `.dic` packs (big-endian, several textures) and single-texture `.hvt`
//! files, both in GX texture formats (I8, IA8, RGB5A3, RGBA8, C4, C8, CMPR).
//! Ported from obscure-texture-tool (Kapie77). Read only.

use crate::{DecodeError, Texture};

const VALID_GX: [u32; 7] = [1, 3, 5, 6, 8, 9, 14];

fn be32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be16(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn truncated(needed: usize, got: usize) -> DecodeError {
    DecodeError::Truncated { needed, got }
}

fn align(v: usize, n: usize) -> usize {
    v.div_ceil(n) * n
}

// ---------------------------------------------------------------- GX pixel decoders

fn rgb565(c: u16) -> [u8; 4] {
    let c = c as u32;
    [
        (((c >> 11) & 31) * 255 / 31) as u8,
        (((c >> 5) & 63) * 255 / 63) as u8,
        ((c & 31) * 255 / 31) as u8,
        255,
    ]
}

fn rgb5a3(p: u16) -> [u8; 4] {
    let p = p as u32;
    if p & 0x8000 != 0 {
        [
            (((p >> 10) & 0x1F) * 255 / 31) as u8,
            (((p >> 5) & 0x1F) * 255 / 31) as u8,
            ((p & 0x1F) * 255 / 31) as u8,
            255,
        ]
    } else {
        [
            (((p >> 8) & 0x0F) * 255 / 15) as u8,
            (((p >> 4) & 0x0F) * 255 / 15) as u8,
            ((p & 0x0F) * 255 / 15) as u8,
            (((p >> 12) & 0x07) * 255 / 7) as u8,
        ]
    }
}

fn ia8_pixel(p: u16) -> [u8; 4] {
    let (a, i) = ((p >> 8) as u8, (p & 0xFF) as u8);
    [i, i, i, a]
}

/// Offset of a pixel in 4x4 tiles of 16-bit pixels (RGB5A3, IA8).
fn offset_16(x: usize, y: usize, sw: usize) -> usize {
    let blocks_x = (sw + 3) >> 2;
    (((y >> 2) * blocks_x + (x >> 2)) << 5) + (((y & 3) << 3) + ((x & 3) << 1))
}

/// Offset of a pixel in 8x4 tiles of 8-bit pixels (I8, C8).
fn offset_8(x: usize, y: usize, sw: usize) -> usize {
    let blocks_x = (sw + 7) >> 3;
    (((y >> 2) * blocks_x + (x >> 3)) << 5) + (((y & 3) << 3) + (x & 7))
}

fn put(out: &mut [u8], w: usize, x: usize, y: usize, rgba: [u8; 4]) {
    let d = (y * w + x) * 4;
    out[d..d + 4].copy_from_slice(&rgba);
}

fn decode_i8(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (sw, sh) = (align(w, 8), align(h, 4));
    for y in 0..sh {
        for x in 0..sw {
            let off = offset_8(x, y, sw);
            if off >= data.len() || x >= w || y >= h {
                continue;
            }
            let v = data[off];
            put(&mut out, w, x, y, [v, v, v, 255]);
        }
    }
    out
}

fn decode_16(data: &[u8], w: usize, h: usize, pixel: fn(u16) -> [u8; 4]) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (sw, sh) = (align(w, 4), align(h, 4));
    for y in 0..sh {
        for x in 0..sw {
            let off = offset_16(x, y, sw);
            if off + 1 >= data.len() || x >= w || y >= h {
                continue;
            }
            let p = ((data[off] as u16) << 8) | data[off + 1] as u16;
            put(&mut out, w, x, y, pixel(p));
        }
    }
    out
}

fn decode_rgba8(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (sw, sh) = (align(w, 4), align(h, 4));
    let mut off = 0usize;
    for y in (0..sh).step_by(4) {
        for x in (0..sw).step_by(4) {
            for dy in 0..4 {
                for dx in 0..4 {
                    if off + 33 < data.len() && x + dx < w && y + dy < h {
                        let (a, r, g, b) = (data[off], data[off + 1], data[off + 32], data[off + 33]);
                        put(&mut out, w, x + dx, y + dy, [r, g, b, a]);
                    }
                    off += 2;
                }
            }
            off += 32;
        }
    }
    out
}

fn decode_c8(data: &[u8], pal: &[[u8; 4]], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (sw, sh) = (align(w, 8), align(h, 4));
    for y in 0..sh {
        for x in 0..sw {
            let off = offset_8(x, y, sw);
            if off >= data.len() || x >= w || y >= h {
                continue;
            }
            let c = pal.get(data[off] as usize).copied().unwrap_or([0, 0, 0, 0]);
            put(&mut out, w, x, y, c);
        }
    }
    out
}

fn decode_c4(data: &[u8], pal: &[[u8; 4]], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (sw, sh) = (align(w, 8), align(h, 8));
    let tiles_per_row = sw / 8;
    for y in 0..sh {
        for x in 0..sw {
            let tile = (y / 8) * tiles_per_row + (x / 8);
            let off = tile * 32 + (y % 8) * 4 + (x % 8) / 2;
            if off >= data.len() || x >= w || y >= h {
                continue;
            }
            let b = data[off];
            let idx = if x % 2 == 1 { b & 0x0F } else { b >> 4 };
            let c = pal.get(idx as usize).copied().unwrap_or([0, 0, 0, 0]);
            put(&mut out, w, x, y, c);
        }
    }
    out
}

fn cmpr_block(block: &[u8]) -> [[u8; 4]; 16] {
    let c0 = ((block[0] as u16) << 8) | block[1] as u16;
    let c1 = ((block[2] as u16) << 8) | block[3] as u16;
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mut pal = [p0, p1, [0; 4], [0; 4]];
    if c0 > c1 {
        for k in 0..3 {
            pal[2][k] = ((2 * p0[k] as u32 + p1[k] as u32) / 3) as u8;
            pal[3][k] = ((2 * p1[k] as u32 + p0[k] as u32) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for k in 0..3 {
            pal[2][k] = ((p0[k] as u32 + p1[k] as u32) / 2) as u8;
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let mut px = [[0u8; 4]; 16];
    for row in 0..4 {
        let bits = block[4 + row];
        for col in 0..4 {
            px[row * 4 + col] = pal[((bits >> (6 - col * 2)) & 3) as usize];
        }
    }
    px
}

fn decode_cmpr(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (sw, sh) = (align(w, 8), align(h, 8));
    let mut off = 0usize;
    for y in (0..sh).step_by(8) {
        for x in (0..sw).step_by(8) {
            for sy in (0..8).step_by(4) {
                for sx in (0..8).step_by(4) {
                    if off + 8 <= data.len() {
                        let block = cmpr_block(&data[off..off + 8]);
                        for dy in 0..4 {
                            for dx in 0..4 {
                                let (px, py) = (x + sx + dx, y + sy + dy);
                                if px < w && py < h {
                                    put(&mut out, w, px, py, block[dy * 4 + dx]);
                                }
                            }
                        }
                    }
                    off += 8;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- .dic

fn is_valid_entry(data: &[u8], off: usize) -> bool {
    let Some(&name_len) = data.get(off) else { return false };
    let name_len = name_len as usize;
    if !(1..=48).contains(&name_len) {
        return false;
    }
    let Some(name) = data.get(off + 1..off + 1 + name_len) else { return false };
    if !name.iter().all(|b| (0x20..0x7F).contains(b)) {
        return false;
    }
    let p = off + 1 + name_len;
    if p + 28 > data.len() {
        return false;
    }
    let (Some(w), Some(h), Some(gx), Some(size)) =
        (be32(data, p), be32(data, p + 4), be32(data, p + 16), be32(data, p + 24))
    else {
        return false;
    };
    let pow2 = |v: u32| v != 0 && v <= 1024 && v & (v - 1) == 0;
    pow2(w)
        && pow2(h)
        && VALID_GX.contains(&gx)
        && size > 0
        && (size as usize) <= data.len()
        && p + 28 + size as usize <= data.len()
}

/// A Wii `.dic`: big-endian count, then entries starting at offset 7.
pub fn looks_like_wii_dic(data: &[u8]) -> bool {
    matches!(be32(data, 0), Some(c) if (1..=4096).contains(&c)) && is_valid_entry(data, 7)
}

fn ascii(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn gx_name(gx: u32) -> &'static str {
    match gx {
        1 => "I8",
        3 => "IA8",
        5 => "RGB5A3",
        6 => "RGBA8",
        8 => "C4",
        9 => "C8",
        14 => "CMPR",
        _ => "?",
    }
}

/// Palette block after the pixels of C4 / C8 textures: count, format, size (big-endian) and the colors.
fn read_palette(data: &[u8], off: usize) -> Option<Vec<[u8; 4]>> {
    let format = be32(data, off + 4)?;
    let size = be32(data, off + 8)? as usize;
    let start = off + 12;
    let bytes = data.get(start..(start + size).min(data.len()))?;
    Some(
        bytes
            .chunks_exact(2)
            .map(|c| {
                let v = ((c[0] as u16) << 8) | c[1] as u16;
                match format {
                    1 => rgb565(v),
                    0 => ia8_pixel(v),
                    _ => rgb5a3(v),
                }
            })
            .collect(),
    )
}

pub fn parse_wii_dic(data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    let count = be32(data, 0).ok_or_else(|| truncated(4, data.len()))? as usize;
    if count == 0 || count > 4096 {
        return Err(DecodeError::Invalid(format!("implausible texture count {count}")));
    }
    let mut out = Vec::new();
    let mut first_err: Option<DecodeError> = None;
    let mut off = 7usize;
    for _ in 0..count {
        let Some(&name_len) = data.get(off) else { break };
        let name_len = name_len as usize;
        if name_len == 0 || off + 1 + name_len > data.len() {
            break;
        }
        let name = ascii(&data[off + 1..off + 1 + name_len]);
        let p = off + 1 + name_len;
        let (Some(w), Some(h), Some(gx)) = (be32(data, p), be32(data, p + 4), be32(data, p + 16)) else { break };
        // the layout with 0x20 in the format field keeps the size one word earlier
        let (size, data_off) = if gx == 0x20 {
            (be32(data, p + 20), p + 24)
        } else {
            (be32(data, p + 24), p + 28)
        };
        let Some(size) = size else { break };
        let end = data_off.saturating_add(size as usize).min(data.len());
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            break;
        }
        let (wu, hu) = (w as usize, h as usize);
        let pixels = data.get(data_off..end).unwrap_or(&[]);
        let decoded: Result<Vec<u8>, DecodeError> = match gx {
            1 => Ok(decode_i8(pixels, wu, hu)),
            3 => Ok(decode_16(pixels, wu, hu, ia8_pixel)),
            5 => Ok(decode_16(pixels, wu, hu, rgb5a3)),
            6 => Ok(decode_rgba8(pixels, wu, hu)),
            14 => Ok(decode_cmpr(pixels, wu, hu)),
            8 | 9 => match read_palette(data, end) {
                Some(pal) if gx == 8 => Ok(decode_c4(pixels, &pal, wu, hu)),
                Some(pal) => Ok(decode_c8(pixels, &pal, wu, hu)),
                None => Err(DecodeError::Invalid(format!("palette of '{name}' is outside the file"))),
            },
            other => Err(DecodeError::Unsupported(format!("GX format {other} in '{name}'"))),
        };
        match decoded {
            Ok(rgba) => out.push(Texture {
                name,
                width: w,
                height: h,
                rgba,
                format: format!("Wii {}", gx_name(gx)),
            }),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        // the next entry starts somewhere after the pixels (and the palette, if there is one)
        let mut next = None;
        for cand in end..(end + 4096).min(data.len()) {
            if is_valid_entry(data, cand) {
                next = Some(cand);
                break;
            }
        }
        match next {
            Some(n) => off = n,
            None => break,
        }
    }
    if out.is_empty() {
        return Err(first_err.unwrap_or_else(|| DecodeError::Invalid("no textures found in the Wii .dic".into())));
    }
    Ok(out)
}

// ---------------------------------------------------------------- .hvt

/// A Wii `.hvt`: the magic " IVH", a format tag at 8 and the size at 0x0E / 0x12 (big-endian).
pub fn looks_like_wii_hvt(data: &[u8]) -> bool {
    data.len() >= 0x18 && data.starts_with(b" IVH")
}

pub fn parse_wii_hvt(name: &str, data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    if !looks_like_wii_hvt(data) {
        return Err(DecodeError::Invalid("not a Wii .hvt (missing the ' IVH' header)".into()));
    }
    let tag = &data[8..12];
    let w = be16(data, 0x0E).ok_or_else(|| truncated(0x10, data.len()))? as usize;
    let h = be16(data, 0x12).ok_or_else(|| truncated(0x14, data.len()))? as usize;
    let bpp = be32(data, 0x14).ok_or_else(|| truncated(0x18, data.len()))? as usize;
    if w == 0 || h == 0 || w > 4096 || h > 4096 || bpp == 0 || bpp > 32 {
        return Err(DecodeError::Invalid(format!("implausible size {w}x{h} at {bpp} bpp")));
    }
    let size = (w * h * bpp).div_ceil(8);
    let pixels = data.get(0x18..(0x18 + size).min(data.len())).unwrap_or(&[]);
    let (rgba, label) = match tag {
        b"S3TW" => (decode_cmpr(pixels, w, h), "CMPR"),
        b"G8A8" => (decode_16(pixels, w, h, ia8_pixel), "IA8"),
        b"GRY8" => (decode_i8(pixels, w, h), "I8"),
        b"4443" => (decode_16(pixels, w, h, rgb5a3), "RGB5A3"),
        b"ARGB" => (decode_rgba8(pixels, w, h), "RGBA8"),
        b"P8WI" => {
            let pal_off = 0x18 + size;
            let pal_bytes = data
                .get(pal_off..(pal_off + 512).min(data.len()))
                .ok_or_else(|| truncated(pal_off + 512, data.len()))?;
            let pal: Vec<[u8; 4]> = pal_bytes
                .chunks_exact(2)
                .map(|c| rgb5a3(((c[0] as u16) << 8) | c[1] as u16))
                .collect();
            (decode_c8(pixels, &pal, w, h), "C8")
        }
        other => {
            return Err(DecodeError::Unsupported(format!(
                "Wii .hvt format tag '{}'",
                String::from_utf8_lossy(other)
            )))
        }
    };
    Ok(vec![Texture {
        name: name.to_string(),
        width: w as u32,
        height: h as u32,
        rgba,
        format: format!("Wii {label}"),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb5a3_pixels() {
        assert_eq!(rgb5a3(0xFC00), [255, 0, 0, 255]); // opaque red (5-5-5)
        assert_eq!(rgb5a3(0x7F00), [255, 0, 0, 255]); // 3-bit alpha 7, red 15 (4-4-4)
        assert_eq!(rgb5a3(0x0F00)[3], 0);
        assert_eq!(ia8_pixel(0x8040), [64, 64, 64, 128]);
    }

    #[test]
    fn cmpr_block_with_two_colors() {
        // c0 red, c1 black, every index 0 -> red; 4 blocks of an 8x8 tile
        let mut d = Vec::new();
        for _ in 0..4 {
            d.extend([0xF8, 0x00, 0x00, 0x00, 0, 0, 0, 0]);
        }
        let px = decode_cmpr(&d, 8, 8);
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[(7 * 8 + 7) * 4..(7 * 8 + 7) * 4 + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn hvt_i8() {
        let mut d = vec![0u8; 0x18];
        d[0..4].copy_from_slice(b" IVH");
        d[8..12].copy_from_slice(b"GRY8");
        d[0x0E..0x10].copy_from_slice(&8u16.to_be_bytes());
        d[0x12..0x14].copy_from_slice(&4u16.to_be_bytes());
        d[0x14..0x18].copy_from_slice(&8u32.to_be_bytes());
        d.extend([0x80u8; 32]);
        let t = parse_wii_hvt("x", &d).unwrap();
        assert_eq!((t[0].width, t[0].height), (8, 4));
        assert_eq!(&t[0].rgba[0..4], &[0x80, 0x80, 0x80, 255]);
    }

    #[test]
    fn dic_with_one_i8_texture() {
        let mut d = vec![0u8; 7];
        d[0..4].copy_from_slice(&1u32.to_be_bytes());
        d.push(3);
        d.extend(b"abc");
        let p = d.len();
        d.resize(p + 28, 0);
        d[p..p + 4].copy_from_slice(&8u32.to_be_bytes());
        d[p + 4..p + 8].copy_from_slice(&4u32.to_be_bytes());
        d[p + 16..p + 20].copy_from_slice(&1u32.to_be_bytes()); // I8
        d[p + 24..p + 28].copy_from_slice(&32u32.to_be_bytes());
        d.extend([0x40u8; 32]);
        assert!(looks_like_wii_dic(&d));
        let t = parse_wii_dic(&d).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "abc");
        assert_eq!(&t[0].rgba[0..4], &[0x40, 0x40, 0x40, 255]);
        assert!(!looks_like_wii_dic(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3]));
    }
}
