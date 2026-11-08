//! Obscure PS2 `.dic` texture packs: RenderWare texture dictionaries (TXD, little-endian) with
//! the PS2 native texture data inside: 4 / 8-bit palettes (swizzled for the GS), RGBA5551 and
//! RGBA8888. Ported from obscure-texture-tool (Kapie77). Read only.

use crate::{DecodeError, Texture};

const RW_STRUCT: u32 = 0x01;
const RW_STRING: u32 = 0x02;
const RW_TEXTURE_NATIVE: u32 = 0x15;
const RW_TEXTURE_DICTIONARY: u32 = 0x16;

fn le32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

struct Chunk {
    id: u32,
    size: usize,
    body_start: usize,
    body_end: usize,
}

/// The chunks (id, size, version, body) that follow each other between `start` and `end`.
fn chunks(data: &[u8], start: usize, end: usize) -> Vec<Chunk> {
    let end = end.min(data.len());
    let mut out = Vec::new();
    let mut off = start;
    while off + 12 <= end {
        let (Some(id), Some(size)) = (le32(data, off), le32(data, off + 4)) else { break };
        let size = size as usize;
        let body_start = off + 12;
        out.push(Chunk { id, size, body_start, body_end: body_start.saturating_add(size) });
        off = match body_start.checked_add(size) {
            Some(n) => n,
            None => break,
        };
    }
    out
}

/// A TXD file starts with the texture dictionary chunk.
pub fn looks_like_ps2_dic(data: &[u8]) -> bool {
    le32(data, 0) == Some(RW_TEXTURE_DICTIONARY) && le32(data, 4).is_some_and(|s| s as usize <= data.len())
}

// ---------------------------------------------------------------- decoders

/// Position of a pixel in the swizzled 8-bit image of the PS2 graphics chip.
fn swizzle_id(x: usize, y: usize, w: usize) -> usize {
    let block = (y & !0xF) * w + (x & !0xF) * 2;
    let swap = (((y + 2) >> 2) & 1) * 4;
    let pos_y = (((y & !3) >> 1) + (y & 1)) & 7;
    let column = pos_y * w * 2 + ((x + swap) & 7) * 4;
    let byte = ((y >> 1) & 1) + ((x >> 2) & 2);
    block + column + byte
}

/// 8-bit palettes store the colors with bits 3 and 4 of the index swapped.
fn remap_clut_index(i: usize) -> usize {
    (i & 0xE7) | ((i & 0x08) << 1) | ((i & 0x10) >> 1)
}

fn rgb5551(v: u32) -> [u8; 4] {
    [
        ((v & 0x1F) * 255 / 31) as u8,
        (((v >> 5) & 0x1F) * 255 / 31) as u8,
        (((v >> 10) & 0x1F) * 255 / 31) as u8,
        if v & 0x8000 != 0 { 255 } else { 0 },
    ]
}

/// RGBA8888 (alpha 0..128 scaled to 0..255) when the palette is big enough, else RGBA5551.
fn palette(pal: &[u8], color_count: usize) -> [[u8; 4]; 256] {
    let mut colors = [[0u8; 4]; 256];
    if pal.len() >= color_count * 4 {
        for (i, c) in colors.iter_mut().enumerate().take(color_count.min(pal.len() / 4)) {
            *c = [pal[i * 4], pal[i * 4 + 1], pal[i * 4 + 2], (pal[i * 4 + 3] as u32 * 2).min(255) as u8];
        }
    } else {
        for (i, c) in colors.iter_mut().enumerate().take(color_count.min(pal.len() / 2)) {
            *c = rgb5551(pal[i * 2] as u32 | ((pal[i * 2 + 1] as u32) << 8));
        }
    }
    if color_count == 256 {
        let mut fixed = [[0u8; 4]; 256];
        for (i, c) in colors.iter().enumerate() {
            fixed[remap_clut_index(i)] = *c;
        }
        return fixed;
    }
    colors
}

fn decode_8bpp(pixels: &[u8], pal: &[u8], w: usize, h: usize) -> Vec<u8> {
    let palette = palette(pal, 256);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let sid = swizzle_id(x, y, w);
            let idx = pixels.get(sid).copied().unwrap_or(0) as usize;
            let d = (y * w + x) * 4;
            out[d..d + 4].copy_from_slice(&palette[idx]);
        }
    }
    out
}

