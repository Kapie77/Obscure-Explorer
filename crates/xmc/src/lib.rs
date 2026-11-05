//! Reader for Obscure 2 `.xmc` files (compiled XML) with an XML writer.
//!
//! Ported from Obscure-XMC-Converter (Kapie77), which is the same author's own tool.
//!
//! Layout (little-endian):
//!   header: node count u32, attribute count u32, string pool size u32
//!   nodes (16 bytes): tag CRC32 u32, parent i16, child count i16, first child i16,
//!                     attribute count i16, first attribute i16, padding u16
//!   attributes (8 bytes): name CRC32 u32, value offset u32 (into the string pool)
//!   string pool: NUL-terminated strings
//! Tag and attribute names are stored as CRC32 hashes; a dictionary (`hashes.json`) maps them back.

pub mod json;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

const NODE_SIZE: usize = 16;
const ATTR_SIZE: usize = 8;

/// CRC32 -> name.
pub type Dictionary = HashMap<u32, String>;

#[derive(Debug)]
pub struct XmcError(pub String);

impl fmt::Display for XmcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for XmcError {}

fn err<T>(msg: impl Into<String>) -> Result<T, XmcError> {
    Err(XmcError(msg.into()))
}

pub fn crc32(text: &str) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(text.as_bytes());
    h.finalize()
}

fn parse_crc_key(key: &str) -> Option<u32> {
    if let Some(hex) = key.strip_prefix("0x").or_else(|| key.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        key.parse::<u32>().ok()
    }
}

/// Accepts the formats of the converter: a JSON object (key = CRC as number / 0x hex, or a name),
/// a JSON array of names, or a text file with one name per line.
pub fn load_dictionary_str(content: &str) -> Dictionary {
    use json::Json;
    let mut map = Dictionary::new();
    match json::parse(content) {
        Some(Json::Object(entries)) => {
            for (key, value) in entries {
                if let Some(name) = value.as_str() {
                    let crc = parse_crc_key(&key).unwrap_or_else(|| crc32(&key));
                    map.insert(crc, name.to_owned());
                }
            }
        }
        Some(Json::Array(items)) => {
            for item in items {
                if let Json::Str(name) = item {
                    map.insert(crc32(&name), name);
                }
            }
        }
        _ => {
            for line in content.lines().map(str::trim).filter(|s| !s.is_empty()) {
                map.insert(crc32(line), line.to_owned());
            }
        }
    }
    map
}

pub fn load_dictionary(path: &Path) -> Result<Dictionary, XmcError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| XmcError(format!("could not read {}: {e}", path.display())))?;
    Ok(load_dictionary_str(&content))
}

#[derive(Clone, Debug)]
struct Node {
    tag_crc: u32,
    child_count: i16,
    first_child: i16,
    attr_count: i16,
    first_attr: i16,
}

#[derive(Clone, Debug)]
struct Attr {
    name_crc: u32,
    value_offset: u32,
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, XmcError> {
    match data.get(offset..offset + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => err("unexpected end of file reading u32"),
    }
}

fn read_i16(data: &[u8], offset: usize) -> Result<i16, XmcError> {
    match data.get(offset..offset + 2) {
        Some(b) => Ok(i16::from_le_bytes([b[0], b[1]])),
        None => err("unexpected end of file reading i16"),
    }
}

fn parse_xmc(data: &[u8]) -> Result<(Vec<Node>, Vec<Attr>, Vec<u8>), XmcError> {
    if data.len() < 12 {
        return err("file is too small to contain an XMC header");
    }
    let node_count = read_u32(data, 0)? as usize;
    let attr_count = read_u32(data, 4)? as usize;
    let string_size = read_u32(data, 8)? as usize;
    let node_bytes = node_count.checked_mul(NODE_SIZE).ok_or_else(|| XmcError("node count overflow".into()))?;
    let attr_bytes = attr_count.checked_mul(ATTR_SIZE).ok_or_else(|| XmcError("attribute count overflow".into()))?;
    let attr_start = 12usize.checked_add(node_bytes).ok_or_else(|| XmcError("file offset overflow".into()))?;
    let strings_start = attr_start.checked_add(attr_bytes).ok_or_else(|| XmcError("file offset overflow".into()))?;
    let end = strings_start.checked_add(string_size).ok_or_else(|| XmcError("file size overflow".into()))?;
    if end > data.len() {
        return err(format!(
            "XMC header counts exceed the file size (expected at least {end} bytes, got {})",
            data.len()
        ));
    }
    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let o = 12 + i * NODE_SIZE;
        nodes.push(Node {
            tag_crc: read_u32(data, o)?,
            child_count: read_i16(data, o + 6)?,
            first_child: read_i16(data, o + 8)?,
            attr_count: read_i16(data, o + 10)?,
            first_attr: read_i16(data, o + 12)?,
        });
    }
    let mut attrs = Vec::with_capacity(attr_count);
    for i in 0..attr_count {
        let o = attr_start + i * ATTR_SIZE;
        attrs.push(Attr { name_crc: read_u32(data, o)?, value_offset: read_u32(data, o + 4)? });
    }
    Ok((nodes, attrs, data[strings_start..end].to_vec()))
}

fn resolve_name(crc: u32, dict: &Dictionary, prefix: &str) -> String {
    dict.get(&crc).cloned().unwrap_or_else(|| format!("{prefix}_{crc:08X}"))
}

fn pool_string(pool: &[u8], offset: u32) -> String {
    let start = offset as usize;
    if start >= pool.len() {
        return String::new();
    }
    let end = pool[start..].iter().position(|b| *b == 0).map(|n| start + n).unwrap_or(pool.len());
    String::from_utf8_lossy(&pool[start..end]).into_owned()
}

