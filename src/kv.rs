//! Small KeyValues lexer that preserves quoted strings and skips comments.
use std::collections::BTreeMap;
#[derive(Clone, Default, Debug)]
pub struct Block {
    pub values: BTreeMap<String, String>,
    pub children: BTreeMap<String, Block>,
}
pub fn blocks(text: &str) -> Block {
    fn read(t: &[String], at: &mut usize, depth: usize) -> Block {
        let mut node = Block::default();
        while *at + 1 < t.len() && t[*at] != "}" && depth < 32 {
            let key = t[*at].to_ascii_lowercase();
            *at += 1;
            if t[*at] == "{" {
                *at += 1;
                node.children.insert(key, read(t, at, depth + 1));
            } else {
                node.values.insert(key, t[*at].clone());
                *at += 1;
            }
        }
        if *at < t.len() && t[*at] == "}" {
            *at += 1;
        }
        node
    }
    read(&tokens(text), &mut 0, 0)
}
pub fn tokens(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() || b[i] == 0 {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < b.len() && !b[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        if b[i] == b'{' || b[i] == b'}' {
            out.push((b[i] as char).to_string());
            i += 1;
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            let mut value = Vec::new();
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() && matches!(b[i + 1], b'"' | b'\\') {
                    i += 1;
                }
                value.push(b[i]);
                i += 1;
            }
            i = (i + 1).min(b.len());
            out.push(String::from_utf8_lossy(&value).into_owned());
        } else {
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'{' | b'}' | 0) {
                i += 1;
            }
            out.push(String::from_utf8_lossy(&b[start..i]).into_owned());
        }
    }
    out
}
pub fn entities(text: &str) -> Vec<BTreeMap<String, String>> {
    let t = tokens(text);
    let mut out = Vec::new();
    let mut current = BTreeMap::new();
    let mut i = 0;
    while i < t.len() {
        match t[i].as_str() {
            "{" => {
                current.clear();
                i += 1;
            }
            "}" => {
                out.push(std::mem::take(&mut current));
                i += 1;
            }
            _ if i + 1 < t.len() => {
                current.insert(t[i].to_lowercase(), t[i + 1].clone());
                i += 2;
            }
            _ => break,
        }
    }
    out
}
pub fn vector(value: Option<&String>) -> [f32; 3] {
    let mut result = [0.; 3];
    if let Some(value) = value {
        for (dst, part) in result.iter_mut().zip(value.split_whitespace()) {
            *dst = part
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(0.);
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_braces_and_comments_do_not_split_entities() {
        let e = entities("// ignore\n{\"classname\" \"prop_dynamic\" \"targetname\" \"a}b\"}");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0]["targetname"], "a}b");
    }
}