fn decode_4bpp(pixels: &[u8], pal: &[u8], w: usize, h: usize) -> Vec<u8> {
    // two pixels per byte (low nibble first), then swizzled like the 8-bit image
    let mut unpacked = vec![0u8; w * h];
    for (i, u) in unpacked.iter_mut().enumerate() {
        if let Some(&b) = pixels.get(i / 2) {
            *u = if i % 2 == 0 { b & 0x0F } else { b >> 4 };
        }
    }
    let palette = palette(pal, 16);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let sid = swizzle_id(x, y, w);
            let idx = unpacked.get(sid).copied().unwrap_or(0) as usize & 0x0F;
            let d = (y * w + x) * 4;
            out[d..d + 4].copy_from_slice(&palette[idx]);
        }
    }
    out
}

fn decode_rgba8888(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for (i, p) in pixels.chunks_exact(4).take(w * h).enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&[p[0], p[1], p[2], (p[3] as u32 * 2).min(255) as u8]);
    }
    out
}

fn decode_rgba5551(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for (i, p) in pixels.chunks_exact(2).take(w * h).enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&rgb5551(p[0] as u32 | ((p[1] as u32) << 8)));
    }
    out
}

// ---------------------------------------------------------------- the dictionary

pub fn parse_ps2_dic(data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    if !looks_like_ps2_dic(data) {
        return Err(DecodeError::Invalid("not a RenderWare texture dictionary (TXD)".into()));
    }
    let root_size = le32(data, 4).unwrap_or(0) as usize;
    let mut out = Vec::new();
    let mut first_err: Option<DecodeError> = None;
    let mut index = 0usize;
    for c in chunks(data, 12, 12usize.saturating_add(root_size)) {
        if c.id != RW_TEXTURE_NATIVE {
            continue;
        }
        let mut name = format!("texture_{index:03}");
        let mut blob: Option<usize> = None;
        for cc in chunks(data, c.body_start, c.body_end) {
            if cc.id == RW_STRING {
                let raw = data.get(cc.body_start..cc.body_end.min(data.len())).unwrap_or(&[]);
                let first = raw.split(|b| *b == 0).next().unwrap_or(&[]);
                let text: String = first.iter().filter(|b| b.is_ascii()).map(|&b| b as char).collect();
                let text = text.trim();
                if !text.is_empty() {
                    name = text.to_string();
                }
            } else if cc.id == RW_STRUCT && cc.size > 64 && blob.is_none() {
                // the small "PS2\0" marker chunk is not the texture data
                if data.get(cc.body_start..cc.body_start + 4) == Some(b"PS2\0".as_slice()) {
                    continue;
                }
                blob = Some(cc.body_start);
            }
        }
        index += 1;
        let Some(blob) = blob else { continue };

        let (Some(w), Some(h), Some(bpp), Some(image_packet), Some(palette_packet)) = (
            le32(data, blob + 0x0C),
            le32(data, blob + 0x10),
            le32(data, blob + 0x14),
            le32(data, blob + 0x3C),
            le32(data, blob + 0x40),
        ) else {
            continue;
        };
        let (w, h, bpp) = (w as usize, h as usize, bpp as usize);
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            continue;
        }
        let image_offset = blob + 0xA8;
        let image_size = match bpp {
            4 => (w * h + 1) / 2,
            8 => w * h,
            _ => w * h * (bpp / 8),
        };
        let palette_offset = if palette_packet > 0 {
            Some(blob + 0x50 + image_packet as usize + 0x58)
        } else {
            None
        };
        let palette_size = match bpp {
            4 => 64,
            8 if palette_packet >= 0x450 => 1024,
            8 if palette_packet > 0 => 512,
            _ => 0,
        };
        let pixels = data.get(image_offset..(image_offset + image_size).min(data.len())).unwrap_or(&[]);
        let pal = palette_offset
            .and_then(|o| data.get(o..(o + palette_size).min(data.len())))
            .unwrap_or(&[]);

        let decoded = match bpp {
            8 if palette_offset.is_some() => Ok((decode_8bpp(pixels, pal, w, h), "PAL8")),
            4 if palette_offset.is_some() => Ok((decode_4bpp(pixels, pal, w, h), "PAL4")),
            32 => Ok((decode_rgba8888(pixels, w, h), "RGBA8888")),
            16 => Ok((decode_rgba5551(pixels, w, h), "RGBA5551")),
            4 | 8 => Err(DecodeError::Invalid(format!("'{name}' has no palette"))),
            other => Err(DecodeError::Unsupported(format!("PS2 texture with {other} bits per pixel in '{name}'"))),
        };
        match decoded {
            Ok((rgba, label)) => out.push(Texture {
                name,
                width: w as u32,
                height: h as u32,
                rgba,
                format: format!("PS2 {label} (RenderWare)"),
            }),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if out.is_empty() {
        return Err(first_err.unwrap_or_else(|| DecodeError::Invalid("no textures found in the PS2 .dic".into())));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: u32, body: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend(id.to_le_bytes());
        c.extend((body.len() as u32).to_le_bytes());
        c.extend(0x1C02_0037u32.to_le_bytes());
        c.extend_from_slice(body);
        c
    }

    /// A TXD with one texture: the marker chunk, the name and the big struct.
    fn txd(name: &str, blob: Vec<u8>) -> Vec<u8> {
        let mut tex = chunk(RW_STRUCT, b"PS2\0");
        tex.extend(chunk(RW_STRING, format!("{name}\0\0\0\0").as_bytes()));
        tex.extend(chunk(RW_STRUCT, &blob));
        let mut root = chunk(RW_STRUCT, &[1, 0, 0, 0]);
        root.extend(chunk(RW_TEXTURE_NATIVE, &tex));
        chunk(RW_TEXTURE_DICTIONARY, &root)
    }

    fn blob(w: u32, h: u32, bpp: u32, image_packet: u32, palette_packet: u32, image: &[u8], palette: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 0xA8];
        b[0x0C..0x10].copy_from_slice(&w.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&h.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&bpp.to_le_bytes());
        b[0x3C..0x40].copy_from_slice(&image_packet.to_le_bytes());
        b[0x40..0x44].copy_from_slice(&palette_packet.to_le_bytes());
        b.extend_from_slice(image);
        b.extend_from_slice(palette);
        b
    }

    #[test]
    fn rgba8888_alpha_is_doubled() {
        let d = txd("tex", blob(2, 1, 32, 0, 0, &[10, 20, 30, 64, 1, 2, 3, 128], &[]));
        assert!(looks_like_ps2_dic(&d));
        let t = parse_ps2_dic(&d).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "tex");
        assert_eq!((t[0].width, t[0].height), (2, 1));
        assert_eq!(t[0].rgba, [10, 20, 30, 128, 1, 2, 3, 255]);
    }

    #[test]
    fn pal8_uses_the_swizzle_and_the_clut_remap() {
        // 16x16 pixels: pixel (0,0) is stored at 0 and pixel (1,0) at 4
        let mut image = vec![0u8; 256];
        image[0] = 5;
        image[4] = 16;
        let mut pal = vec![0u8; 1024];
        pal[5 * 4..5 * 4 + 4].copy_from_slice(&[9, 8, 7, 50]);
        pal[8 * 4..8 * 4 + 4].copy_from_slice(&[1, 2, 3, 40]);
        let d = txd("p8", blob(16, 16, 8, 256, 0x450, &image, &pal));
        let t = parse_ps2_dic(&d).unwrap();
        assert_eq!(&t[0].rgba[0..4], &[9, 8, 7, 100]);
        // index 16 reads the color stored at index 8 (bits 3 and 4 are swapped)
        assert_eq!(&t[0].rgba[4..8], &[1, 2, 3, 80]);
    }

    #[test]
    fn rgba5551_pixels() {
        // red with alpha, then green without
        let d = txd("c16", blob(2, 1, 16, 0, 0, &[0x1F, 0x80, 0xE0, 0x03], &[]));
        let t = parse_ps2_dic(&d).unwrap();
        assert_eq!(t[0].rgba, [255, 0, 0, 255, 0, 255, 0, 0]);
    }

    #[test]
    fn swizzle_ids() {
        assert_eq!(swizzle_id(0, 0, 16), 0);
        assert_eq!(swizzle_id(1, 0, 16), 4);
        assert_eq!(remap_clut_index(8), 16);
        assert_eq!(remap_clut_index(16), 8);
        assert_eq!(remap_clut_index(0xFF), 0xFF);
    }

    #[test]
    fn rejects_other_data() {
        assert!(!looks_like_ps2_dic(&[0, 0, 0, 5, 0, 0, 0, 0]));
        assert!(parse_ps2_dic(b"not a txd at all").is_err());
        // a texture without a palette
        let d = txd("np", blob(16, 16, 8, 0, 0, &[0u8; 256], &[]));
        assert!(parse_ps2_dic(&d).is_err());
    }
}
