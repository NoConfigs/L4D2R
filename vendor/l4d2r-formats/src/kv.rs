//! Valve KeyValues (text) parser: used by VMT materials, weapon scripts, gameinfo, soundscripts...

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum KvValue {
    Str(String),
    Obj(Vec<Kv>),
}

impl Default for KvValue {
    fn default() -> Self {
        KvValue::Obj(Vec::new())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Kv {
    pub key: String,
    pub value: KvValue,
}

impl Kv {
    /// Parse a whole file. The result is a synthetic root whose children are the top-level keys.
    pub fn parse(src: &str) -> Kv {
        let mut t = Tokens {
            s: src.as_bytes(),
            p: 0,
        };
        let children = parse_body(&mut t, true);
        Kv {
            key: String::new(),
            value: KvValue::Obj(children),
        }
    }

    pub fn children(&self) -> &[Kv] {
        match &self.value {
            KvValue::Obj(v) => v,
            _ => &[],
        }
    }

    /// Case-insensitive child lookup.
    pub fn get(&self, key: &str) -> Option<&Kv> {
        self.children()
            .iter()
            .find(|c| c.key.eq_ignore_ascii_case(key))
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match &self.get(key)?.value {
            KvValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn f32(&self, key: &str) -> Option<f32> {
        self.str(key)?.trim().parse().ok()
    }

    pub fn i32(&self, key: &str) -> Option<i32> {
        let s = self.str(key)?.trim();
        s.parse::<i32>()
            .ok()
            .or_else(|| s.parse::<f32>().ok().map(|f| f as i32))
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            KvValue::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl fmt::Display for Kv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn go(kv: &Kv, d: usize, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let pad = "  ".repeat(d);
            match &kv.value {
                KvValue::Str(s) => writeln!(f, "{pad}\"{}\" \"{s}\"", kv.key),
                KvValue::Obj(c) => {
                    writeln!(f, "{pad}\"{}\"\n{pad}{{", kv.key)?;
                    for k in c {
                        go(k, d + 1, f)?;
                    }
                    writeln!(f, "{pad}}}")
                }
            }
        }
        go(self, 0, f)
    }
}

struct Tokens<'a> {
    s: &'a [u8],
    p: usize,
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
    Eof,
}

impl Tokens<'_> {
    fn next(&mut self) -> Tok {
        loop {
            while self.p < self.s.len()
                && (self.s[self.p].is_ascii_whitespace() || self.s[self.p] == 0)
            {
                self.p += 1;
            }
            if self.p + 1 < self.s.len() && self.s[self.p] == b'/' && self.s[self.p + 1] == b'/' {
                while self.p < self.s.len() && self.s[self.p] != b'\n' {
                    self.p += 1;
                }
                continue;
            }
            break;
        }
        let Some(&c) = self.s.get(self.p) else {
            return Tok::Eof;
        };
        match c {
            b'{' => {
                self.p += 1;
                Tok::Open
            }
            b'}' => {
                self.p += 1;
                Tok::Close
            }
            b'"' => {
                self.p += 1;
                let start = self.p;
                while self.p < self.s.len() && self.s[self.p] != b'"' {
                    // Source's parser treats backslash escapes loosely; keep raw bytes.
                    self.p += 1;
                }
                let t = String::from_utf8_lossy(&self.s[start..self.p]).into_owned();
                self.p += 1;
                Tok::Str(t)
            }
            _ => {
                let start = self.p;
                while self.p < self.s.len()
                    && !self.s[self.p].is_ascii_whitespace()
                    && !matches!(self.s[self.p], b'{' | b'}' | b'"')
                {
                    self.p += 1;
                }
                Tok::Str(String::from_utf8_lossy(&self.s[start..self.p]).into_owned())
            }
        }
    }

    fn peek(&mut self) -> Tok {
        let save = self.p;
        let t = self.next();
        self.p = save;
        t
    }
}

fn is_conditional(s: &str) -> bool {
    s.starts_with("[$") || s.starts_with("[!$")
}

fn parse_body(t: &mut Tokens, top: bool) -> Vec<Kv> {
    let mut out = Vec::new();
    loop {
        let key = match t.next() {
            Tok::Str(k) => k,
            Tok::Close => return out,
            Tok::Eof => return out,
            Tok::Open => continue,
        };
        match t.next() {
            Tok::Str(v) => {
                // `key value [$X360]` — drop platform-conditional entries we don't care about.
                let cond = matches!(t.peek(), Tok::Str(ref c) if is_conditional(c));
                if cond {
                    let _ = t.next();
                    continue;
                }
                out.push(Kv {
                    key,
                    value: KvValue::Str(v),
                });
            }
            Tok::Open => {
                let children = parse_body(t, false);
                if matches!(t.peek(), Tok::Str(ref c) if is_conditional(c)) {
                    let _ = t.next();
                    continue;
                }
                out.push(Kv {
                    key,
                    value: KvValue::Obj(children),
                });
            }
            Tok::Close | Tok::Eof => return out,
        }
        let _ = top;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested() {
        let kv = Kv::parse(
            "\"WeaponData\"\n{\n\t\"Damage\" \"33\" // c\n\tSub { a 1 b \"2 3 4\" }\n}\n",
        );
        let w = kv.get("weapondata").unwrap();
        assert_eq!(w.i32("damage"), Some(33));
        assert_eq!(w.get("sub").unwrap().str("b"), Some("2 3 4"));
    }
}
