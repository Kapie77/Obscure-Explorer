//! Reader for ObsCure `.lng` text files (Obscure 1, Obscure 2 and Final Exam), converted to a
//! readable `.txt`. The formats and the text layout come from ObsCure-Text-Editor
//! (HeitorSpectre); this is a port of its extraction code (read only).
//!
//! * Obscure 1  - big-endian: language code, entry count, then group/id/length/encoding entries
//!                (cp1252 or UTF-16LE). Button glyphs are shown as tags like `[Cross]`.
//! * Obscure 2  - little-endian: language code, groups of (meta, length, bytes) entries (UTF-8).
//! * Final Exam - little-endian: glyph table, string ids with tagged sub-strings, UTF-8 string data.

use std::fmt;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LngGame {
    Obscure1,
    Obscure2,
    FinalExam,
}

#[derive(Debug)]
pub struct LngError(pub String);

impl fmt::Display for LngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for LngError {}

fn fail<T>(msg: impl Into<String>) -> Result<T, LngError> {
    Err(LngError(msg.into()))
}

/// Guesses the game from the first 8 bytes, like the original tool.
pub fn detect(data: &[u8]) -> Option<LngGame> {
    if data.len() < 8 {
        return None;
    }
    let a_be = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let b_be = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let a_le = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let b_le = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if a_le == 1 && (b_le & 0xFF00_FFFF) == 0x0100_0000 {
        Some(LngGame::FinalExam)
    } else if a_be == 0 && (1..=100_000).contains(&b_be) {
        Some(LngGame::Obscure1)
    } else if (1..=10_000).contains(&b_le) {
        Some(LngGame::Obscure2)
    } else {
        None
    }
}

/// Converts a `.lng` file to the readable text format.
pub fn lng_to_txt(data: &[u8]) -> Result<(LngGame, String), LngError> {
    match detect(data) {
        Some(LngGame::Obscure1) => Ok((LngGame::Obscure1, ob1_to_txt(data)?)),
        Some(LngGame::Obscure2) => Ok((LngGame::Obscure2, ob2_to_txt(data)?)),
        Some(LngGame::FinalExam) => Ok((LngGame::FinalExam, final_exam_to_txt(data)?)),
        None => fail("unrecognised .lng layout (not Obscure 1, Obscure 2 or Final Exam)"),
    }
}

struct Cur<'a> {
    data: &'a [u8],
    pos: usize,
    be: bool,
}

