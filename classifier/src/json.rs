//! A small JSON reader (RFC 8259) for Arm's AARCHMRS files.
//!
//! Written here rather than taken from a crate: the workspace has no
//! dependency, and the reader needs one feature a general parser lacks: the
//! values of some keys (Arm's access rules, descriptions) are skipped
//! without being built, which keeps the 214 MB of the two files at a few
//! hundred MB in memory.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Numbers are kept as written: the files hold only small integers.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

#[derive(Debug)]
pub struct Error {
    pub offset: usize,
    pub what: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON error at byte {}: {}", self.offset, self.what)
    }
}

impl Json {
    /// The value of `key` in an object, or `Null`.
    pub fn get(&self, key: &str) -> &Json {
        match self {
            Json::Obj(pairs) => pairs
                .iter()
                .find(|(k, _)| k == key)
                .map_or(&Json::Null, |(_, v)| v),
            _ => &Json::Null,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(items) => items,
            _ => &[],
        }
    }

    pub fn obj(&self) -> &[(String, Json)] {
        match self {
            Json::Obj(pairs) => pairs,
            _ => &[],
        }
    }

    pub fn int(&self) -> Option<i64> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }
}

/// Parses `text`; the values of the keys in `skip` are read and dropped.
pub fn parse(text: &str, skip: &[&str]) -> Result<Json, Error> {
    let mut p = Parser {
        b: text.as_bytes(),
        i: 0,
        skip,
    };
    let v = p.value(true)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    skip: &'a [&'a str],
}

impl Parser<'_> {
    fn err(&self, what: &'static str) -> Error {
        Error {
            offset: self.i,
            what,
        }
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.b.get(self.i) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), Error> {
        self.ws();
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err("unexpected character"))
        }
    }

    fn word(&mut self, w: &[u8], v: Json) -> Result<Json, Error> {
        if self.b[self.i..].starts_with(w) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(self.err("bad literal"))
        }
    }

    /// One value; with `keep` false nothing is built (Null is returned).
    fn value(&mut self, keep: bool) -> Result<Json, Error> {
        self.ws();
        match self.b.get(self.i) {
            Some(b'{') => self.object(keep),
            Some(b'[') => self.array(keep),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.word(b"true", Json::Bool(true)),
            Some(b'f') => self.word(b"false", Json::Bool(false)),
            Some(b'n') => self.word(b"null", Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while let Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') = self.b.get(self.i)
                {
                    self.i += 1;
                }
                let n =
                    std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("number"))?;
                Ok(Json::Num(n.to_owned()))
            }
            _ => Err(self.err("expected a value")),
        }
    }

    fn object(&mut self, keep: bool) -> Result<Json, Error> {
        self.i += 1;
        let mut pairs = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Json::Obj(pairs));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.eat(b':')?;
            let keep_value = keep && !self.skip.contains(&key.as_str());
            let v = self.value(keep_value)?;
            if keep_value {
                pairs.push((key, v));
            }
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(if keep { Json::Obj(pairs) } else { Json::Null });
                }
                _ => return Err(self.err("expected , or }")),
            }
        }
    }

    fn array(&mut self, keep: bool) -> Result<Json, Error> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            let v = self.value(keep)?;
            if keep {
                items.push(v);
            }
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(if keep { Json::Arr(items) } else { Json::Null });
                }
                _ => return Err(self.err("expected , or ]")),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let h = self
            .b
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("short escape"))?;
        let s = std::str::from_utf8(h).map_err(|_| self.err("bad escape"))?;
        self.i += 4;
        u32::from_str_radix(s, 16).map_err(|_| self.err("bad escape"))
    }

    fn string(&mut self) -> Result<String, Error> {
        if self.b.get(self.i) != Some(&b'"') {
            return Err(self.err("expected a string"));
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(&c) = self.b.get(self.i) {
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.i += 1;
            }
            // The input is a &str and the run stops at ASCII bytes only.
            out.push_str(
                std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("utf-8"))?,
            );
            match self.b.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = *self.b.get(self.i).ok_or_else(|| self.err("short escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode()?),
                        _ => return Err(self.err("bad escape")),
                    }
                }
                _ => return Err(self.err("unterminated string")),
            }
        }
    }

    fn unicode(&mut self) -> Result<char, Error> {
        let hi = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&hi) {
            if !self.b[self.i..].starts_with(b"\\u") {
                return Err(self.err("lone surrogate"));
            }
            self.i += 2;
            let lo = self.hex4()?;
            if !(0xDC00..0xE000).contains(&lo) {
                return Err(self.err("bad surrogate pair"));
            }
            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
        } else {
            hi
        };
        char::from_u32(code).ok_or_else(|| self.err("bad code point"))
    }
}

#[cfg(test)]
mod tests {
    use super::{Json, parse};

    #[test]
    fn reads_nested_values_and_escapes() {
        let v = parse(r#"{"a": [1, -2.5e3, true, null], "b": "x\"é😀"}"#, &[]).unwrap();
        assert_eq!(v.get("a").arr().len(), 4);
        assert_eq!(v.get("a").arr()[0].int(), Some(1));
        assert_eq!(v.get("b").str(), Some("x\"é😀"));
    }

    #[test]
    fn skipped_keys_are_dropped_but_checked() {
        let v = parse(r#"{"keep": 1, "drop": {"deep": [1, 2]}}"#, &["drop"]).unwrap();
        assert_eq!(v.obj().len(), 1);
        assert!(parse(r#"{"drop": {"deep": [1, }}"#, &["drop"]).is_err());
    }

    #[test]
    fn malformed_input_is_refused() {
        for bad in ["{", "[1,]", r#"{"a" 1}"#, "tru", r#""\x""#, "1 2"] {
            assert!(parse(bad, &[]).is_err(), "{bad}");
        }
        assert_eq!(parse("[]", &[]).unwrap(), Json::Arr(vec![]));
    }
}
