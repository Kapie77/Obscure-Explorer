use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use crc32fast::Hasher;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use xmltree::{Element, EmitterConfig, XMLNode};

const NODE_SIZE: usize = 16;
const ATTR_SIZE: usize = 8;

/// Final Exam uses a different container: a 17-byte header, 20-byte nodes and
/// an encrypted string pool.
const FE_NODE_SIZE: usize = 20;
const FE_HEADER_SIZE: usize = 17;
const FE_MAGIC: u32 = 0x66;
const FE_VERSION: u8 = 3;
/// Seed mixed into the pool cipher state (`"BanI"` in little-endian).
const FE_POOL_KEY: u32 = 0x4961_6E42;
/// Markers written into the XML so `build` can pick the right container back.
const FE_FORMAT_MARKER: &str = "<!-- xmc-format: final-exam -->";
const XBOX360_FORMAT_MARKER: &str = "<!-- xmc-format: xbox360 -->";
const WII_FORMAT_MARKER: &str = "<!-- xmc-format: wii -->";

/// Container format of an `.xmc` file.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum XmcFormat {
    /// Detect from the file when extracting, or from the XML marker when building.
    Auto,
    /// Obscure 1/2 style container (little-endian); the PC, PS2 and PSP
    /// releases use this layout.
    #[value(alias = "pc", alias = "ps2", alias = "psp")]
    Obscure2,
    /// Obscure 2 on Wii: same layout as Obscure 2, but big-endian and with a
    /// 32-bit `first_attr` instead of the trailing 16-bit padding.
    Wii,
    /// Final Exam style container.
    FinalExam,
    /// Final Exam on Xbox 360: same container as Final Exam, but big-endian and
    /// with a 32-bit `first_attr` instead of the trailing 16-bit padding.
    #[value(name = "xbox360", alias = "x360", alias = "xbox")]
    Xbox360,
}

/// CRC of the unnamed attribute that holds the glyph character of a `<char>` node.
/// (hashes.json does not name it, so it shows up as `attr_0BB5AC54`.)
const FONT_CHAR_VALUE_CRC: u32 = 0x0BB5_AC54;

/// Text encoding used for the string pool.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum TextEncoding {
    /// Decode UTF-8 when the bytes are valid UTF-8, otherwise CP1252.
    /// Encode as CP1252 when possible, otherwise UTF-8.
    Auto,
    /// Always decode and encode CP1252 (the encoding used by the game data).
    Cp1252,
    /// Always decode and encode UTF-8.
    Utf8,
}

mod text {
    use super::TextEncoding;

    /// CP1252 code points for bytes 0x80..=0x9F. The five bytes CP1252 leaves
    /// undefined (0x81, 0x8D, 0x8F, 0x90, 0x9D) map to themselves so that
    /// decoding is lossless.
    const CP1252_HIGH: [u32; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];

