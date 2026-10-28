//! Parsers de `.dic` (PC, header big-endian) e `.dip` (Obscure 1 PC, little-endian).
//! Portado de DicFile.cs / DicCodec.cs (ObsCure-Texture-Editor, MIT).
//!
//! Layout de cada textura (inteiros de 32 bits na endianness do arquivo):
//!   4 bytes ignorados | tamanho do nome | nome | nº de mipmaps | flag alpha |
//!   flag alpha 1-bit | largura | altura | formato | [tamanho do mip + bytes] x mipmaps
//! Só o mip 0 é decodificado. Os pixels de 16 bits são sempre little-endian.

use crate::pixel::{decode16, decode32, Fmt16, Order32};
use crate::{DecodeError, Texture};

#[derive(Clone, Copy)]
enum Endian {
    Big,
    Little,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| DecodeError::Invalid("offset out of range".into()))?;
        if end > self.data.len() {
            return Err(DecodeError::Truncated { needed: end, got: self.data.len() });
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u32(&mut self, e: Endian) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Ok(match e {
            Endian::Big => u32::from_be_bytes(a),
            Endian::Little => u32::from_le_bytes(a),
        })
    }
}

/// Nomes vistos até agora são ASCII; bytes fora de ASCII viram Latin-1.
fn name_from(bytes: &[u8]) -> String {
    let s: String = bytes.iter().map(|&c| c as char).collect();
    s.trim_end_matches('\0').to_string()
}

/// `.dic` de PC (Obscure 1/2): header big-endian a partir do byte 0.
pub fn parse_pc_dic(data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    parse(data, 0, Endian::Big, false)
}

/// `.dip` de PC (Obscure 1): little-endian, pula um u32 inicial.
pub fn parse_dip(data: &[u8]) -> Result<Vec<Texture>, DecodeError> {
    parse(data, 4, Endian::Little, true)
}

fn parse(data: &[u8], start: usize, e: Endian, dip: bool) -> Result<Vec<Texture>, DecodeError> {
    let mut r = Reader::new(data, start);
    let count = r.u32(e)? as usize;
    if count == 0 || count > 4096 {
        return Err(DecodeError::Invalid(format!("invalid texture count: {count}")));
    }

    let mut out = Vec::new();
    let mut first_err: Option<DecodeError> = None;

    for i in 0..count {
        r.take(4)?; // 4 bytes ignorados
        let name_len = r.u32(e)? as usize;
        if name_len > 1024 {
            return Err(DecodeError::Invalid(format!("name too long in texture {i}")));
        }
        let name = name_from(r.take(name_len)?);
        let mipmaps = r.u32(e)? as usize;
        let alpha = r.u32(e)? != 0;
        let one_bit_alpha = r.u32(e)? != 0;
        let width = r.u32(e)?;
        let height = r.u32(e)?;
        let format = r.u32(e)?;

        if mipmaps == 0 || mipmaps > 32 {
            return Err(DecodeError::Invalid(format!("invalid mip count in '{name}': {mipmaps}")));
        }
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(DecodeError::Invalid(format!("invalid dimensions in '{name}': {width}x{height}")));
        }

        let mut first_mip: &[u8] = &[];
        for m in 0..mipmaps {
            let size = r.u32(e)? as usize;
            let bytes = r.take(size)?;
            if m == 0 {
                first_mip = bytes;
            }
        }

        let use_alpha = alpha || one_bit_alpha;
        let decoded: Result<(Vec<u8>, &'static str), DecodeError> = match format {
            21 => {
                let order = if dip { Order32::Bgra } else { Order32::Rgba };
                decode32(first_mip, width, height, order)
                    .map(|px| (px, if dip { "B8G8R8A8" } else { "R8G8B8A8" }))
            }
            23 => decode16(first_mip, width, height, Fmt16::R5G6B5, false).map(|px| (px, "R5G6B5")),
            25 => decode16(first_mip, width, height, Fmt16::A1R5G5B5, false).map(|mut px| {
                if !use_alpha {
                    for p in px.chunks_exact_mut(4) {
                        p[3] = 255;
                    }
                }
                (px, "A1R5G5B5")
            }),
            other => Err(DecodeError::Unsupported(format!(
                "pixel format {other} in texture '{name}'"
            ))),
        };

        match decoded {
            Ok((rgba, label)) => out.push(Texture {
                name: if name.is_empty() { format!("texture_{i:03}") } else { name },
                width,
                height,
                rgba,
                format: format!("{label}, {mipmaps} mip(s)"),
            }),
            Err(err) => {
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
    }

    if out.is_empty() {
        if let Some(err) = first_err {
            return Err(err);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(v: u32) -> [u8; 4] {
        v.to_be_bytes()
    }
    fn le(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    fn sample_dic(format: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(be(1)); // nº de texturas
        d.extend([0u8; 4]); // 4 bytes ignorados
        d.extend(be(1)); // tamanho do nome
        d.push(b'a');
        d.extend(be(1)); // mipmaps
        d.extend(be(1)); // alpha
        d.extend(be(0)); // alpha 1-bit
        d.extend(be(2)); // largura
        d.extend(be(1)); // altura
        d.extend(be(format));
        d.extend(be(8)); // tamanho do mip 0
        d.extend([255, 0, 0, 255, 0, 255, 0, 255]);
        d
    }

    #[test]
    fn pc_dic_r8g8b8a8() {
        let t = parse_pc_dic(&sample_dic(21)).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "a");
        assert_eq!((t[0].width, t[0].height), (2, 1));
        assert_eq!(t[0].rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn pc_dic_unsupported_format_is_error() {
        assert!(matches!(parse_pc_dic(&sample_dic(99)), Err(DecodeError::Unsupported(_))));
    }

    #[test]
    fn pc_dic_truncated_is_error() {
        let mut d = sample_dic(21);
        d.truncate(d.len() - 3);
        assert!(parse_pc_dic(&d).is_err());
    }

    #[test]
    fn dip_b8g8r8a8_reorders_to_rgba() {
        let mut d = Vec::new();
        d.extend(le(0)); // u32 inicial
        d.extend(le(1)); // nº de texturas
        d.extend([0u8; 4]);
        d.extend(le(1));
        d.push(b'b');
        d.extend(le(1)); // mipmaps
        d.extend(le(0)); // alpha
        d.extend(le(0)); // alpha 1-bit
        d.extend(le(1)); // largura
        d.extend(le(1)); // altura
        d.extend(le(21));
        d.extend(le(4));
        d.extend([0, 0, 255, 255]); // B,G,R,A
        let t = parse_dip(&d).unwrap();
        assert_eq!(t[0].rgba, [255, 0, 0, 255]);
    }
}
