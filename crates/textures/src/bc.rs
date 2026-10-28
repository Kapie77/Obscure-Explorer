//! Decoders BC1/BC2/BC3 (DXT1/DXT3/DXT5) para RGBA8.
//! Para X360/PS3 o tiling/endianness deve ser desfeito antes de chamar `decode`.

use crate::{need, DecodeError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcKind {
    Bc1,
    Bc2,
    Bc3,
}

impl BcKind {
    fn block_size(self) -> usize {
        match self {
            BcKind::Bc1 => 8,
            _ => 16,
        }
    }
}

pub fn decode(data: &[u8], w: u32, h: u32, kind: BcKind) -> Result<Vec<u8>, DecodeError> {
    let (w, h) = (w as usize, h as usize);
    let bw = (w + 3) / 4;
    let bh = (h + 3) / 4;
    let bs = kind.block_size();
    need(data, bw * bh * bs)?;

    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * bs;
            let block = &data[off..off + bs];
            let mut px = [[0u8; 4]; 16];
            match kind {
                BcKind::Bc1 => color_block(&block[0..8], true, &mut px),
                BcKind::Bc2 => {
                    color_block(&block[8..16], false, &mut px);
                    bc2_alpha(&block[0..8], &mut px);
                }
                BcKind::Bc3 => {
                    color_block(&block[8..16], false, &mut px);
                    bc3_alpha(&block[0..8], &mut px);
                }
            }
            for y in 0..4 {
                for x in 0..4 {
                    let (ox, oy) = (bx * 4 + x, by * 4 + y);
                    if ox < w && oy < h {
                        let o = (oy * w + ox) * 4;
                        out[o..o + 4].copy_from_slice(&px[y * 4 + x]);
                    }
                }
            }
        }
    }
    Ok(out)
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
    ]
}

/// `bc1_mode`: BC1 usa o modo de 3 cores + transparente quando c0 <= c1;
/// BC2/BC3 sempre usam 4 cores.
fn color_block(b: &[u8], bc1_mode: bool, px: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let (e0, e1) = (rgb565(c0), rgb565(c1));

    let mut pal = [[0u8; 4]; 4];
    pal[0] = [e0[0], e0[1], e0[2], 255];
    pal[1] = [e1[0], e1[1], e1[2], 255];
    if !bc1_mode || c0 > c1 {
        for i in 0..3 {
            pal[2][i] = ((2 * e0[i] as u32 + e1[i] as u32) / 3) as u8;
            pal[3][i] = ((e0[i] as u32 + 2 * e1[i] as u32) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for i in 0..3 {
            pal[2][i] = ((e0[i] as u32 + e1[i] as u32) / 2) as u8;
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    for i in 0..16 {
        px[i] = pal[((bits >> (2 * i)) & 3) as usize];
    }
}

fn bc2_alpha(b: &[u8], px: &mut [[u8; 4]; 16]) {
    for i in 0..16 {
        let byte = b[i / 2];
        let nibble = if i % 2 == 0 { byte & 0x0F } else { byte >> 4 };
        px[i][3] = nibble * 17;
    }
}

fn bc3_alpha(b: &[u8], px: &mut [[u8; 4]; 16]) {
    let a0 = b[0] as u32;
    let a1 = b[1] as u32;
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for k in 1..7u32 {
            pal[(k + 1) as usize] = ((7 - k) * a0 + k * a1) / 7;
        }
    } else {
        for k in 1..5u32 {
            pal[(k + 1) as usize] = ((5 - k) * a0 + k * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits: u64 = 0;
    for i in 0..6 {
        bits |= (b[2 + i] as u64) << (8 * i);
    }
    for i in 0..16 {
        px[i][3] = pal[((bits >> (3 * i)) & 7) as usize] as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bc1_solid_red() {
        let block = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let out = decode(&block, 4, 4, BcKind::Bc1).unwrap();
        assert!(out.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
    }

    #[test]
    fn bc1_transparent_index() {
        let block = [0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        let out = decode(&block, 4, 4, BcKind::Bc1).unwrap();
        assert!(out.chunks_exact(4).all(|p| p == [0, 0, 0, 0]));
    }

    #[test]
    fn bc3_opaque_red() {
        let mut block = [0u8; 16];
        block[0] = 255;
        block[1] = 0;
        block[8] = 0x00;
        block[9] = 0xF8;
        block[10] = 0x00;
        block[11] = 0xF8;
        let out = decode(&block, 4, 4, BcKind::Bc3).unwrap();
        assert!(out.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
    }

    #[test]
    fn non_multiple_of_four_is_clipped() {
        let data = vec![0u8; 4 * 8]; // 2x2 blocos BC1
        let out = decode(&data, 5, 5, BcKind::Bc1).unwrap();
        assert_eq!(out.len(), 5 * 5 * 4);
    }

    #[test]
    fn truncated_is_error() {
        assert!(decode(&[0u8; 7], 4, 4, BcKind::Bc1).is_err());
    }
}
