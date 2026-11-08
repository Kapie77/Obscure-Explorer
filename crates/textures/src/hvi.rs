//! Obscure `.hvi` textures of the PS2 and PSP versions: a header, a 256-color BGRA palette and
//! 8-bit pixels in linear order (no swizzle). Ported from obscure-texture-tool (Kapie77).
//!
//! Layout (little-endian): "HVI ", width u32 at 0x0C, height u32 at 0x10, bits per pixel u32 at 0x14
//! (only 8), palette (256 x B,G,R,A) at 0x18, pixels (one byte each) at 0x418.

use crate::{DecodeError, Texture};

fn le32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

const PALETTE_OFFSET: usize = 0x18;
const PIXEL_OFFSET: usize = PALETTE_OFFSET + 1024;

pub fn looks_like_hvi(data: &[u8]) -> bool {
    data.len() >= PIXEL_OFFSET && data.starts_with(b"HVI ")
}

pub fn parse_hvi(name: &str, data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    if !looks_like_hvi(data) {
        return Err(DecodeError::Invalid("not an .hvi file (missing the 'HVI ' header or the palette)".into()));
    }
    let w = le32(data, 0x0C).unwrap_or(0) as usize;
    let h = le32(data, 0x10).unwrap_or(0) as usize;
    let bpp = le32(data, 0x14).unwrap_or(0);
    if bpp != 8 {
        return Err(DecodeError::Unsupported(format!("HVI with {bpp} bits per pixel (only 8 is supported)")));
    }
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err(DecodeError::Invalid(format!("implausible size {w}x{h}")));
    }
    let pal = &data[PALETTE_OFFSET..PIXEL_OFFSET];
    // palettes with alpha up to 0x80 (PS2 style) are scaled to the full 0..255 range
    let scale_alpha = pal.chunks_exact(4).map(|c| c[3]).max().unwrap_or(0) <= 0x90;
    let colors: Vec<[u8; 4]> = pal
        .chunks_exact(4)
        .map(|c| {
            let a = if scale_alpha { (c[3] as u32 * 2).min(255) as u8 } else { c[3] };
            [c[2], c[1], c[0], a]
        })
        .collect();
    let pixels = &data[PIXEL_OFFSET..];
    let mut rgba = vec![0u8; w * h * 4];
    for i in 0..w * h {
        if let Some(&idx) = pixels.get(i) {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&colors[idx as usize]);
        }
    }
    let platform = if w <= 480 && h <= 272 { "PSP" } else { "PS2" };
    Ok(vec![Texture {
        name: name.to_string(),
        width: w as u32,
        height: h as u32,
        rgba,
        format: format!("{platform} HVI 8-bit palette"),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(alpha: u8) -> Vec<u8> {
        let mut d = vec![0u8; PIXEL_OFFSET];
        d[0..4].copy_from_slice(b"HVI ");
        d[0x0C..0x10].copy_from_slice(&4u32.to_le_bytes());
        d[0x10..0x14].copy_from_slice(&2u32.to_le_bytes());
        d[0x14..0x18].copy_from_slice(&8u32.to_le_bytes());
        // color 1: B=10, G=20, R=30, A
        d[PALETTE_OFFSET + 4..PALETTE_OFFSET + 8].copy_from_slice(&[10, 20, 30, alpha]);
        let mut px = vec![0u8; 8];
        px[0] = 1;
        d.extend(px);
        d
    }

    #[test]
    fn palette_is_bgra_and_low_alpha_is_scaled() {
        let t = parse_hvi("x", &sample(0x40)).unwrap();
        assert_eq!((t[0].width, t[0].height), (4, 2));
        assert_eq!(&t[0].rgba[0..4], &[30, 20, 10, 128]);
        assert_eq!(&t[0].rgba[4..8], &[0, 0, 0, 0]); // color 0 is fully transparent
    }

    #[test]
    fn full_range_alpha_is_kept() {
        let t = parse_hvi("x", &sample(0xFF)).unwrap();
        assert_eq!(&t[0].rgba[0..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn rejects_other_files_and_depths() {
        assert!(!looks_like_hvi(b"HVI "));
        assert!(parse_hvi("x", b"nothing").is_err());
        let mut d = sample(0x40);
        d[0x14..0x18].copy_from_slice(&4u32.to_le_bytes());
        assert!(matches!(parse_hvi("x", &d), Err(DecodeError::Unsupported(_))));
    }
}