impl<'a> Cur<'a> {
    fn new(data: &'a [u8], be: bool) -> Self {
        Self { data, pos: 0, be }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], LngError> {
        let end = self.pos.checked_add(n).ok_or_else(|| LngError("size overflow".into()))?;
        if end > self.data.len() {
            return fail(format!("unexpected end of file at offset {}", self.pos));
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, LngError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, LngError> {
        let b = self.take(2)?;
        Ok(if self.be { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) })
    }

    fn u32(&mut self) -> Result<u32, LngError> {
        let b = self.take(4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Ok(if self.be { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
    }
}

fn escape_newlines(s: &str) -> String {
    s.replace('\n', "\\n").replace('\r', "\\r")
}

// ---------------------------------------------------------------- Obscure 2

fn ob2_to_txt(data: &[u8]) -> Result<String, LngError> {
    let mut r = Cur::new(data, false);
    let language = r.u32()?;
    let groups = r.u32()?;
    let mut out = String::new();
    let _ = write!(out, "### LANGUAGE\ngame = ob2\nlanguageCode = {language}\n###\n\n");
    for g in 0..groups {
        let group_id = r.u32()?;
        let entries = r.u32()?;
        for e in 0..entries {
            let meta = r.u32()?;
            let len = r.u32()? as usize;
            let text = if len > 0 {
                // the original replaces real new lines by a visible "\n"
                String::from_utf8_lossy(r.take(len)?).replace('\n', "\\n")
            } else {
                String::new()
            };
            let _ = write!(
                out,
                "### ENTRY\ngroup_index = {g}\ngroup_id = {group_id}\nentry_index = {e}\nmeta = {meta}\n###\n{text}\n\n"
            );
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- Obscure 1

/// Windows-1252 decoding (the 0x80..0x9F range differs from Latin-1).
fn cp1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80 => '\u{20AC}',
            0x82 => '\u{201A}',
            0x83 => '\u{0192}',
            0x84 => '\u{201E}',
            0x85 => '\u{2026}',
            0x86 => '\u{2020}',
            0x87 => '\u{2021}',
            0x88 => '\u{02C6}',
            0x89 => '\u{2030}',
            0x8A => '\u{0160}',
            0x8B => '\u{2039}',
            0x8C => '\u{0152}',
            0x8E => '\u{017D}',
            0x91 => '\u{2018}',
            0x92 => '\u{2019}',
            0x93 => '\u{201C}',
            0x94 => '\u{201D}',
            0x95 => '\u{2022}',
            0x96 => '\u{2013}',
            0x97 => '\u{2014}',
            0x98 => '\u{02DC}',
            0x99 => '\u{2122}',
            0x9A => '\u{0161}',
            0x9B => '\u{203A}',
            0x9C => '\u{0153}',
            0x9E => '\u{017E}',
            0x9F => '\u{0178}',
            b => b as char,
        })
        .collect()
}

/// PS2 button glyphs are shown as tags (the original tool does the same).
fn apply_ob1_tags(text: &str) -> String {
    // "x" followed by a variation selector is the L2 button; it must be replaced first
    let mut s = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{00D7}' && matches!(chars.peek(), Some(n) if ('\u{FE00}'..='\u{FE0F}').contains(n)) {
            chars.next();
            s.push_str("[L2]");
        } else {
            s.push(c);
        }
    }
    const TAGS: [(&str, &str); 11] = [
        ("\u{00F7}", "[L1]"),
        ("\u{00C6}", "[R1]"),
        ("\u{00D0}", "[Triangle]"),
        ("\u{00C3}", "[Cross]"),
        ("\u{00D8}", "[Select]"),
        ("\u{00C2}", "[Circle]"),
        ("\u{00C0}", "[Square]"),
        ("\u{00C7}", "[R2]"),
        ("\u{00C5}", "[Start]"),
        ("\u{00CA}", "[L3]"),
        ("\u{00F5}", "[R3]"),
    ];
    for (src, tag) in TAGS {
        s = s.replace(src, tag);
    }
    s
}

fn ob1_to_txt(data: &[u8]) -> Result<String, LngError> {
    let mut r = Cur::new(data, true);
    let language = r.u32()?;
    let count = r.u32()?;
    let mut out = String::new();
    let _ = write!(out, "### LANGUAGE\ngame = ob1\nlanguageCode = {language}\n###\n\n");
    for i in 0..count {
        let group = r.u16()?;
        let id = r.u16()?;
        let text_len = r.u32()? as i64;
        let enc = r.u8()?;
        let mut body_len = text_len - 1;
        let mut param: Option<u8> = None;
        if enc == 1 {
            param = Some(r.u8()?);
            body_len -= 1;
        }
        let body: &[u8] = if body_len > 0 { r.take(body_len as usize)? } else { &[] };
        let text = match enc {
            0 => {
                let mut len = body.len();
                while len > 0 && body[len - 1] == 0 {
                    len -= 1;
                }
                cp1252(&body[..len])
            }
            1 => {
                let mut byte_len = body.len();
                if byte_len % 2 != 0 {
                    byte_len -= 1;
                }
                while byte_len >= 2 && body[byte_len - 1] == 0 && body[byte_len - 2] == 0 {
                    byte_len -= 2;
                }
                let units: Vec<u16> =
                    body[..byte_len].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&units)
            }
            _ => {
                let mut hex = String::new();
                for b in body {
                    let _ = write!(hex, "{b:02X}");
                }
                format!("[RAW:{hex}]")
            }
        };
        let text = escape_newlines(&apply_ob1_tags(&text));
        let _ = write!(out, "### ENTRY\nindex = {i}\ngroup = {group}\nid = {id}\nencoding = {enc}\n");
        if let Some(p) = param {
            let _ = write!(out, "param = {p}\n");
        }
        let _ = write!(out, "###\n{text}\n\n");
    }
    Ok(out)
}

// ---------------------------------------------------------------- Final Exam

fn final_exam_to_txt(data: &[u8]) -> Result<String, LngError> {
    let mut r = Cur::new(data, false);
    let v1 = r.u32()?;
    let magic = r.u32()?;
    let _total_subs = r.u32()?;
    let entry_count = r.u32()?;
    let glyph_count = r.u32()? as usize;
    if glyph_count.checked_mul(4).map_or(true, |n| n > data.len()) {
        return fail("glyph table is larger than the file");
    }
    let mut glyphs = Vec::with_capacity(glyph_count);
    for _ in 0..glyph_count {
        glyphs.push(r.u32()?);
    }
    let mut entries: Vec<(u32, Vec<(u32, usize)>)> = Vec::new();
    for _ in 0..entry_count {
        let sid = r.u32()?;
        let n = r.u32()? as usize;
        if n.checked_mul(8).map_or(true, |b| b > data.len()) {
            return fail("sub-string table is larger than the file");
        }
        let mut subs = Vec::with_capacity(n);
        for _ in 0..n {
            let a = r.u32()?;
            let _b = r.u32()?;
            subs.push((a >> 17, (a & 0x0001_FFFF) as usize));
        }
        entries.push((sid, subs));
    }
    let data_size = r.u32()? as usize;
    let strings = r.take(data_size)?;

    let mut out = String::new();
    let glyph_list: Vec<String> = glyphs.iter().map(|g| format!("0x{g:X}")).collect();
    let _ = write!(
        out,
        "### LANGUAGE\ngame = finalexam\nv1 = {v1}\nmagic = 0x{magic:08X}\nglyphs = {}\n###\n\n",
        glyph_list.join(",")
    );
    for (i, (sid, subs)) in entries.iter().enumerate() {
        let _ = write!(out, "### ENTRY\nindex = {i}\nsid = 0x{sid:08X}\n###\n");
        for (tag, offset) in subs {
            let slice = strings.get(*offset..).unwrap_or(&[]);
            let end = slice.iter().position(|b| *b == 0).unwrap_or(slice.len());
            let text = escape_newlines(&String::from_utf8_lossy(&slice[..end]));
            let _ = write!(out, "[tag=0x{tag:04X}] {text}\n");
        }
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obscure2_sample() {
        let mut d = Vec::new();
        d.extend(7u32.to_le_bytes()); // language
        d.extend(1u32.to_le_bytes()); // groups
        d.extend(5u32.to_le_bytes()); // group id
        d.extend(1u32.to_le_bytes()); // entries
        d.extend(3u32.to_le_bytes()); // meta
        d.extend(5u32.to_le_bytes()); // length
        d.extend(b"Hi\nyo");
        let (game, txt) = lng_to_txt(&d).unwrap();
        assert_eq!(game, LngGame::Obscure2);
        assert_eq!(
            txt,
            "### LANGUAGE\ngame = ob2\nlanguageCode = 7\n###\n\n### ENTRY\ngroup_index = 0\ngroup_id = 5\nentry_index = 0\nmeta = 3\n###\nHi\\nyo\n\n"
        );
    }

    #[test]
    fn obscure1_sample_with_button_tag() {
        let mut d = Vec::new();
        d.extend(0u32.to_be_bytes()); // language
        d.extend(1u32.to_be_bytes()); // entries
        d.extend(1u16.to_be_bytes()); // group
        d.extend(2u16.to_be_bytes()); // id
        d.extend(5u32.to_be_bytes()); // length: encoding byte + 4 body bytes
        d.push(0); // cp1252
        d.extend([b'H', 0xC3, b'i', 0]);
        let (game, txt) = lng_to_txt(&d).unwrap();
        assert_eq!(game, LngGame::Obscure1);
        assert!(txt.contains("game = ob1"));
        assert!(txt.contains("encoding = 0\n###\nH[Cross]i\n\n"));
    }

    #[test]
    fn final_exam_sample() {
        let mut d = Vec::new();
        d.extend(1u32.to_le_bytes()); // v1
        d.extend(0x0140_0000u32.to_le_bytes()); // magic
        d.extend(1u32.to_le_bytes()); // total subs
        d.extend(1u32.to_le_bytes()); // entries
        d.extend(0u32.to_le_bytes()); // glyphs
        d.extend(0x0140_2000u32.to_le_bytes()); // sid
        d.extend(1u32.to_le_bytes()); // sub count
        d.extend((2u32 << 17).to_le_bytes()); // tag 2, offset 0
        d.extend(0u32.to_le_bytes());
        d.extend(3u32.to_le_bytes()); // data size
        d.extend(b"Hi\0");
        let (game, txt) = lng_to_txt(&d).unwrap();
        assert_eq!(game, LngGame::FinalExam);
        assert_eq!(
            txt,
            "### LANGUAGE\ngame = finalexam\nv1 = 1\nmagic = 0x01400000\nglyphs = \n###\n\n### ENTRY\nindex = 0\nsid = 0x01402000\n###\n[tag=0x0002] Hi\n\n"
        );
    }

    #[test]
    fn rejects_unknown_and_truncated() {
        assert!(lng_to_txt(&[1, 2, 3]).is_err());
        let mut d = Vec::new();
        d.extend(7u32.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        assert!(lng_to_txt(&d).is_err());
    }
}