    pub fn cp1252_decode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len());
        for &b in bytes {
            let cp = if (0x80..=0x9F).contains(&b) {
                CP1252_HIGH[(b - 0x80) as usize]
            } else {
                b as u32
            };
            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
        }
        out
    }

    fn cp1252_encode(s: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(s.len());
        for ch in s.chars() {
            let cp = ch as u32;
            let byte = if cp < 0x80 || (0xA0..=0xFF).contains(&cp) {
                cp as u8
            } else {
                (0x80 + CP1252_HIGH.iter().position(|&c| c == cp)?) as u8
            };
            out.push(byte);
        }
        Some(out)
    }

    pub fn decode(bytes: &[u8], mode: TextEncoding) -> String {
        match mode {
            TextEncoding::Utf8 => escape_controls(&String::from_utf8_lossy(bytes)),
            TextEncoding::Cp1252 => escape_controls(&cp1252_decode(bytes)),
            TextEncoding::Auto => match std::str::from_utf8(bytes) {
                Ok(s) => escape_controls(s),
                Err(_) => escape_controls(&cp1252_decode(bytes)),
            },
        }
    }

    /// XML 1.0 cannot represent C0 control characters (other than tab, LF and
    /// CR), so they are written as `\u0012`-style escapes and restored on build.
    fn is_xml_illegal(ch: char) -> bool {
        let c = ch as u32;
        c < 0x20 && c != 0x09 && c != 0x0A && c != 0x0D
    }

    fn escape_controls(s: &str) -> String {
        if !s.chars().any(is_xml_illegal) {
            return s.to_owned();
        }
        let mut out = String::with_capacity(s.len() + 8);
        for ch in s.chars() {
            if is_xml_illegal(ch) {
                out.push_str(&format!("\\u{:04X}", ch as u32));
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn unescape_controls(s: &str) -> String {
        if !s.contains("\\u") {
            return s.to_owned();
        }
        let chars: Vec<char> = s.chars().collect();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && i + 5 < chars.len() && chars[i + 1] == 'u' {
                let hex: String = chars[i + 2..i + 6].iter().collect();
                if let Ok(value) = u32::from_str_radix(&hex, 16) {
                    if value < 0x20 {
                        if let Some(ch) = char::from_u32(value) {
                            out.push(ch);
                            i += 6;
                            continue;
                        }
                    }
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    pub fn encode(s: &str, mode: TextEncoding) -> Vec<u8> {
        let s = unescape_controls(s);
        let s = s.as_str();
        if mode != TextEncoding::Utf8 {
            // The shipped font files store U+2122 (TM) as 0x84 0xA2, i.e. the
            // UTF-8 sequence E2 84 A2 with its lead byte missing. Reproduce the
            // game's own bytes so a rebuild stays byte-identical.
            if s == "\u{2122}" {
                return vec![0x84, 0xA2];
            }
            if let Some(bytes) = cp1252_encode(s) {
                return bytes;
            }
        }
        s.as_bytes().to_vec()
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "obscure-xmc-converter",
    version,
    about = "Convert between Obscure 1/2 and Final Exam XMC and XML",
    long_about = "Extract XMC files (Obscure 1/2 on PC, PS2, Wii and PSP, and Final Exam) to XML or build XMC files from XML."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Extract an XMC file to XML
    Extract {
        /// Input XMC files
        #[arg(required = true, value_name = "INPUT")]
        inputs: Vec<PathBuf>,

        /// Output file for one input, or output directory for multiple inputs
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// CRC dictionary JSON or text file
        #[arg(long = "dict")]
        dict_file: Option<PathBuf>,

        /// String pool encoding
        #[arg(long, value_enum, default_value_t = TextEncoding::Auto)]
        encoding: TextEncoding,

        /// Container format (auto detects the file layout)
        #[arg(long, value_enum, default_value_t = XmcFormat::Auto)]
        format: XmcFormat,

        /// Print additional binary information
        #[arg(long)]
        debug: bool,
    },

    /// Build an XMC file from XML
    Build {
        /// Input XML files
        #[arg(required = true, value_name = "INPUT")]
        inputs: Vec<PathBuf>,

        /// Output file for one input, or output directory for multiple inputs
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// CRC dictionary JSON or text file
        #[arg(long = "dict")]
        dict_file: Option<PathBuf>,

        /// String pool encoding
        #[arg(long, value_enum, default_value_t = TextEncoding::Auto)]
        encoding: TextEncoding,

        /// Container format (auto reads the marker written by extract, and
        /// falls back to the Obscure 1/2 layout)
        #[arg(long, value_enum, default_value_t = XmcFormat::Auto)]
        format: XmcFormat,

        /// Reuse an identical string that is already in the pool. The shipped
        /// files store one copy per attribute, so this is off by default.
        #[arg(long)]
        dedup: bool,

        /// Print additional binary information
        #[arg(long)]
        debug: bool,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Mode {
    Xmc2xml,
    Xml2xmc,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn u32(self, bytes: &[u8]) -> u32 {
        match self {
            Endian::Little => u32::from_le_bytes(bytes.try_into().unwrap()),
            Endian::Big => u32::from_be_bytes(bytes.try_into().unwrap()),
        }
    }

    fn i16(self, bytes: &[u8]) -> i16 {
        match self {
            Endian::Little => i16::from_le_bytes(bytes.try_into().unwrap()),
            Endian::Big => i16::from_be_bytes(bytes.try_into().unwrap()),
        }
    }

    fn u32_bytes(self, value: u32) -> [u8; 4] {
        match self {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        }
    }

    fn i16_bytes(self, value: i16) -> [u8; 2] {
        match self {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        }
    }

    fn u16_bytes(self, value: u16) -> [u8; 2] {
        match self {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        }
    }
}

#[derive(Clone, Debug)]
struct XmcNode {
    tag_crc: u32,
    parent_idx: i16,
    child_count: i16,
    first_child: i16,
    attr_count: i16,
    /// 16-bit in Obscure 1/2 and Final Exam, 32-bit in the Wii layout.
    first_attr: i32,
    /// Trailing `u16` padding of the 16-byte Obscure node (unused by Final Exam).
    pad: u16,
    /// Final Exam only: trailing `u32`, `0xFFFFFFFF` in every shipped file.
    extra: u32,
}

#[derive(Clone, Debug)]
struct XmcAttribute {
    name_crc: u32,
    value_offset: u32,
}

struct ParsedXmc {
    nodes: Vec<XmcNode>,
    attrs: Vec<XmcAttribute>,
    pool: Vec<u8>,
    format: XmcFormat,
    version: u8,
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data.get(offset..offset + 2).context("Unexpected end of file reading u16")?;
    Ok(u16::from_le_bytes(bytes.try_into()?))
}

fn read_endian_u32(data: &[u8], offset: usize, endian: Endian) -> Result<u32> {
    let bytes = data.get(offset..offset + 4).context("Unexpected end of file reading u32")?;
    Ok(endian.u32(bytes))
}

fn read_endian_i16(data: &[u8], offset: usize, endian: Endian) -> Result<i16> {
    let bytes = data.get(offset..offset + 2).context("Unexpected end of file reading i16")?;
    Ok(endian.i16(bytes))
}

/// Final Exam string pool cipher. `decrypt` reverses the in-place transform the
/// game applies right after reading the pool; the same keystream is used for
/// both directions, and it only depends on the three header counts.
fn fe_crypt_pool(
    bytes: &mut [u8],
    node_count: u32,
    attr_count: u32,
    pool_size: u32,
    decrypt: bool,
) {
    let mut state = node_count.wrapping_add(attr_count).wrapping_add(pool_size);
    let mut mix = state ^ FE_POOL_KEY;
    for byte in bytes.iter_mut() {
        state = state
            .wrapping_mul(0x1_0000)
            .wrapping_add(state >> 16)
            .wrapping_add(mix);
        let key = (state & 0xFF) as u8;
        *byte = if decrypt { byte.wrapping_sub(key) } else { byte.wrapping_add(key) };
        mix = mix.wrapping_add(state);
    }
}

/// The Final Exam header is 17 bytes: a `u32` magic, a `u8` version and three
/// 32-bit counts, one after the other. The PC build writes them little-endian
/// and the Xbox 360 build big-endian.
fn fe_counts(data: &[u8], endian: Endian) -> Result<(usize, usize, usize)> {
    let node_count = read_endian_u32(data, 5, endian)? as usize;
    let attr_count = read_endian_u32(data, 9, endian)? as usize;
    let pool_size = read_endian_u32(data, 13, endian)? as usize;
    Ok((node_count, attr_count, pool_size))
}

/// Byte length the Final Exam header counts describe, if they can be trusted.
fn fe_declared_len(data: &[u8], endian: Endian) -> Option<usize> {
    if data.len() < FE_HEADER_SIZE || read_endian_u32(data, 0, endian).ok()? != FE_MAGIC {
        return None;
    }
    let (node_count, attr_count, pool_size) = fe_counts(data, endian).ok()?;
    let end = FE_HEADER_SIZE
        .checked_add(node_count.checked_mul(FE_NODE_SIZE)?)?
        .checked_add(attr_count.checked_mul(ATTR_SIZE)?)?
        .checked_add(pool_size)?;
    if end > data.len() {
        return None;
    }
    Some(end)
}

fn looks_like_final_exam(data: &[u8]) -> bool {
    fe_declared_len(data, Endian::Little) == Some(data.len())
}

/// The Xbox 360 build of Final Exam stores the same container big-endian.
fn looks_like_xbox360(data: &[u8]) -> bool {
    fe_declared_len(data, Endian::Big) == Some(data.len())
        && fe_declared_len(data, Endian::Little) != Some(data.len())
}

/// Byte length the Obscure header counts describe, if they can be trusted.
fn obscure2_declared_len(data: &[u8], endian: Endian) -> Option<usize> {
    if data.len() < 12 {
        return None;
    }
    let node_count = read_endian_u32(data, 0, endian).ok()? as usize;
    let attr_count = read_endian_u32(data, 4, endian).ok()? as usize;
    let string_size = read_endian_u32(data, 8, endian).ok()? as usize;
    let end = 12usize
        .checked_add(node_count.checked_mul(NODE_SIZE)?)?
        .checked_add(attr_count.checked_mul(ATTR_SIZE)?)?
        .checked_add(string_size)?;
    if end > data.len() {
        return None;
    }
    Some(end)
}

/// The Wii build of Obscure 2 stores the same container big-endian.
fn looks_like_wii(data: &[u8]) -> bool {
    obscure2_declared_len(data, Endian::Big) == Some(data.len())
        && obscure2_declared_len(data, Endian::Little) != Some(data.len())
}

fn parse_obscure2(data: &[u8], endian: Endian) -> Result<ParsedXmc> {
    if data.len() < 12 {
        bail!("File is too small to contain an XMC header");
    }
    let node_count = read_endian_u32(data, 0, endian)? as usize;
    let attr_count = read_endian_u32(data, 4, endian)? as usize;
    let string_size = read_endian_u32(data, 8, endian)? as usize;

    let node_bytes = node_count.checked_mul(NODE_SIZE).context("Node count overflow")?;
    let attr_bytes = attr_count.checked_mul(ATTR_SIZE).context("Attribute count overflow")?;
    let attr_start = 12usize.checked_add(node_bytes).context("File offset overflow")?;
    let strings_start = attr_start.checked_add(attr_bytes).context("File offset overflow")?;
    let end = strings_start.checked_add(string_size).context("File size overflow")?;
    if end > data.len() {
        bail!("XMC header counts exceed file size (expected at least {end} bytes, got {})", data.len());
    }

    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let o = 12 + i * NODE_SIZE;
        let (first_attr, pad) = match endian {
            // The Wii build widens `first_attr` to 32 bits and drops the padding.
            Endian::Big => (read_endian_u32(data, o + 12, endian)? as i32, 0),
            Endian::Little => (read_endian_i16(data, o + 12, endian)? as i32, read_u16(data, o + 14)?),
        };
        nodes.push(XmcNode {
            tag_crc: read_endian_u32(data, o, endian)?,
            parent_idx: read_endian_i16(data, o + 4, endian)?,
            child_count: read_endian_i16(data, o + 6, endian)?,
            first_child: read_endian_i16(data, o + 8, endian)?,
            attr_count: read_endian_i16(data, o + 10, endian)?,
            first_attr,
            pad,
            extra: 0,
        });
    }

    let mut attrs = Vec::with_capacity(attr_count);
    for i in 0..attr_count {
        let o = attr_start + i * ATTR_SIZE;
        attrs.push(XmcAttribute {
            name_crc: read_endian_u32(data, o, endian)?,
            value_offset: read_endian_u32(data, o + 4, endian)?,
        });
    }

    Ok(ParsedXmc {
        nodes,
        attrs,
        pool: data[strings_start..end].to_vec(),
        format: match endian {
            Endian::Little => XmcFormat::Obscure2,
            Endian::Big => XmcFormat::Wii,
        },
        version: 0,
    })
}

fn parse_final_exam(data: &[u8], endian: Endian) -> Result<ParsedXmc> {
    if data.len() < FE_HEADER_SIZE {
        bail!("File is too small to contain a Final Exam XMC header");
    }
    let magic = read_endian_u32(data, 0, endian)?;
    if magic != FE_MAGIC {
        bail!("Not a Final Exam XMC file (magic {magic:#x})");
    }
    let version = *data.get(4).context("Unexpected end of file reading version")?;
    let (node_count, attr_count, pool_size) = fe_counts(data, endian)?;

    let node_start = FE_HEADER_SIZE;
    let attr_start = node_start
        .checked_add(node_count.checked_mul(FE_NODE_SIZE).context("Node count overflow")?)
        .context("File offset overflow")?;
    let pool_start = attr_start
        .checked_add(attr_count.checked_mul(ATTR_SIZE).context("Attribute count overflow")?)
        .context("File offset overflow")?;
    let end = pool_start.checked_add(pool_size).context("File size overflow")?;
    if end > data.len() {
        bail!(
            "Final Exam XMC header counts exceed file size (expected at least {end} bytes, got {})",
            data.len()
        );
    }

    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let o = node_start + i * FE_NODE_SIZE;
        let (first_attr, pad) = match endian {
            // The Xbox 360 build widens `first_attr` to 32 bits and drops the
            // padding, just like the Wii build of Obscure 2.
            Endian::Big => (read_endian_u32(data, o + 12, endian)? as i32, 0),
            Endian::Little => (read_endian_i16(data, o + 12, endian)? as i32, read_u16(data, o + 14)?),
        };
        nodes.push(XmcNode {
            tag_crc: read_endian_u32(data, o, endian)?,
            parent_idx: read_endian_i16(data, o + 4, endian)?,
            child_count: read_endian_i16(data, o + 6, endian)?,
            first_child: read_endian_i16(data, o + 8, endian)?,
            attr_count: read_endian_i16(data, o + 10, endian)?,
            first_attr,
            pad,
            extra: read_endian_u32(data, o + 16, endian)?,
        });
    }

    let mut attrs = Vec::with_capacity(attr_count);
    for i in 0..attr_count {
        let o = attr_start + i * ATTR_SIZE;
        attrs.push(XmcAttribute {
            name_crc: read_endian_u32(data, o, endian)?,
            value_offset: read_endian_u32(data, o + 4, endian)?,
        });
    }

    let mut pool = data[pool_start..end].to_vec();
    fe_crypt_pool(
        &mut pool,
        node_count as u32,
        attr_count as u32,
        pool_size as u32,
        true,
    );

    Ok(ParsedXmc {
        nodes,
        attrs,
        pool,
        format: match endian {
            Endian::Little => XmcFormat::FinalExam,
            Endian::Big => XmcFormat::Xbox360,
        },
        version,
    })
}

fn parse_xmc(data: &[u8], requested: XmcFormat) -> Result<ParsedXmc> {
    match requested {
        XmcFormat::FinalExam => parse_final_exam(data, Endian::Little),
        XmcFormat::Xbox360 => parse_final_exam(data, Endian::Big),
        XmcFormat::Obscure2 => parse_obscure2(data, Endian::Little),
        XmcFormat::Wii => parse_obscure2(data, Endian::Big),
        XmcFormat::Auto => {
            if looks_like_final_exam(data) {
                parse_final_exam(data, Endian::Little)
            } else if looks_like_xbox360(data) {
                parse_final_exam(data, Endian::Big)
            } else if looks_like_wii(data) {
                parse_obscure2(data, Endian::Big)
            } else {
                parse_obscure2(data, Endian::Little)
            }
        }
    }
}

fn crc32(text: &str) -> u32 {
    let mut h = Hasher::new();
    h.update(text.as_bytes());
    h.finalize()
}

fn load_crc_dictionary(path: &Path) -> Result<HashMap<u32, String>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Could not read dictionary {}", path.display()))?;
    let mut map = HashMap::new();

    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Object(obj)) => {
            for (key, value) in obj {
                let name = match value.as_str() {
                    Some(s) => s.to_owned(),
                    None => continue,
                };
                let crc = parse_crc_key(&key).unwrap_or_else(|| crc32(&key));
                // CRC 0 is crc32("") and has no valid XML name; the shipped
                // dictionary maps it to "data", which collides with the real
                // "data" attribute. Skip it so CRC 0 keeps its placeholder.
                if crc != 0 {
                    map.insert(crc, name);
                }
            }
        }
        Ok(Value::Array(items)) => {
            for item in items {
                if let Some(name) = item.as_str() {
                    map.insert(crc32(name), name.to_owned());
                }
            }
        }
        _ => {
            for line in content.lines().map(str::trim).filter(|s| !s.is_empty()) {
                map.insert(crc32(line), line.to_owned());
            }
        }
    }
    println!("Loaded {} CRC dictionary entries", map.len());
    Ok(map)
}

fn parse_crc_key(key: &str) -> Option<u32> {
    if let Some(hex) = key.strip_prefix("0x").or_else(|| key.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        key.parse::<u32>().ok()
    }
}

fn resolve_name(crc: u32, dict: &HashMap<u32, String>, prefix: &str) -> String {
    dict.get(&crc).cloned().unwrap_or_else(|| format!("{prefix}_{crc:08X}"))
}

fn pool_bytes(pool: &[u8], offset: u32) -> &[u8] {
    let start = offset as usize;
    if start >= pool.len() {
        return &[];
    }
    let end = pool[start..].iter().position(|b| *b == 0)
        .map(|n| start + n).unwrap_or(pool.len());
    &pool[start..end]
}

/// The `<char>` glyph value is a copy of the glyph character, but some shipped
/// font files store characters above U+07FF as the UTF-8 sequence with the lead
/// byte stripped (U+2122 is stored as `84 A2` instead of `E2 84 A2`). When the
/// stored bytes are exactly the tail of the UTF-8 encoding of the node's `code`
/// attribute, the code point is authoritative and is used instead.
fn repair_char_value(raw: &[u8], decoded: String, code: Option<u32>) -> String {
    let Some(code) = code else {
        return decoded;
    };
    if decoded.chars().count() == 1 {
        return decoded;
    }
    let Some(ch) = char::from_u32(code) else {
        return decoded;
    };
    let mut buf = [0u8; 4];
    let utf8 = ch.encode_utf8(&mut buf).as_bytes();
    if utf8.len() > 2 && raw == &utf8[1..] {
        return ch.to_string();
    }
    decoded
}

fn node_code(raw_attrs: &[(u32, &[u8])]) -> Option<u32> {
    let code_crc = crc32("code");
    raw_attrs.iter()
        .find(|(crc, _)| *crc == code_crc)
        .and_then(|(_, raw)| std::str::from_utf8(raw).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
}

fn element_from_node(
    idx: usize,
    nodes: &[XmcNode],
    attrs: &[XmcAttribute],
    pool: &[u8],
    dict: &HashMap<u32, String>,
    encoding: TextEncoding,
    depth: usize,
) -> Result<Element> {
    if depth > nodes.len() {
        bail!("Possible circular reference in XMC node tree");
    }
    let node = nodes.get(idx).context("Node index out of range")?;
    let mut elem = Element::new(&resolve_name(node.tag_crc, dict, "tag"));

    if node.attr_count > 0 {
        if node.first_attr < 0 {
            bail!("Node {idx} has attributes but an invalid first_attr");
        }
        let mut raw_attrs: Vec<(u32, &[u8])> = Vec::with_capacity(node.attr_count as usize);
        for n in 0..node.attr_count as usize {
            let ai = node.first_attr as usize + n;
            let attr = attrs.get(ai).with_context(|| format!("Attribute index {ai} out of range"))?;
            raw_attrs.push((attr.name_crc, pool_bytes(pool, attr.value_offset)));
        }
        let code = node_code(&raw_attrs);
        for (name_crc, raw) in raw_attrs {
            let mut value = text::decode(raw, encoding);
            if name_crc == FONT_CHAR_VALUE_CRC {
                value = repair_char_value(raw, value, code);
            }
            elem.attributes.insert(resolve_name(name_crc, dict, "attr"), value);
        }
    }

    if node.child_count > 0 {
        if node.first_child < 0 {
            bail!("Node {idx} has children but an invalid first_child");
        }
        for n in 0..node.child_count as usize {
            let child_idx = node.first_child as usize + n;
            elem.children.push(XMLNode::Element(element_from_node(
                child_idx, nodes, attrs, pool, dict, encoding, depth + 1,
            )?));
        }
    }
    Ok(elem)
}

fn xmc_to_xml(
    input: &Path,
    output: &Path,
    dict: &HashMap<u32, String>,
    encoding: TextEncoding,
    format: XmcFormat,
    debug: bool,
) -> Result<()> {
    let data = fs::read(input).with_context(|| format!("Could not read {}", input.display()))?;
    let parsed = parse_xmc(&data, format)?;
    let (nodes, attrs, pool) = (&parsed.nodes, &parsed.attrs, &parsed.pool);
    if debug {
        println!(
            "Format: {:?}, header: nodes={}, attrs={}, strings={} bytes, version={}",
            parsed.format,
            nodes.len(),
            attrs.len(),
            pool.len(),
            parsed.version
        );
        for (i, node) in nodes.iter().take(10).enumerate() {
            println!("node[{i}]: {node:?}");
        }
    }

    let mut xml = Vec::new();
    xml.extend_from_slice(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    match parsed.format {
        XmcFormat::FinalExam => {
            xml.extend_from_slice(FE_FORMAT_MARKER.as_bytes());
            xml.push(b'\n');
        }
        XmcFormat::Xbox360 => {
            xml.extend_from_slice(XBOX360_FORMAT_MARKER.as_bytes());
            xml.push(b'\n');
        }
        XmcFormat::Wii => {
            xml.extend_from_slice(WII_FORMAT_MARKER.as_bytes());
            xml.push(b'\n');
        }
        _ => {}
    }
    if !nodes.is_empty() {
        let root = element_from_node(0, nodes, attrs, pool, dict, encoding, 0)?;
        root.write_with_config(
            &mut xml,
            EmitterConfig::new().perform_indent(true).write_document_declaration(false),
        )?;
        xml.push(b'\n');
    }
    fs::write(output, xml).with_context(|| format!("Could not write {}", output.display()))?;
    println!("Converted XMC -> XML: {}", output.display());
    Ok(())
}

fn name_to_crc(name: &str, reverse: &HashMap<String, u32>) -> u32 {
    if let Some(crc) = reverse.get(name) {
        return *crc;
    }
    for prefix in ["tag_", "attr_"] {
        if let Some(hex) = name.strip_prefix(prefix) {
            if hex.len() == 8 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(value) = u32::from_str_radix(hex, 16) {
                    return value;
                }
            }
        }
    }
    crc32(name)
}

fn validate_i16(value: usize, field: &str) -> Result<i16> {
    if value > i16::MAX as usize {
        bail!("{field} exceeds the supported i16 range: {value}");
    }
    Ok(value as i16)
}

fn intern_value(
    value: &str,
    pool: &mut Vec<u8>,
    string_offsets: &mut HashMap<String, u32>,
    encoding: TextEncoding,
    dedup: bool,
) -> Result<u32> {
    if dedup {
        if let Some(offset) = string_offsets.get(value) {
            return Ok(*offset);
        }
    }
    let offset = u32::try_from(pool.len()).context("String pool exceeds u32 range")?;
    pool.extend_from_slice(&text::encode(value, encoding));
    pool.push(0);
    if dedup {
        string_offsets.insert(value.to_owned(), offset);
    }
    Ok(offset)
}

fn new_node(tag_crc: u32, parent: i16) -> XmcNode {
    // The shipped files store 0 in the "absent" index fields (root parent,
    // leaf first_child, node first_attr) instead of -1.
    XmcNode {
        tag_crc,
        parent_idx: parent,
        child_count: 0,
        first_child: 0,
        attr_count: 0,
        first_attr: 0,
        pad: 0,
        extra: 0xFFFF_FFFF,
    }
}

fn build_element(
    element: &Element,
    parent: i16,
    nodes: &mut Vec<XmcNode>,
    attrs: &mut Vec<XmcAttribute>,
    pool: &mut Vec<u8>,
    string_offsets: &mut HashMap<String, u32>,
    reverse: &HashMap<String, u32>,
    encoding: TextEncoding,
    dedup: bool,
) -> Result<usize> {
    let idx = nodes.len();
    nodes.push(new_node(name_to_crc(&element.name, reverse), parent));

    let first_attr = attrs.len();
    for (name, value) in &element.attributes {
        let offset = intern_value(value, pool, string_offsets, encoding, dedup)?;
        attrs.push(XmcAttribute { name_crc: name_to_crc(name, reverse), value_offset: offset });
    }
    let attr_count = element.attributes.len();
    nodes[idx].attr_count = validate_i16(attr_count, "attribute count")?;
    if attr_count > 0 {
        nodes[idx].first_attr = validate_i16(first_attr, "first attribute index")? as i32;
    }

    let child_elements: Vec<&Element> = element.children.iter().filter_map(|n| {
        if let XMLNode::Element(e) = n { Some(e) } else { None }
    }).collect();

    let mut child_indices = Vec::with_capacity(child_elements.len());
    for child in &child_elements {
        // Reserve all siblings first so child indices stay contiguous, as XMC expects.
        let child_idx = nodes.len();
        nodes.push(new_node(
            name_to_crc(&child.name, reverse),
            validate_i16(idx, "parent index")?,
        ));
        child_indices.push(child_idx);
    }
    nodes[idx].child_count = validate_i16(child_indices.len(), "child count")?;
    if let Some(first) = child_indices.first() {
        nodes[idx].first_child = validate_i16(*first, "first child index")?;
    }

    for (child, child_idx) in child_elements.iter().zip(child_indices.iter()) {
        // Build attributes and descendants for the already-reserved node.
        populate_reserved_child(
            child, *child_idx, nodes, attrs, pool, string_offsets, reverse, encoding, dedup,
        )?;
    }
    Ok(idx)
}

fn populate_reserved_child(
    element: &Element,
    idx: usize,
    nodes: &mut Vec<XmcNode>,
    attrs: &mut Vec<XmcAttribute>,
    pool: &mut Vec<u8>,
    string_offsets: &mut HashMap<String, u32>,
    reverse: &HashMap<String, u32>,
    encoding: TextEncoding,
    dedup: bool,
) -> Result<()> {
    // Build this node's attributes and children in a helper, without adding another root node.
    let first_attr = attrs.len();
    for (name, value) in &element.attributes {
        let offset = intern_value(value, pool, string_offsets, encoding, dedup)?;
        attrs.push(XmcAttribute { name_crc: name_to_crc(name, reverse), value_offset: offset });
    }
    nodes[idx].attr_count = validate_i16(element.attributes.len(), "attribute count")?;
    if !element.attributes.is_empty() {
        nodes[idx].first_attr = validate_i16(first_attr, "first attribute index")? as i32;
    }

    let children: Vec<&Element> = element.children.iter().filter_map(|n| {
        if let XMLNode::Element(e) = n { Some(e) } else { None }
    }).collect();
    let mut indices = Vec::new();
    for child in &children {
        let child_idx = nodes.len();
        nodes.push(new_node(
            name_to_crc(&child.name, reverse),
            validate_i16(idx, "parent index")?,
        ));
        indices.push(child_idx);
    }
    nodes[idx].child_count = validate_i16(indices.len(), "child count")?;
    if let Some(first) = indices.first() {
        nodes[idx].first_child = validate_i16(*first, "first child index")?;
    }
    for (child, child_idx) in children.iter().zip(indices.iter()) {
        populate_reserved_child(child, *child_idx, nodes, attrs, pool, string_offsets, reverse, encoding, dedup)?;
    }
    Ok(())
}

fn xml_to_xmc(
    input: &Path,
    output: &Path,
    dict: &HashMap<u32, String>,
    encoding: TextEncoding,
    format: XmcFormat,
    dedup: bool,
    debug: bool,
) -> Result<()> {
    let text = fs::read_to_string(input)
        .with_context(|| format!("Could not read {}", input.display()))?;
    let root = Element::parse(text.as_bytes())
        .with_context(|| format!("Could not parse XML {}", input.display()))?;

    // `auto` follows the marker written by `extract`, and keeps the historical
    // Obscure layout for XML files that do not carry one.
    let format = match format {
        XmcFormat::Auto => {
            if text.contains(FE_FORMAT_MARKER) {
                XmcFormat::FinalExam
            } else if text.contains(XBOX360_FORMAT_MARKER) {
                XmcFormat::Xbox360
            } else if text.contains(WII_FORMAT_MARKER) {
                XmcFormat::Wii
            } else {
                XmcFormat::Obscure2
            }
        }
        explicit => explicit,
    };

    let mut reverse = HashMap::new();
    for (crc, name) in dict {
        reverse.insert(name.clone(), *crc);
    }

    let mut nodes = Vec::new();
    let mut attrs = Vec::new();
    let mut pool = Vec::new();
    let mut string_offsets = HashMap::new();

    build_element(
        &root, 0, &mut nodes, &mut attrs, &mut pool, &mut string_offsets, &reverse, encoding, dedup,
    )?;

    let output_bytes = match format {
        XmcFormat::FinalExam => encode_final_exam(&nodes, &attrs, pool, Endian::Little, debug)?,
        XmcFormat::Xbox360 => encode_final_exam(&nodes, &attrs, pool, Endian::Big, debug)?,
        XmcFormat::Wii => encode_obscure2(&nodes, &attrs, pool, Endian::Big, debug)?,
        _ => encode_obscure2(&nodes, &attrs, pool, Endian::Little, debug)?,
    };

    if debug {
        println!(
            "Format: {:?}, nodes={}, attrs={}",
            format,
            nodes.len(),
            attrs.len()
        );
    }
    fs::write(output, &output_bytes).with_context(|| format!("Could not write {}", output.display()))?;
    println!("Converted XML -> XMC: {} ({} bytes)", output.display(), output_bytes.len());
    Ok(())
}

fn encode_obscure2(
    nodes: &[XmcNode],
    attrs: &[XmcAttribute],
    mut pool: Vec<u8>,
    endian: Endian,
    debug: bool,
) -> Result<Vec<u8>> {
    // The shipped pools (PC, PS2, PSP and Wii) are zero-padded to a 16-byte
    // boundary. The padding is not referenced by any attribute, but keeping it
    // makes a rebuilt file match the shipped layout byte for byte whenever the
    // original did not use string sharing.
    let padded = (pool.len() + 15) / 16 * 16;
    pool.resize(padded, 0);

    let node_count = u32::try_from(nodes.len()).context("Too many nodes")?;
    let attr_count = u32::try_from(attrs.len()).context("Too many attributes")?;
    let pool_size = u32::try_from(pool.len()).context("String pool too large")?;

    let mut output_bytes = Vec::with_capacity(12 + nodes.len() * NODE_SIZE + attrs.len() * ATTR_SIZE + pool.len());
    output_bytes.extend_from_slice(&endian.u32_bytes(node_count));
    output_bytes.extend_from_slice(&endian.u32_bytes(attr_count));
    output_bytes.extend_from_slice(&endian.u32_bytes(pool_size));

    for node in nodes {
        output_bytes.extend_from_slice(&endian.u32_bytes(node.tag_crc));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.parent_idx));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.child_count));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.first_child));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.attr_count));
        match endian {
            // The Wii build stores `first_attr` as a full 32-bit field.
            Endian::Big => output_bytes.extend_from_slice(&endian.u32_bytes(node.first_attr as u32)),
            Endian::Little => {
                output_bytes.extend_from_slice(&endian.i16_bytes(node.first_attr as i16));
                output_bytes.extend_from_slice(&endian.u16_bytes(node.pad));
            }
        }
    }
    for attr in attrs {
        output_bytes.extend_from_slice(&endian.u32_bytes(attr.name_crc));
        output_bytes.extend_from_slice(&endian.u32_bytes(attr.value_offset));
    }
    output_bytes.extend_from_slice(&pool);

    if debug {
        println!("Debug: nodes={}, attrs={}, string_pool={} bytes", nodes.len(), attrs.len(), pool.len());
    }
    Ok(output_bytes)
}

