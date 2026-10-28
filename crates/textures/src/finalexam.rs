//! Final Exam `.hvt` textures (PC, PS3 and Xbox 360 flavours), ported from
//! FinalExamHvt.cs / FinalExamCodec.cs (ObsCure-Texture-Editor, MIT).
//!
//! Header (offsets identical on PC and PS3; X360 adds an extra chunk):
//!   0x00 magic: "HVI " (PC, little-endian) or " IVH" (PS3/X360, big-endian)
//!   0x0C "HEAD" tag, 0x14 pixel format tag, 0x18 width, 0x1C height, 0x20 bpp,
//!   0x24 architecture tag ("X360" for Xbox 360), 0x28 mipmap count.
//!   PC/PS3: mip 0 size at 0x3C, pixels at 0x40.  X360: pixels at 0x84, GPU-tiled.

use std::path::Path;

use crate::bc::{self, BcKind};
use crate::pixel::{decode32, Order32};
use crate::{DecodeError, Texture};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fmt {
    Bgra,
    Bgrx,
    Dxt1,
    Dxt3,
    Dxt5,
    Argb,
}

impl Fmt {
    fn is_bc(self) -> bool {
        matches!(self, Fmt::Dxt1 | Fmt::Dxt3 | Fmt::Dxt5)
    }

    fn block_bytes(self) -> usize {
        if self == Fmt::Dxt1 {
            8
        } else {
            16
        }
    }

