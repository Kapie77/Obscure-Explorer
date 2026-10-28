//! Decoders de pixel lineares (sem swizzle). Swizzle de PS2/PSP/Xbox/X360/PS3
//! entra depois, em módulos próprios, antes de chamar estas funções.

use crate::{need, DecodeError};

#[derive(Clone, Copy, Debug)]
pub enum Order32 {
    /// bytes R,G,B,A
    Rgba,
    /// bytes B,G,R,A
    Bgra,
    /// bytes A,R,G,B
    Argb,
    /// bytes B,G,R,X (alpha forçado a 255)
    Bgrx,
}

pub fn decode32(data: &[u8], w: u32, h: u32, order: Order32) -> Result<Vec<u8>, DecodeError> {
    let n = (w as usize) * (h as usize);
    need(data, n * 4)?;
    let mut out = Vec::with_capacity(n * 4);
    for px in data[..n * 4].chunks_exact(4) {
        let (r, g, b, a) = match order {
            Order32::Rgba => (px[0], px[1], px[2], px[3]),
            Order32::Bgra => (px[2], px[1], px[0], px[3]),
            Order32::Argb => (px[1], px[2], px[3], px[0]),
            Order32::Bgrx => (px[2], px[1], px[0], 255),
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub shift: u8,
    pub bits: u8,
}

/// Layout de 16 bits descrito por campos (posição do bit menos significativo e largura).
/// Os nomes das constantes listam os canais do bit mais significativo para o menos.
#[derive(Clone, Copy, Debug)]
pub struct Fmt16 {
    pub r: Field,
    pub g: Field,
    pub b: Field,
    pub a: Option<Field>,
}

const fn f(shift: u8, bits: u8) -> Field {
    Field { shift, bits }
}

impl Fmt16 {
    pub const R5G6B5: Fmt16 = Fmt16 { r: f(11, 5), g: f(5, 6), b: f(0, 5), a: None };
    pub const B5G6R5: Fmt16 = Fmt16 { r: f(0, 5), g: f(5, 6), b: f(11, 5), a: None };
    pub const R5G5B5A1: Fmt16 = Fmt16 { r: f(11, 5), g: f(6, 5), b: f(1, 5), a: Some(f(0, 1)) };
    pub const B5G5R5A1: Fmt16 = Fmt16 { r: f(1, 5), g: f(6, 5), b: f(11, 5), a: Some(f(0, 1)) };
    pub const A1R5G5B5: Fmt16 = Fmt16 { r: f(10, 5), g: f(5, 5), b: f(0, 5), a: Some(f(15, 1)) };
}

fn extract(v: u16, fld: Field) -> u8 {
    let max = (1u32 << fld.bits) - 1;
    let raw = ((v as u32) >> fld.shift) & max;
    ((raw * 255 + max / 2) / max) as u8
}

pub fn decode16(
    data: &[u8],
    w: u32,
    h: u32,
    fmt: Fmt16,
    big_endian: bool,
) -> Result<Vec<u8>, DecodeError> {
    let n = (w as usize) * (h as usize);
    need(data, n * 2)?;
    let mut out = Vec::with_capacity(n * 4);
    for px in data[..n * 2].chunks_exact(2) {
        let v = if big_endian {
            u16::from_be_bytes([px[0], px[1]])
        } else {
            u16::from_le_bytes([px[0], px[1]])
        };
        let a = match fmt.a {
            Some(fa) => extract(v, fa),
            None => 255,
        };
        out.extend_from_slice(&[extract(v, fmt.r), extract(v, fmt.g), extract(v, fmt.b), a]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_red() {
        let out = decode16(&[0x00, 0xF8], 1, 1, Fmt16::R5G6B5, false).unwrap();
        assert_eq!(out, [255, 0, 0, 255]);
    }

    #[test]
    fn a1r5g5b5_alpha_bit() {
        let out = decode16(&[0x00, 0x80], 1, 1, Fmt16::A1R5G5B5, false).unwrap();
        assert_eq!(out, [0, 0, 0, 255]);
    }

    #[test]
    fn bgra_reorders() {
        let out = decode32(&[10, 20, 30, 40], 1, 1, Order32::Bgra).unwrap();
        assert_eq!(out, [30, 20, 10, 40]);
    }

    #[test]
    fn truncated_is_error() {
        assert!(decode32(&[0, 0, 0], 1, 1, Order32::Rgba).is_err());
    }
}
