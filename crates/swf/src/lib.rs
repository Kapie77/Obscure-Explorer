//! Reads the header and the tag list of Flash (`.swf`) files to show a summary of them.
//! (Playing is left to an external player; see the explorer.)
//!
//! Layout: signature `FWS` (plain), `CWS` (zlib after the first 8 bytes) or `ZWS` (LZMA),
//! version byte, u32 length, then RECT (stage size in twips), u16 frame rate (8.8 fixed),
//! u16 frame count and the tags (u16 header = code << 6 | length, long form 0x3F + u32 length).

use std::fmt;
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Zlib,
    Lzma,
}

#[derive(Clone, Debug, Default)]
pub struct SwfCounts {
    pub images: u32,
    pub sounds: u32,
    pub shapes: u32,
    pub sprites: u32,
    pub fonts: u32,
    pub texts: u32,
    pub scripts: u32,
    pub tags: u32,
}

#[derive(Clone, Debug)]
pub struct SwfInfo {
    pub compression: Compression,
    pub version: u8,
    /// Length of the file once unpacked, as declared in the header.
    pub unpacked_length: u32,
    /// Stage size in pixels (None when the header could not be read).
    pub stage: Option<(f32, f32)>,
    pub frame_rate: Option<f32>,
    pub frame_count: Option<u16>,
    /// From the FileAttributes tag (Flash 8+); None when the file has none.
    pub actionscript3: Option<bool>,
    pub counts: SwfCounts,
    /// False when the tags could not be listed (LZMA-compressed files).
    pub tags_listed: bool,
}

#[derive(Debug)]
pub struct SwfError(pub String);

impl fmt::Display for SwfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SwfError {}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Some(v)
    }

    fn read_signed(&mut self, n: u32) -> Option<i32> {
        let v = self.read(n)?;
        if n == 0 {
            return Some(0);
        }
        if v & (1 << (n - 1)) != 0 {
            Some((v as i64 - (1i64 << n)) as i32)
        } else {
            Some(v as i32)
        }
    }
}

fn inflate(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // what was unpacked before an error is still useful for the summary
    let _ = flate2::read::ZlibDecoder::new(body)
        .take(512 * 1024 * 1024)
        .read_to_end(&mut out);
    out
}