    fn bc_kind(self) -> BcKind {
        match self {
            Fmt::Dxt1 => BcKind::Bc1,
            Fmt::Dxt3 => BcKind::Bc2,
            _ => BcKind::Bc3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Fmt::Bgra => "BGRA",
            Fmt::Bgrx => "BGRX",
            Fmt::Dxt1 => "DXT1",
            Fmt::Dxt3 => "DXT3",
            Fmt::Dxt5 => "DXT5",
            Fmt::Argb => "ARGB",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Plat {
    Pc,
    Ps3,
    X360,
}

pub fn parse(path: &str, data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    if data.len() < 0x40 {
        return Err(DecodeError::Invalid("file too small to be a Final Exam HVT".into()));
    }
    let pc = &data[0..4] == b"HVI ";
    let be = &data[0..4] == b" IVH";
    if !pc && !be {
        return Err(DecodeError::Invalid("not a Final Exam HVT (magic mismatch)".into()));
    }
    if be && &data[0x0C..0x10] != b"HEAD" {
        return Err(DecodeError::Invalid(
            "big-endian .hvt without a HEAD chunk (ObsCure GC/Wii format, not Final Exam)".into(),
        ));
    }
    let plat = if pc {
        Plat::Pc
    } else if &data[0x24..0x28] == b"X360" {
        Plat::X360
    } else {
        Plat::Ps3
    };

    let tag = String::from_utf8_lossy(&data[0x14..0x18]).into_owned();
    let fmt = match tag.as_str() {
        "BGRA" => Fmt::Bgra,
        "BGRX" | "XRGB" => Fmt::Bgrx,
        "1TXD" | "TXD1" | "DXT1" => Fmt::Dxt1,
        "3TXD" | "TXD3" | "DXT3" => Fmt::Dxt3,
        "5TXD" | "TXD5" | "DXT5" => Fmt::Dxt5,
        "ARGB" => Fmt::Argb,
        other => {
            return Err(DecodeError::Unsupported(format!("Final Exam pixel format '{other}'")))
        }
    };

    let rd = |off: usize| -> u32 {
        let b = [data[off], data[off + 1], data[off + 2], data[off + 3]];
        if be {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        }
    };
    let (w, h, bpp, mips) = (rd(0x18), rd(0x1C), rd(0x20), rd(0x28));
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(DecodeError::Invalid(format!("implausible texture size {w}x{h}")));
    }
    let (w, h) = (w as usize, h as usize);

    let (aw, ah, mip0_size, pixel_offset) = if plat == Plat::X360 {
        // 32x32 GPU tiles; a "texel" is a pixel for ARGB and a 4x4 block for BC formats
        let align = 32 * if fmt.is_bc() { 4 } else { 1 };
        let aw = (w + align - 1) & !(align - 1);
        let ah = (h + align - 1) & !(align - 1);
        let size = if fmt.is_bc() {
            ((aw + 3) / 4) * ((ah + 3) / 4) * fmt.block_bytes()
        } else {
            aw * ah * 4
        };
        (aw, ah, size, 0x84usize)
    } else {
        (w, h, rd(0x3C) as usize, 0x40usize)
    };

    if pixel_offset > data.len() {
        return Err(DecodeError::Truncated { needed: pixel_offset, got: data.len() });
    }
    let avail = data.len() - pixel_offset;
    let raw = &data[pixel_offset..pixel_offset + mip0_size.min(avail)];

    let rgba = if plat == Plat::X360 && fmt == Fmt::Argb {
        let linear = unswizzle_x360(raw, aw, ah, 1, 4)?;
        let full = decode32(&linear, aw as u32, ah as u32, Order32::Argb)?;
        crop_rgba(&full, aw, w, h)
    } else if plat == Plat::X360 && fmt.is_bc() {
        let mut linear = unswizzle_x360(raw, aw, ah, 4, fmt.block_bytes())?;
        // X360 stores BC blocks big-endian: swap every 16-bit pair
        for c in linear.chunks_exact_mut(2) {
            c.swap(0, 1);
        }
        let full = bc::decode(&linear, aw as u32, ah as u32, fmt.bc_kind())?;
        crop_rgba(&full, aw, w, h)
    } else {
        decode_linear(fmt, plat, raw, w, h, mips)?
    };

    let plat_label = match plat {
        Plat::Pc => "PC",
        Plat::Ps3 => "PS3",
        Plat::X360 => "Xbox 360",
    };
    let name = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("texture")
        .to_string();
    Ok(vec![Texture {
        name,
        width: w as u32,
        height: h as u32,
        rgba,
        format: format!("FinalExam {plat_label} {}, {bpp}bpp, {mips} mip(s)", fmt.label()),
    }])
}

fn decode_linear(
    fmt: Fmt,
    plat: Plat,
    raw: &[u8],
    w: usize,
    h: usize,
    mips: u32,
) -> Result<Vec<u8>, DecodeError> {
    let (wu, hu) = (w as u32, h as u32);
    match fmt {
        Fmt::Bgra => decode32(raw, wu, hu, Order32::Bgra),
        Fmt::Bgrx => decode32(raw, wu, hu, Order32::Bgrx),
        // PS3 keeps the ARGB tag, but the bytes are R,G,B,A; mipmapped ones are swizzled
        Fmt::Argb if plat == Plat::Ps3 => {
            if mips > 1 {
                decode_ps3_swizzled(raw, w, h)
            } else {
                decode32(raw, wu, hu, Order32::Rgba)
            }
        }
        Fmt::Argb => decode32(raw, wu, hu, Order32::Argb),
        Fmt::Dxt1 | Fmt::Dxt3 | Fmt::Dxt5 => bc::decode(raw, wu, hu, fmt.bc_kind()),
    }
}

fn crop_rgba(full: &[u8], src_w: usize, dst_w: usize, dst_h: usize) -> Vec<u8> {
    let mut o = Vec::with_capacity(dst_w * dst_h * 4);
    for y in 0..dst_h {
        let s = y * src_w * 4;
        o.extend_from_slice(&full[s..s + dst_w * 4]);
    }
    o
}

/// PS3 swizzle (bit interleaving of x and y, square-ish textures).
fn swizzle_offset(x: usize, y: usize, w: usize, h: usize) -> usize {
    let (mut offset, mut xs, mut ys, mut dest) = (0usize, 0usize, 0usize, 0usize);
    while (1usize << xs) < w || (1usize << ys) < h {
        if (1usize << xs) < w {
            offset |= ((x >> xs) & 1) << dest;
            xs += 1;
            dest += 1;
        }
        if (1usize << ys) < h {
            offset |= ((y >> ys) & 1) << dest;
            ys += 1;
            dest += 1;
        }
    }
    offset
}

fn decode_ps3_swizzled(raw: &[u8], w: usize, h: usize) -> Result<Vec<u8>, DecodeError> {
    if w * h * 4 > (512 << 20) {
        return Err(DecodeError::Invalid("texture too large".into()));
    }
    let mut o = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let src = swizzle_offset(x, y, w, h) * 4;
            let dst = (y * w + x) * 4;
            if src + 4 > raw.len() {
                continue;
            }
            o[dst..dst + 4].copy_from_slice(&raw[src..src + 4]);
        }
    }
    Ok(o)
}

// ---- Xbox 360 GPU tiling (port of XGAddress2DTiledX/Y) ----

fn x360_tiled_x(block_offset: usize, width_in_blocks: usize, pitch: usize) -> usize {
    let aligned_width = (width_in_blocks + 31) & !31;
    let log_bpp = (pitch >> 2) + ((pitch >> 1) >> (pitch >> 2));
    let offset_byte = block_offset << log_bpp;
    let offset_tile =
        ((offset_byte & !0xFFF) >> 3) + ((offset_byte & 0x700) >> 2) + (offset_byte & 0x3F);
    let offset_macro = offset_tile >> (7 + log_bpp);
    let macro_x = (offset_macro % (aligned_width >> 5)) << 2;
    let tile = (((offset_tile >> (5 + log_bpp)) & 2) + (offset_byte >> 6)) & 3;
    let macro_ = (macro_x + tile) << 3;
    let micro = ((((offset_tile >> 1) & !0xF) + (offset_tile & 0xF)) & ((pitch << 3) - 1)) >> log_bpp;
    macro_ + micro
}

