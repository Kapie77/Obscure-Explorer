//! A tiny JSON reader: enough for `hashes.json` and the answer of the GitHub releases API.
//! It exists so the project needs no extra crate for this (builds can then work offline).

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    /// Numbers, true, false and null (never needed by this project).
    Other,
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The first value stored under `key` in an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

const MAX_DEPTH: usize = 128;

impl Parser<'_> {
    fn ws(&mut self) {
        while self.pos < self.s.len() && matches!(self.s[self.pos], b' ' | b'\t' | b'\r' | b'\n') {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn literal(&mut self, word: &[u8]) -> Option<Json> {
        if self.s[self.pos..].starts_with(word) {
            self.pos += word.len();
            Some(Json::Other)
        } else {
            None
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.s.get(self.pos..self.pos + 4)?;
        let mut v = 0u32;
        for d in digits {
            v = v * 16 + (*d as char).to_digit(16)?;
        }
        self.pos += 4;
        Some(v)
    }

    fn string(&mut self) -> Option<String> {
        if self.peek()? != b'"' {
            return None;
        }
        self.pos += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let b = self.peek()?;
            self.pos += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let e = self.peek()?;
                    self.pos += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{0008}',
                        b'f' => '\u{000C}',
                        b'/' => '/',
                        b'\\' => '\\',
                        b'"' => '"',
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xD800..0xDC00).contains(&hi) && self.s[self.pos..].starts_with(b"\\u") {
                                self.pos += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                                        .unwrap_or('\u{FFFD}')
                                } else {
                                    '\u{FFFD}'
                                }
                            } else {
                                char::from_u32(hi).unwrap_or('\u{FFFD}')
                            }
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(b),
            }
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match self.peek()? {
            b'"' => self.string().map(Json::Str),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek()? == b']' {
                    self.pos += 1;
                    return Some(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b']' => {
                            self.pos += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Some(Json::Array(items))
            }
            b'{' => {
                self.pos += 1;
                let mut entries = Vec::new();
                self.ws();
                if self.peek()? == b'}' {
                    self.pos += 1;
                    return Some(Json::Object(entries));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.ws();
                    if self.peek()? != b':' {
                        return None;
                    }
                    self.pos += 1;
                    let v = self.value(depth + 1)?;
                    entries.push((key, v));
                    self.ws();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b'}' => {
                            self.pos += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Some(Json::Object(entries))
            }
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            b'n' => self.literal(b"null"),
            b'-' | b'0'..=b'9' => {
                while self.pos < self.s.len()
                    && matches!(self.s[self.pos], b'+' | b'-' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    self.pos += 1;
                }
                Some(Json::Other)
            }
            _ => None,
        }
    }
}

/// Parses a whole JSON document; None when it is not valid JSON.
pub fn parse(text: &str) -> Option<Json> {
    let mut p = Parser { s: text.as_bytes(), pos: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.pos == p.s.len() {
        Some(v)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects_arrays_and_escapes() {
        let j = parse(" {\"a\":\"b\\u00e9\\n\",\"n\":[1,2.5e3,{\"x\":null}],\"t\":true,\"e\":{}} ").unwrap();
        assert_eq!(j.get("a").and_then(Json::as_str), Some("b\u{00E9}\n"));
        assert_eq!(j.get("n").and_then(Json::as_array).map(|a| a.len()), Some(3));
        assert_eq!(j.get("t"), Some(&Json::Other));
        assert_eq!(j.get("missing"), None);
    }

    #[test]
    fn surrogate_pairs_and_utf8() {
        let j = parse("[\"\\ud83d\\ude00\", \"caf\u{00E9}\"]").unwrap();
        let a = j.as_array().unwrap();
        assert_eq!(a[0].as_str(), Some("\u{1F600}"));
        assert_eq!(a[1].as_str(), Some("caf\u{00E9}"));
    }

    #[test]
    fn rejects_broken_json() {
        assert!(parse("{\"a\": }").is_none());
        assert!(parse("[1, 2").is_none());
        assert!(parse("plain text lines").is_none());
        assert!(parse("{\"a\":\"b\"} trailing").is_none());
    }
}