fn encode_final_exam(
    nodes: &[XmcNode],
    attrs: &[XmcAttribute],
    mut pool: Vec<u8>,
    endian: Endian,
    debug: bool,
) -> Result<Vec<u8>> {
    // The shipped pools are zero-padded to a 16-byte boundary, and the padding
    // is part of the encrypted region.
    let padded = (pool.len() + 15) / 16 * 16;
    pool.resize(padded, 0);

    let node_count = u32::try_from(nodes.len()).context("Too many nodes")?;
    let attr_count = u32::try_from(attrs.len()).context("Too many attributes")?;
    let pool_size = u32::try_from(pool.len()).context("String pool too large")?;
    // The PC build's game code reads the counts as 24-bit values. The Xbox 360
    // build stores full 32-bit counts, so only the PC build needs the limit.
    if endian == Endian::Little {
        for (value, field) in [
            (node_count, "node count"),
            (attr_count, "attribute count"),
            (pool_size, "string pool size"),
        ] {
            if value >= 1 << 24 {
                bail!("{field} does not fit the 24-bit Final Exam header field: {value}");
            }
        }
    }

    fe_crypt_pool(&mut pool, node_count, attr_count, pool_size, false);

    let mut output_bytes = Vec::with_capacity(
        FE_HEADER_SIZE + nodes.len() * FE_NODE_SIZE + attrs.len() * ATTR_SIZE + pool.len(),
    );
    output_bytes.extend_from_slice(&endian.u32_bytes(FE_MAGIC));
    output_bytes.push(FE_VERSION);
    output_bytes.extend_from_slice(&endian.u32_bytes(node_count));
    output_bytes.extend_from_slice(&endian.u32_bytes(attr_count));
    output_bytes.extend_from_slice(&endian.u32_bytes(pool_size));

    for node in nodes {
        output_bytes.extend_from_slice(&endian.u32_bytes(node.tag_crc));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.parent_idx));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.child_count));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.first_child));
        output_bytes.extend_from_slice(&endian.i16_bytes(node.attr_count));
        match endian {
            // The Xbox 360 build stores `first_attr` as a full 32-bit field.
            Endian::Big => output_bytes.extend_from_slice(&endian.u32_bytes(node.first_attr as u32)),
            Endian::Little => {
                output_bytes.extend_from_slice(&endian.i16_bytes(node.first_attr as i16));
                output_bytes.extend_from_slice(&endian.u16_bytes(node.pad));
            }
        }
        output_bytes.extend_from_slice(&endian.u32_bytes(node.extra));
    }
    for attr in attrs {
        output_bytes.extend_from_slice(&endian.u32_bytes(attr.name_crc));
        output_bytes.extend_from_slice(&endian.u32_bytes(attr.value_offset));
    }
    output_bytes.extend_from_slice(&pool);

    if debug {
        println!(
            "Debug: nodes={}, attrs={}, string_pool={} bytes (padded)",
            nodes.len(),
            attrs.len(),
            pool_size
        );
    }
    Ok(output_bytes)
}