fn x360_tiled_y(block_offset: usize, width_in_blocks: usize, pitch: usize) -> usize {
    let aligned_width = (width_in_blocks + 31) & !31;
    let log_bpp = (pitch >> 2) + ((pitch >> 1) >> (pitch >> 2));
    let offset_byte = block_offset << log_bpp;
    let offset_tile =
        ((offset_byte & !0xFFF) >> 3) + ((offset_byte & 0x700) >> 2) + (offset_byte & 0x3F);
    let offset_macro = offset_tile >> (7 + log_bpp);
    let macro_y = (offset_macro / (aligned_width >> 5)) << 2;
    let tile = ((offset_tile >> (6 + log_bpp)) & 1) + ((offset_byte & 0x800) >> 10);
    let macro_ = (macro_y + tile) << 3;
    let micro = (((offset_tile & (((pitch << 6) - 1) & !0x1F)) + ((offset_tile & 0xF) << 1))
        >> (3 + log_bpp))
        & !1;
    macro_ + micro + ((offset_tile & 0x10) >> 4)
}

fn unswizzle_x360(
    swizzled: &[u8],
    width: usize,
    height: usize,
    block_px: usize,
    pitch: usize,
) -> Result<Vec<u8>, DecodeError> {
    let wb = width / block_px;
    let hb = height / block_px;
    if wb * hb * pitch > (512 << 20) {
        return Err(DecodeError::Invalid("texture too large".into()));
    }
    let pwb = (wb + 31) & !31;
    let phb = (hb + 31) & !31;
    let total = pwb * phb;
    let mut o = vec![0u8; wb * hb * pitch];
    for block_offset in 0..total {
        let x = x360_tiled_x(block_offset, pwb, pitch);
        let y = x360_tiled_y(block_offset, pwb, pitch);
        if x >= wb || y >= hb {
            continue;
        }
        let src = block_offset * pitch;
        let dst = (y * wb + x) * pitch;
        if src + pitch > swizzled.len() {
            continue;
        }
        o[dst..dst + pitch].copy_from_slice(&swizzled[src..src + pitch]);
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn pc_header(tag: &[u8; 4], w: u32, h: u32, mip0: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x40];
        d[0..4].copy_from_slice(b"HVI ");
        d[0x14..0x18].copy_from_slice(tag);
        d[0x18..0x1C].copy_from_slice(&w.to_le_bytes());
        d[0x1C..0x20].copy_from_slice(&h.to_le_bytes());
        d[0x20..0x24].copy_from_slice(&32u32.to_le_bytes());
        d[0x28..0x2C].copy_from_slice(&1u32.to_le_bytes());
        d[0x3C..0x40].copy_from_slice(&mip0.to_le_bytes());
        d
    }

    #[test]
    fn pc_bgra() {
        let mut d = pc_header(b"BGRA", 2, 1, 8);
        d.extend([10, 20, 30, 255, 1, 2, 3, 4]);
        let t = parse("x.hvt", &d).unwrap();
        assert_eq!((t[0].width, t[0].height), (2, 1));
        assert_eq!(t[0].rgba, [30, 20, 10, 255, 3, 2, 1, 4]);
    }

    #[test]
    fn pc_bgrx_is_opaque() {
        let mut d = pc_header(b"BGRX", 1, 1, 4);
        d.extend([10, 20, 30, 0]);
        let t = parse("x.hvt", &d).unwrap();
        assert_eq!(t[0].rgba, [30, 20, 10, 255]);
    }

    #[test]
    fn pc_dxt1_solid_red() {
        let mut d = pc_header(b"1TXD", 4, 4, 8);
        d.extend([0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0]);
        let t = parse("x.hvt", &d).unwrap();
        assert!(t[0].rgba.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
    }

    #[test]
    fn swizzle_is_morton_like() {
        assert_eq!(swizzle_offset(1, 0, 4, 4), 1);
        assert_eq!(swizzle_offset(0, 1, 4, 4), 2);
        assert_eq!(swizzle_offset(1, 1, 4, 4), 3);
        assert_eq!(swizzle_offset(2, 0, 4, 4), 4);
    }

    #[test]
    fn x360_tiling_is_a_bijection() {
        for &pitch in &[4usize, 8, 16] {
            let mut seen = HashSet::new();
            for o in 0..1024 {
                let x = x360_tiled_x(o, 32, pitch);
                let y = x360_tiled_y(o, 32, pitch);
                assert!(x < 32 && y < 32, "pitch {pitch}: offset {o} -> ({x},{y})");
                assert!(seen.insert((x, y)), "pitch {pitch}: duplicate ({x},{y})");
            }
        }
    }

    #[test]
    fn rejects_wrong_magic() {
        let d = vec![0u8; 0x80];
        assert!(parse("x.hvt", &d).is_err());
    }
}
