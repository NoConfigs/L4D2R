//! VMT material parsing (just the parameters the renderer cares about).

use crate::kv::{Kv, KvValue};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Vmt {
    pub shader: String,
    /// Lower-cased parameter name -> raw string value (no nested blocks except patch resolution).
    pub params: HashMap<String, String>,
}

impl Vmt {
    /// `resolve_include` loads another VMT's text (for `patch` materials).
    pub fn parse(src: &str, resolve_include: &dyn Fn(&str) -> Option<String>) -> Vmt {
        Self::parse_depth(src, resolve_include, 0)
    }

    fn parse_depth(src: &str, inc: &dyn Fn(&str) -> Option<String>, depth: u32) -> Vmt {
        let root = Kv::parse(src);
        let Some(top) = root.children().first() else {
            return Vmt::default();
        };
        let mut vmt = Vmt {
            shader: top.key.to_ascii_lowercase(),
            params: HashMap::new(),
        };
        if vmt.shader == "patch" && depth < 8 {
            if let Some(path) = top.str("include") {
                if let Some(text) = inc(path) {
                    let mut base = Self::parse_depth(&text, inc, depth + 1);
                    if let Some(KvValue::Obj(ins)) = top.get("insert").map(|k| k.value.clone()) {
                        for k in ins {
                            if let KvValue::Str(v) = k.value {
                                base.params.insert(k.key.to_ascii_lowercase(), v);
                            }
                        }
                    }
                    return base;
                }
            }
        }
        for k in top.children() {
            if let KvValue::Str(v) = &k.value {
                vmt.params.insert(k.key.to_ascii_lowercase(), v.clone());
            }
        }
        vmt
    }

    pub fn param(&self, k: &str) -> Option<&str> {
        self.params.get(k).map(|s| s.as_str())
    }

    pub fn flag(&self, k: &str) -> bool {
        self.param(k).map(|v| v.trim() != "0").unwrap_or(false)
    }

    pub fn base_texture(&self) -> Option<&str> {
        self.param("$basetexture")
    }

    pub fn is_translucent(&self) -> bool {
        self.flag("$translucent") || self.flag("$additive")
    }

    pub fn is_alpha_tested(&self) -> bool {
        self.flag("$alphatest")
    }
}