fn resolve_default_dict() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let candidates = [
        exe_dir.join("hashes.json"),
        exe_dir.join("utils").join("hashes.json"),
        exe_dir.join("zwoLib").join("utils").join("hashes.json"),
        PathBuf::from("hashes.json"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn output_path(source: &Path, mode: Mode, override_path: Option<&Path>, batch: bool) -> PathBuf {
    let extension = if mode == Mode::Xmc2xml { "xml" } else { "xmc" };
    let Some(out) = override_path else {
        return source.with_extension(extension);
    };
    if batch || out.is_dir() || out.extension().is_none() {
        let _ = fs::create_dir_all(out);
        return out.join(source.file_stem().unwrap_or_default()).with_extension(extension);
    }
    out.to_path_buf()
}


fn parse_args_with_implicit_mode() -> Args {
    use std::ffi::OsString;

    let raw: Vec<OsString> = std::env::args_os().collect();

    let first = raw.get(1).and_then(|arg| arg.to_str());

    // Preserve explicit commands and normal help/version requests.
    if matches!(
        first,
        Some("extract" | "build" | "--help" | "-h" | "--version" | "-V")
    ) {
        return Args::parse_from(raw);
    }

    let mut xmc_count = 0usize;
    let mut xml_count = 0usize;
    let mut skip_next = false;

    for arg in raw.iter().skip(1) {
        let Some(value) = arg.to_str() else {
            continue;
        };

        // These options take a value that must not be treated as input.
        if skip_next {
            skip_next = false;
            continue;
        }

        if matches!(value, "--dict" | "-o" | "--output" | "--format" | "--encoding") {
            skip_next = true;
            continue;
        }

        // Also support --dict=... and --output=...
        if value.starts_with("--dict=")
            || value.starts_with("--output=")
            || value.starts_with("--format=")
            || value.starts_with("--encoding=")
            || value.starts_with("-o=")
        {
            continue;
        }

        // Ignore other command-line options.
        if value.starts_with('-') {
            continue;
        }

        let extension = std::path::Path::new(value)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        match extension.as_str() {
            "xmc" => xmc_count += 1,
            "xml" => xml_count += 1,
            _ => {}
        }
    }

    if xmc_count == 0 && xml_count == 0 {
        eprintln!(
            "No .xmc or .xml input detected. \
             Use 'extract' for XMC files or 'build' for XML files."
        );
        std::process::exit(2);
    }

    // Preserve the old automatic behavior: XMC wins a tie.
    let command = if xml_count > xmc_count {
        "build"
    } else {
        "extract"
    };

    let mut forwarded = Vec::with_capacity(raw.len() + 1);
    forwarded.push(raw[0].clone());
    forwarded.push(command.into());
    forwarded.extend(raw.into_iter().skip(1));

    Args::parse_from(forwarded)
}

fn main() -> Result<()> {
    let args = parse_args_with_implicit_mode();

    let (inputs, output, dict_file, encoding, format, dedup, debug, mode) = match args.command {
        Command::Extract {
            inputs,
            output,
            dict_file,
            encoding,
            format,
            debug,
        } => (inputs, output, dict_file, encoding, format, false, debug, Mode::Xmc2xml),

        Command::Build {
            inputs,
            output,
            dict_file,
            encoding,
            format,
            dedup,
            debug,
        } => (inputs, output, dict_file, encoding, format, dedup, debug, Mode::Xml2xmc),
    };

    let dict_path = dict_file.or_else(resolve_default_dict);

    let dict = match dict_path {
        Some(path) => load_crc_dictionary(&path)
            .with_context(|| format!("Failed to load dictionary {}", path.display()))?,
        None => {
            eprintln!(
                "Warning: hashes.json not found; unknown names will use CRC32."
            );
            HashMap::new()
        }
    };

    let expected_extension = match mode {
        Mode::Xmc2xml => "xmc",
        Mode::Xml2xmc => "xml",
    };

    let mut sources = Vec::new();
    let mut ignored = Vec::new();
    let mut missing = Vec::new();

    for path in &inputs {
        if !path.exists() {
            missing.push(path.clone());
            continue;
        }

        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if extension == expected_extension {
            sources.push(path.clone());
        } else {
            ignored.push(path.clone());
        }
    }

    for path in missing {
        eprintln!("Skipping missing file: {}", path.display());
    }

    for path in ignored {
        eprintln!(
            "Ignoring file with unsupported extension for this command: {}",
            path.display()
        );
    }

    if sources.is_empty() {
        bail!(
            "No compatible input files found. Expected .{expected_extension} files."
        );
    }

    let batch = sources.len() > 1;

    for source in &sources {
        let destination = output_path(
            source,
            mode,
            output.as_deref(),
            batch,
        );

        match mode {
            Mode::Xmc2xml => {
                xmc_to_xml(source, &destination, &dict, encoding, format, debug)?;
            }
            Mode::Xml2xmc => {
                xml_to_xmc(source, &destination, &dict, encoding, format, dedup, debug)?;
            }
        }
    }

    if batch {
        println!(
            "Processed {} files in mode {:?}",
            sources.len(),
            mode
        );
    }

    Ok(())
}