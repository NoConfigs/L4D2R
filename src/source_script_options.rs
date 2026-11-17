//! Bounded option-script evaluation, not a general Squirrel VM. Unknown calls,
//! branches, functions and loops are reported rather than executed as constants.
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Number(f64, bool),
    Bool(bool),
    String(String),
    Unknown,
}
impl Value {
    fn number(&self) -> Option<f64> {
        match self {
            Self::Number(n, _) => Some(*n),
            Self::Bool(b) => Some(u8::from(*b) as f64),
            _ => None,
        }
    }
    fn truth(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Number(n, _) => Some(*n != 0.),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Name(String),
    Number(String),
    String(String),
    Symbol(String),
    Newline,
}
impl Token {
    fn text(&self) -> &str {
        match self {
            Self::Name(s) | Self::Number(s) | Self::String(s) | Self::Symbol(s) => s,
            Self::Newline => "\n",
        }
    }
}
fn lex(text: &str) -> Vec<Token> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() && out.len() < 100_000 {
        if b[i] == b'\n' {
            out.push(Token::Newline);
            i += 1;
            continue;
        }
        if b[i].is_ascii_whitespace() {
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
        if matches!(b[i], b'"' | b'\'') {
            let quote = b[i];
            i += 1;
            let mut value = String::new();
            while i < b.len() && b[i] != quote {
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                }
                value.push(b[i] as char);
                i += 1;
            }
            i = (i + 1).min(b.len());
            out.push(Token::String(value));
            continue;
        }
        if b[i].is_ascii_digit() || b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let start = i;
            if b[i] == b'0' && b.get(i + 1).is_some_and(|c| matches!(c, b'x' | b'X')) {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
                out.push(Token::Number(text[start..i].into()));
                continue;
            }
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_digit()
                    || matches!(b[i], b'.' | b'e' | b'E')
                    || matches!(b[i], b'+' | b'-') && matches!(b[i - 1], b'e' | b'E'))
            {
                i += 1;
            }
            out.push(Token::Number(text[start..i].into()));
            continue;
        }
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'.')) {
                i += 1;
            }
            out.push(Token::Name(text[start..i].into()));
            continue;
        }
        let pair = b
            .get(i..i + 2)
            .and_then(|s| std::str::from_utf8(s).ok())
            .unwrap_or("");
        if matches!(
            pair,
            "<-" | "==" | "!=" | "<=" | ">=" | "&&" | "||" | "+=" | "-=" | "*=" | "/=" | "::"
        ) {
            out.push(Token::Symbol(pair.into()));
            i += 2;
        } else {
            out.push(Token::Symbol((b[i] as char).to_string()));
            i += 1;
        }
    }
    out
}
#[derive(Default, Clone)]
pub struct Evaluated {
    pub options: HashMap<String, f32>,
    pub tables: HashMap<String, HashMap<String, f32>>,
    pub unsupported: Vec<String>,
    pub includes: Vec<String>,
}
#[derive(Default)]
struct Scope {
    vars: HashMap<String, Value>,
    tables: HashMap<String, HashMap<String, Value>>,
    report: Evaluated,
}
pub fn evaluate(sources: &HashMap<String, String>, key: &str) -> Evaluated {
    let mut scope = Scope::default();
    run(sources, key, &mut scope, &mut vec![]);
    scope.report.tables = scope
        .tables
        .into_iter()
        .map(|(name, values)| {
            (
                name,
                values
                    .into_iter()
                    .filter_map(|(k, v)| {
                        v.number()
                            .filter(|n| n.is_finite() && (*n as f32).is_finite())
                            .map(|n| (k, n as f32))
                    })
                    .collect(),
            )
        })
        .collect();
    scope.report.options = scope
        .report
        .tables
        .get("DirectorOptions")
        .cloned()
        .unwrap_or_default();
    scope.report.unsupported.sort();
    scope.report.unsupported.dedup();
    scope.report
}
fn run(sources: &HashMap<String, String>, key: &str, scope: &mut Scope, path: &mut Vec<String>) {
    if path.len() >= 16 || path.iter().any(|s| s == key) {
        scope
            .report
            .unsupported
            .push(format!("include-cycle:{key}"));
        return;
    }
    let Some(text) = sources.get(key) else {
        scope
            .report
            .unsupported
            .push(format!("missing-include:{key}"));
        return;
    };
    path.push(key.into());
    Parser {
        tokens: lex(text),
        at: 0,
        scope,
        sources,
        path,
        depth: 0,
        expression_depth: 0,
        statement_depth: 0,
    }
    .block(true, false);
    path.pop();
}
struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    scope: &'a mut Scope,
    sources: &'a HashMap<String, String>,
    path: &'a mut Vec<String>,
    depth: usize,
    expression_depth: usize,
    statement_depth: usize,
}
impl Parser<'_> {
    fn peek(&self) -> &str {
        self.tokens.get(self.at).map_or("", Token::text)
    }
    fn take(&mut self, s: &str) -> bool {
        if self.peek() == s {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn gaps(&mut self) {
        while matches!(self.peek(), "\n" | ";" | ",") {
            self.at += 1;
        }
    }
    fn lines(&mut self) {
        while self.take("\n") {}
    }
    fn skip_balanced(&mut self, open: &str, close: &str) {
        if !self.take(open) {
            return;
        }
        let mut depth = 1;
        while self.at < self.tokens.len() && depth > 0 {
            let s = self.peek();
            if s == open {
                depth += 1;
            }
            if s == close {
                depth -= 1;
            }
            self.at += 1;
        }
    }
    fn block(&mut self, execute: bool, braced: bool) {
        if self.depth >= 32 {
            self.scope.report.unsupported.push("nesting-limit".into());
            return;
        }
        self.depth += 1;
        while self.at < self.tokens.len() {
            self.gaps();
            if braced && self.take("}") {
                break;
            }
            let before = self.at;
            self.statement(execute);
            if self.at == before {
                self.at += 1;
            }
        }
        self.depth -= 1;
    }
    fn statement(&mut self, execute: bool) {
        if self.statement_depth >= 32 {
            self.scope
                .report
                .unsupported
                .push("statement-depth-limit".into());
            self.at = self.tokens.len();
            return;
        }
        self.statement_depth += 1;
        self.statement_inner(execute);
        self.statement_depth -= 1;
    }
    fn statement_inner(&mut self, execute: bool) {
        self.lines();
        if self.take("if") {
            self.take("(");
            let condition = self.expr(0);
            self.take(")");
            self.lines();
            if execute && condition.truth().is_none() {
                self.scope
                    .report
                    .unsupported
                    .push("unknown-condition".into());
            }
            self.statement(execute && condition.truth() == Some(true));
            self.lines();
            if self.take("else") {
                self.statement(execute && condition.truth() == Some(false));
            }
            return;
        }
        if self.take("{") {
            self.block(execute, true);
            return;
        }
        if matches!(
            self.peek(),
            "function" | "class" | "foreach" | "for" | "while" | "switch"
        ) {
            let kind = self.peek().to_string();
            self.at += 1;
            let name = self.peek().to_string();
            if execute {
                self.scope.report.unsupported.push(format!("{kind}:{name}"));
            }
            while self.at < self.tokens.len() && self.peek() != "{" && self.peek() != "\n" {
                self.at += 1;
            }
            self.lines();
            if self.peek() == "{" {
                self.skip_balanced("{", "}");
            } else if matches!(kind.as_str(), "while" | "for" | "foreach") {
                self.statement(false);
            }
            return;
        }
        self.take("local");
        self.take("::");
        let Some(Token::Name(name)) = self.tokens.get(self.at).cloned() else {
            self.at += usize::from(self.at < self.tokens.len());
            return;
        };
        self.at += 1;
        if self.peek() == "(" {
            if name == "IncludeScript" {
                self.at += 1;
                let include = self.expr(0);
                let target = if self.take(",") {
                    let name = self.peek().to_string();
                    self.expr(0);
                    Some(name)
                } else {
                    None
                };
                self.take(")");
                if execute {
                    if let Value::String(key) = include {
                        if target.as_deref().is_none_or(|t| t == "this") {
                            let key = key
                                .replace('\\', "/")
                                .trim_end_matches(".nut")
                                .trim_end_matches(".nuc")
                                .to_ascii_lowercase();
                            if key.split('/').any(|s| s == "..") {
                                self.scope
                                    .report
                                    .unsupported
                                    .push(format!("invalid-include:{key}"));
                            } else {
                                self.scope.report.includes.push(key.clone());
                                run(self.sources, &key, self.scope, self.path);
                            }
                        } else {
                            self.scope
                                .report
                                .unsupported
                                .push(format!("include-scope:{}", target.unwrap()));
                        }
                    } else {
                        self.scope.report.unsupported.push("dynamic-include".into());
                    }
                }
            } else {
                self.skip_balanced("(", ")");
                if execute && !matches!(name.as_str(), "Msg" | "print" | "printl") {
                    self.scope.report.unsupported.push(format!("call:{name}"));
                }
            }
            return;
        }
        let op = self.peek().to_string();
        if !matches!(op.as_str(), "=" | "<-" | "+=" | "-=" | "*=" | "/=") {
            return;
        }
        self.at += 1;
        self.lines();
        if self.take("{") {
            let tracked = matches!(
                name.as_str(),
                "DirectorOptions"
                    | "MapOptions"
                    | "MutationOptions"
                    | "SessionOptions"
                    | "BaseScriptedDOTable"
            );
            let mut values = HashMap::new();
            while self.at < self.tokens.len() {
                self.gaps();
                if self.take("}") {
                    break;
                }
                if self.peek() == "function" {
                    self.statement(execute && tracked);
                    continue;
                }
                let key = self.peek().to_string();
                self.at += 1;
                if self.take("=") || self.take("<-") {
                    self.lines();
                    let v = self.value();
                    if execute && tracked && v == Value::Unknown {
                        self.scope.report.unsupported.push(format!("{name}.{key}"));
                    }
                    values.insert(key, v);
                } else {
                    if self.peek() == "{" {
                        self.skip_balanced("{", "}");
                    } else {
                        self.at += usize::from(self.at < self.tokens.len());
                    }
                }
            }
            if execute && tracked {
                self.scope.tables.insert(name, values);
            }
            return;
        }
        let mut value = self.value();
        if execute {
            if op.len() == 2 && op.ends_with('=') && op != "<-" {
                value = binary(&op[..1], self.lookup(&name), value);
            }
            if let Some((table, key)) = name.split_once('.') {
                if let Some(values) = self.scope.tables.get_mut(table) {
                    values.insert(key.into(), value.clone());
                } else {
                    self.scope
                        .report
                        .unsupported
                        .push(format!("scope-assignment:{name}"));
                }
            } else {
                self.scope.vars.insert(name.clone(), value.clone());
            }
            if value == Value::Unknown {
                self.scope
                    .report
                    .unsupported
                    .push(format!("assignment:{name}"));
            }
        }
    }
    fn lookup(&self, name: &str) -> Value {
        match name {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => self
                .scope
                .vars
                .get(name)
                .cloned()
                .or_else(|| {
                    name.split_once('.')
                        .and_then(|(t, k)| self.scope.tables.get(t)?.get(k).cloned())
                })
                .unwrap_or(Value::Unknown),
        }
    }
    fn value(&mut self) -> Value {
        let result = self.expr(0);
        // An unsupported continuation must invalidate the whole RHS. Accepting
        // just its numeric prefix would silently apply the wrong setting.
        let invalid_name_tail = matches!(self.tokens.get(self.at), Some(Token::Name(_)))
            && !self
                .tokens
                .get(self.at + 1)
                .is_some_and(|t| matches!(t.text(), "=" | "<-"));
        if invalid_name_tail || matches!(self.peek(), "?" | "|" | "&" | "^" | "[" | "::") {
            while self.at < self.tokens.len() && !matches!(self.peek(), "\n" | "," | ";" | "}") {
                if self.peek() == "(" {
                    self.skip_balanced("(", ")");
                } else if self.peek() == "[" {
                    self.skip_balanced("[", "]");
                } else {
                    self.at += 1;
                }
            }
            Value::Unknown
        } else {
            result
        }
    }
    fn expr(&mut self, min: u8) -> Value {
        if self.expression_depth >= 64 {
            self.scope
                .report
                .unsupported
                .push("expression-depth-limit".into());
            self.at = self.tokens.len();
            return Value::Unknown;
        }
        self.expression_depth += 1;
        let value = self.expr_inner(min);
        self.expression_depth -= 1;
        value
    }
    fn expr_inner(&mut self, min: u8) -> Value {
        self.lines();
        let Some(token) = self.tokens.get(self.at).cloned() else {
            return Value::Unknown;
        };
        let mut left = match token {
            Token::Number(n) => {
                self.at += 1;
                let hex = n.starts_with("0x") || n.starts_with("0X");
                let parsed = if hex {
                    u64::from_str_radix(&n[2..], 16).ok().map(|n| n as f64)
                } else {
                    n.parse::<f64>().ok()
                };
                parsed
                    .filter(|n| n.is_finite())
                    .map_or(Value::Unknown, |v| {
                        Value::Number(v, hex || !n.contains(['.', 'e', 'E']))
                    })
            }
            Token::String(s) => {
                self.at += 1;
                Value::String(s)
            }
            Token::Name(n) if n == "function" => {
                self.at += 1;
                self.skip_balanced("(", ")");
                self.lines();
                self.skip_balanced("{", "}");
                Value::Unknown
            }
            Token::Name(n) => {
                self.at += 1;
                if self.peek() == "(" {
                    let no_arguments = self
                        .tokens
                        .get(self.at + 1)
                        .is_some_and(|t| t.text() == ")");
                    self.skip_balanced("(", ")");
                    match n.as_str() {
                        _ if !no_arguments => Value::Unknown,
                        "Director.GetGameModeBase" => Value::String("coop".into()),
                        "Director.IsPlayingOnConsole" => Value::Bool(false),
                        _ => Value::Unknown,
                    }
                } else {
                    self.lookup(&n)
                }
            }
            Token::Symbol(s) if s == "(" => {
                self.at += 1;
                let v = self.expr(0);
                self.lines();
                self.take(")");
                v
            }
            Token::Symbol(s) if matches!(s.as_str(), "-" | "+" | "!") => {
                self.at += 1;
                let v = self.expr(7);
                match (s.as_str(), v) {
                    ("-", Value::Number(n, i)) => Value::Number(-n, i),
                    ("+", v) => v,
                    ("!", v) => v.truth().map_or(Value::Unknown, |v| Value::Bool(!v)),
                    _ => Value::Unknown,
                }
            }
            Token::Symbol(s) if s == "[" => {
                self.skip_balanced("[", "]");
                Value::Unknown
            }
            Token::Symbol(s) if s == "{" => {
                self.skip_balanced("{", "}");
                Value::Unknown
            }
            _ => return Value::Unknown,
        };
        loop {
            let op = self.peek().to_string();
            let precedence = match op.as_str() {
                "||" => 1,
                "&&" => 2,
                "==" | "!=" => 3,
                "<" | ">" | "<=" | ">=" => 4,
                "+" | "-" => 5,
                "*" | "/" | "%" => 6,
                _ => break,
            };
            if precedence < min {
                break;
            }
            self.at += 1;
            let right = self.expr(precedence + 1);
            left = binary(&op, left, right);
        }
        left
    }
}
fn binary(op: &str, a: Value, b: Value) -> Value {
    if op == "&&" && (a.truth() == Some(false) || b.truth() == Some(false)) {
        return Value::Bool(false);
    }
    if op == "||" && (a.truth() == Some(true) || b.truth() == Some(true)) {
        return Value::Bool(true);
    }
    if matches!(a, Value::Unknown) || matches!(b, Value::Unknown) {
        return Value::Unknown;
    }
    if matches!(op, "==" | "!=") {
        let same = if let (Some(a), Some(b)) = (a.number(), b.number()) {
            a == b
        } else {
            a == b
        };
        return Value::Bool(if op == "==" { same } else { !same });
    }
    if let (Value::String(a), Value::String(b)) = (&a, &b) {
        if op == "+" {
            return Value::String(format!("{a}{b}"));
        }
    }
    let (Some(x), Some(y)) = (a.number(), b.number()) else {
        return Value::Unknown;
    };
    if matches!(op, "&&" | "||" | "<" | ">" | "<=" | ">=") {
        return Value::Bool(match op {
            "&&" => x != 0. && y != 0.,
            "||" => x != 0. || y != 0.,
            "<" => x < y,
            ">" => x > y,
            "<=" => x <= y,
            _ => x >= y,
        });
    }
    let integer = matches!(a, Value::Number(_, true)) && matches!(b, Value::Number(_, true));
    let n = match op {
        "+" => x + y,
        "-" => x - y,
        "*" => x * y,
        "/" if y != 0. => {
            if integer {
                (x / y).trunc()
            } else {
                x / y
            }
        }
        "%" if integer && y != 0. => x % y,
        _ => return Value::Unknown,
    };
    if n.is_finite() {
        Value::Number(n, integer)
    } else {
        Value::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn one(text: &str) -> Evaluated {
        evaluate(&HashMap::from([("test".into(), text.into())]), "test")
    }
    #[test]
    fn variables_arithmetic_pc_conditions_and_member_updates_follow_source_order() {
        let s = one(r#"
            limit <- 25
            if (Director.IsPlayingOnConsole()) { limit = 20 }
            wave <- 50
            if (Director.GetGameModeBase() == "coop") wave = 30
            else wave = 99
            DirectorOptions <- { CommonLimit=limit, MobMinSize=(wave - 10) / 2, MobMaxSize=wave }
            DirectorOptions.CommonLimit -= 1
            DirectorOptions.MobMaxSize += 2 * 3
            // integer division must differ from float division
            DirectorOptions.Int = 7 / 2
            DirectorOptions.Float = 7.0 / 2
            DirectorOptions.Hex = 0x19 + 1
        "#);
        assert_eq!(s.options["CommonLimit"], 24.);
        assert_eq!(s.options["MobMinSize"], 10.);
        assert_eq!(s.options["MobMaxSize"], 36.);
        assert_eq!(s.options["Int"], 3.);
        assert_eq!(s.options["Float"], 3.5);
        assert_eq!(s.options["Hex"], 26.);
        assert!(s.unsupported.is_empty(), "{:?}", s.unsupported);
    }
    #[test]
    fn callbacks_loops_unknown_branches_and_nested_tables_are_not_executed_as_options() {
        let s = one(r#"
            DirectorOptions <- {
                CommonLimit = 25
                function Recalculate() { CommonLimit = 100 }
                GetValue = function() { CommonLimit = 101 }
                Nested = { CommonLimit = 102 }
                MobMaxSize = 12
            }
            function Later() { DirectorOptions.CommonLimit = 200 }
            if (UnsupportedQuery()) { DirectorOptions.CommonLimit = 201 }
            else { DirectorOptions.CommonLimit = 202 }
            while (true)
                DirectorOptions.CommonLimit = 203
            /* DirectorOptions <- { CommonLimit = 204 } */
            Msg("DirectorOptions <- { CommonLimit = 205 }")
        "#);
        assert_eq!(s.options["CommonLimit"], 25.);
        assert_eq!(s.options["MobMaxSize"], 12.);
        assert!(s.unsupported.contains(&"unknown-condition".into()));
        assert!(s.unsupported.iter().any(|s| s.starts_with("function:")));
    }
    #[test]
    fn same_scope_includes_share_variables_and_handle_cycles_and_missing_sources() {
        let sources=HashMap::from([
            ("test".into(),"limit <- 17; IncludeScript(\"child.nut\", this); DirectorOptions.CommonLimit += 1; IncludeScript(\"missing\");".into()),
            ("child".into(),"DirectorOptions <- { CommonLimit = limit, MobMaxSize = 20 }; IncludeScript(\"test\");".into()),
        ]);
        let s = evaluate(&sources, "test");
        assert_eq!(s.options["CommonLimit"], 18.);
        assert!(s.unsupported.contains(&"include-cycle:test".into()));
        assert!(s.unsupported.contains(&"missing-include:missing".into()));
        assert_eq!(s.includes, ["child", "test", "missing"]);
    }
    #[test]
    fn invalid_values_and_other_option_scopes_do_not_override_director_options() {
        let s = one(
            "DirectorOptions <- { CommonLimit = 25, Invalid = 1 / 0, Dynamic = RandomInt(1, 20) }; MapOptions <- { CommonLimit = 100 }; SessionOptions <- { CommonLimit = 200 }; DirectorOptions.CommonLimit = unknown;",
        );
        assert!(!s.options.contains_key("CommonLimit"));
        assert!(!s.options.contains_key("Invalid"));
        assert!(!s.options.contains_key("Dynamic"));
        assert_eq!(s.tables["MapOptions"]["CommonLimit"], 100.);
        assert_eq!(s.tables["SessionOptions"]["CommonLimit"], 200.);
    }
    #[test]
    fn unsupported_expression_tails_and_deep_nesting_are_bounded_and_audited() {
        let s = one(
            "DirectorOptions <- { CommonLimit = 1 | 8, MobMaxSize = true ? 30 : 10, MobMinSize = 6 }",
        );
        assert!(!s.options.contains_key("CommonLimit"));
        assert!(!s.options.contains_key("MobMaxSize"));
        assert_eq!(s.options["MobMinSize"], 6.);
        let s = one("DirectorOptions <- { CommonLimit=25oops, MobMinSize=6 }");
        assert!(!s.options.contains_key("CommonLimit"));
        assert_eq!(s.options["MobMinSize"], 6.);
        let text = format!(
            "DirectorOptions <- {{ CommonLimit = {}1{} }}",
            "(".repeat(200),
            ")".repeat(200)
        );
        assert!(
            one(&text)
                .unsupported
                .contains(&"expression-depth-limit".into())
        );
        let text = format!(
            "{}DirectorOptions <- {{ CommonLimit = 1 }}",
            "if (true) ".repeat(200)
        );
        assert!(
            one(&text)
                .unsupported
                .contains(&"statement-depth-limit".into())
        );
    }
    #[test]
    fn owned_campaign_scripts_resolve_pc_limits_and_skip_versus_overrides() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let mut sources = HashMap::new();
        for key in ["c7m1_docks", "c7m2_barge", "c14_junkyard_crane"] {
            let bytes = assets.read(&format!("scripts/vscripts/{key}.nuc")).unwrap();
            let decoded = crate::source_ice::decode(&bytes);
            sources.insert(key.into(), String::from_utf8_lossy(&decoded).into_owned());
        }
        let docks = evaluate(&sources, "c7m1_docks");
        assert_eq!(docks.options["CommonLimit"], 25.);
        assert_eq!(docks.options["MegaMobSize"], 30.);
        assert_eq!(evaluate(&sources, "c7m2_barge").options["CommonLimit"], 25.);
        let crane = evaluate(&sources, "c14_junkyard_crane");
        assert_eq!(crane.options["CommonLimit"], 16.);
        assert_eq!(crane.options["MobSpawnMinTime"], 3.);
    }
}