/// Makes sure a name is a valid XML name.
fn xml_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':') { c } else { '_' })
        .collect();
    let first = s.chars().next();
    match first {
        None => s.push('_'),
        Some(c) if c.is_ascii_digit() || c == '-' || c == '.' => s.insert(0, '_'),
        _ => {}
    }
    s
}

fn escape_attr(value: &str) -> String {
    let mut o = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            '\t' => o.push_str("&#9;"),
            c if (c as u32) < 0x20 => o.push_str(&format!("&#{};", c as u32)),
            c => o.push(c),
        }
    }
    o
}

fn write_node(
    idx: usize,
    nodes: &[Node],
    attrs: &[Attr],
    pool: &[u8],
    dict: &Dictionary,
    depth: usize,
    out: &mut String,
) -> Result<(), XmcError> {
    if depth > nodes.len() {
        return err("possible circular reference in the XMC node tree");
    }
    let node = match nodes.get(idx) {
        Some(n) => n,
        None => return err("node index out of range"),
    };
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push('<');
    out.push_str(&xml_name(&resolve_name(node.tag_crc, dict, "tag")));
    if node.attr_count > 0 {
        if node.first_attr < 0 {
            return err(format!("node {idx} has attributes but an invalid first attribute"));
        }
        let mut seen: Vec<String> = Vec::new();
        for n in 0..node.attr_count as usize {
            let ai = node.first_attr as usize + n;
            let a = match attrs.get(ai) {
                Some(a) => a,
                None => return err(format!("attribute index {ai} out of range")),
            };
            let name = xml_name(&resolve_name(a.name_crc, dict, "attr"));
            if seen.contains(&name) {
                continue; // XML does not allow repeated attributes
            }
            out.push(' ');
            out.push_str(&name);
            out.push_str("=\"");
            out.push_str(&escape_attr(&pool_string(pool, a.value_offset)));
            out.push('"');
            seen.push(name);
        }
    }
    if node.child_count > 0 {
        if node.first_child < 0 {
            return err(format!("node {idx} has children but an invalid first child"));
        }
        out.push_str(">\n");
        for n in 0..node.child_count as usize {
            write_node(node.first_child as usize + n, nodes, attrs, pool, dict, depth + 1, out)?;
        }
        out.push_str(&indent);
        out.push_str("</");
        out.push_str(&xml_name(&resolve_name(node.tag_crc, dict, "tag")));
        out.push_str(">\n");
    } else {
        out.push_str(" />\n");
    }
    Ok(())
}

/// Converts an XMC file to indented XML text.
pub fn xmc_to_xml(data: &[u8], dict: &Dictionary) -> Result<String, XmcError> {
    let (nodes, attrs, pool) = parse_xmc(data)?;
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    if !nodes.is_empty() {
        write_node(0, &nodes, &attrs, &pool, dict, 0, &mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// <a k="v"><b /></a>
    fn sample() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(2u32.to_le_bytes()); // nodes
        d.extend(1u32.to_le_bytes()); // attributes
        d.extend(2u32.to_le_bytes()); // pool size ("v\0")
        // root: tag a, parent -1, 1 child at 1, 1 attribute at 0
        d.extend(crc32("a").to_le_bytes());
        d.extend((-1i16).to_le_bytes());
        d.extend(1i16.to_le_bytes());
        d.extend(1i16.to_le_bytes());
        d.extend(1i16.to_le_bytes());
        d.extend(0i16.to_le_bytes());
        d.extend(0u16.to_le_bytes());
        // child: tag b
        d.extend(crc32("b").to_le_bytes());
        d.extend(0i16.to_le_bytes());
        d.extend(0i16.to_le_bytes());
        d.extend((-1i16).to_le_bytes());
        d.extend(0i16.to_le_bytes());
        d.extend((-1i16).to_le_bytes());
        d.extend(0u16.to_le_bytes());
        // attribute k = "v"
        d.extend(crc32("k").to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(b"v\0");
        d
    }

    #[test]
    fn converts_with_a_dictionary() {
        let dict = load_dictionary_str("[\"a\", \"b\", \"k\"]");
        let xml = xmc_to_xml(&sample(), &dict).unwrap();
        assert_eq!(xml, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a k=\"v\">\n  <b />\n</a>\n");
    }

    #[test]
    fn unknown_names_use_their_crc() {
        let xml = xmc_to_xml(&sample(), &Dictionary::new()).unwrap();
        assert!(xml.contains("tag_"));
        assert!(xml.contains("attr_"));
    }

    #[test]
    fn dictionary_formats() {
        let d = load_dictionary_str("{\"0x00000010\": \"x\", \"12\": \"y\", \"name\": \"z\"}");
        assert_eq!(d.get(&0x10).map(String::as_str), Some("x"));
        assert_eq!(d.get(&12).map(String::as_str), Some("y"));
        assert_eq!(d.get(&crc32("name")).map(String::as_str), Some("z"));
        let lines = load_dictionary_str("one\ntwo\n");
        assert_eq!(lines.get(&crc32("two")).map(String::as_str), Some("two"));
    }

    #[test]
    fn rejects_truncated_files() {
        assert!(xmc_to_xml(&[0, 0, 0], &Dictionary::new()).is_err());
        let mut d = sample();
        d.truncate(d.len() - 4);
        assert!(xmc_to_xml(&d, &Dictionary::new()).is_err());
    }

    #[test]
    fn escapes_attribute_values() {
        assert_eq!(escape_attr("a<b&\"c\"\n"), "a&lt;b&amp;&quot;c&quot;&#10;");
    }
}