pub fn parse_swf(data: &[u8]) -> Result<SwfInfo, SwfError> {
    if data.len() < 8 {
        return Err(SwfError("the file is too small to be an SWF".into()));
    }
    let compression = match &data[0..3] {
        b"FWS" => Compression::None,
        b"CWS" => Compression::Zlib,
        b"ZWS" => Compression::Lzma,
        _ => return Err(SwfError("not an SWF file (unknown signature)".into())),
    };
    let mut info = SwfInfo {
        compression,
        version: data[3],
        unpacked_length: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
        stage: None,
        frame_rate: None,
        frame_count: None,
        actionscript3: None,
        counts: SwfCounts::default(),
        tags_listed: false,
    };
    let owned;
    let body: &[u8] = match compression {
        Compression::None => &data[8..],
        Compression::Zlib => {
            owned = inflate(&data[8..]);
            &owned
        }
        Compression::Lzma => return Ok(info),
    };

    // RECT: 5 bits for the field size, then xmin, xmax, ymin, ymax (signed, in twips)
    let mut bits = Bits { data: body, pos: 0 };
    let nbits = match bits.read(5) {
        Some(n) => n,
        None => return Ok(info),
    };
    let (xmin, xmax, ymin, ymax) = match (
        bits.read_signed(nbits),
        bits.read_signed(nbits),
        bits.read_signed(nbits),
        bits.read_signed(nbits),
    ) {
        (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
        _ => return Ok(info),
    };
    info.stage = Some(((xmax - xmin) as f32 / 20.0, (ymax - ymin) as f32 / 20.0));
    let mut pos = (5 + 4 * nbits as usize + 7) / 8;
    if pos + 4 > body.len() {
        return Ok(info);
    }
    info.frame_rate = Some(u16::from_le_bytes([body[pos], body[pos + 1]]) as f32 / 256.0);
    info.frame_count = Some(u16::from_le_bytes([body[pos + 2], body[pos + 3]]));
    pos += 4;

    // tags
    info.tags_listed = true;
    while pos + 2 <= body.len() {
        let h = u16::from_le_bytes([body[pos], body[pos + 1]]);
        pos += 2;
        let code = h >> 6;
        let mut len = (h & 0x3F) as usize;
        if len == 0x3F {
            if pos + 4 > body.len() {
                break;
            }
            len = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as usize;
            pos += 4;
        }
        if code == 0 {
            break; // End
        }
        info.counts.tags += 1;
        let c = &mut info.counts;
        match code {
            2 | 22 | 32 | 83 => c.shapes += 1,
            6 | 20 | 21 | 35 | 36 | 90 => c.images += 1,
            14 => c.sounds += 1,
            39 => c.sprites += 1,
            10 | 48 | 75 | 91 => c.fonts += 1,
            11 | 33 | 37 => c.texts += 1,
            12 | 59 | 82 => c.scripts += 1,
            69 => {
                if let Some(b) = body.get(pos) {
                    info.actionscript3 = Some(b & 0x08 != 0);
                }
            }
            _ => {}
        }
        pos = match pos.checked_add(len) {
            Some(p) => p,
            None => break,
        };
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes bits MSB first.
    struct BitWriter {
        bytes: Vec<u8>,
        bit: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self { bytes: Vec::new(), bit: 0 }
        }

        fn put(&mut self, value: u32, n: u32) {
            for i in (0..n).rev() {
                if self.bit % 8 == 0 {
                    self.bytes.push(0);
                }
                let b = ((value >> i) & 1) as u8;
                let last = self.bytes.len() - 1;
                self.bytes[last] |= b << (7 - (self.bit % 8));
                self.bit += 1;
            }
        }
    }

    /// 550 x 400 px, 24 fps, 30 frames, FileAttributes (AS3), one image, one sound, one script.
    fn body() -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(15, 5);
        for v in [0u32, 11000, 0, 8000] {
            w.put(v, 15);
        }
        let mut b = w.bytes;
        b.extend(0x1800u16.to_le_bytes()); // 24.0 fps
        b.extend(30u16.to_le_bytes());
        let tag = |code: u16, data: &[u8]| {
            let mut t = ((code << 6) | data.len() as u16).to_le_bytes().to_vec();
            t.extend_from_slice(data);
            t
        };
        b.extend(tag(69, &[0x08, 0, 0, 0]));
        b.extend(tag(21, &[1, 0]));
        b.extend(tag(14, &[1, 0]));
        b.extend(tag(82, &[0, 0, 0, 0]));
        b.extend(tag(1, &[]));
        b.extend(tag(0, &[]));
        b
    }

    fn check(info: &SwfInfo) {
        assert_eq!(info.version, 10);
        assert_eq!(info.stage, Some((550.0, 400.0)));
        assert_eq!(info.frame_rate, Some(24.0));
        assert_eq!(info.frame_count, Some(30));
        assert_eq!(info.actionscript3, Some(true));
        assert_eq!(info.counts.images, 1);
        assert_eq!(info.counts.sounds, 1);
        assert_eq!(info.counts.scripts, 1);
        assert_eq!(info.counts.tags, 5);
        assert!(info.tags_listed);
    }

    #[test]
    fn reads_a_plain_swf() {
        let b = body();
        let mut f = b"FWS".to_vec();
        f.push(10);
        f.extend((b.len() as u32 + 8).to_le_bytes());
        f.extend(b);
        let info = parse_swf(&f).unwrap();
        assert_eq!(info.compression, Compression::None);
        check(&info);
    }

    #[test]
    fn reads_a_zlib_swf() {
        let b = body();
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&b).unwrap();
        let packed = enc.finish().unwrap();
        let mut f = b"CWS".to_vec();
        f.push(10);
        f.extend((b.len() as u32 + 8).to_le_bytes());
        f.extend(packed);
        let info = parse_swf(&f).unwrap();
        assert_eq!(info.compression, Compression::Zlib);
        check(&info);
    }

    #[test]
    fn lzma_files_only_give_the_header() {
        let mut f = b"ZWS".to_vec();
        f.push(13);
        f.extend(1000u32.to_le_bytes());
        f.extend([0u8; 16]);
        let info = parse_swf(&f).unwrap();
        assert_eq!(info.compression, Compression::Lzma);
        assert_eq!(info.version, 13);
        assert!(!info.tags_listed);
        assert!(info.stage.is_none());
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse_swf(b"not flash at all").is_err());
        assert!(parse_swf(b"FWS").is_err());
    }
}
