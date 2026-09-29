//! Parser for Valve's text KeyValues format (.vdf / .acf):
//!
//! ```text
//! "AppState" { "appid" "620" "name" "Portal 2" }
//! ```
//!
//! Returns a nested [`Value`] tree. Keys keep their original case; use [`get`] for case-insensitive
//! lookups, since Steam is inconsistent ("apps" vs "Apps").
//!
//! Direct port of `src/vdf.js` — the tokenizer and object-builder below follow that file's logic line
//! for line so behaviour (escapes, comments, `[$WIN32]`-style conditionals, stray-brace tolerance) stays
//! identical. See `parity` below for a Rust translation of its existing test in `test/games.test.js`.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Object(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Object(_) => None,
        }
    }

    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Value::Object(o) => Some(o),
            Value::Str(_) => None,
        }
    }
}

enum Token {
    Open,
    Close,
    Str(String),
}

struct Parser {
    chars: Vec<char>,
    i: usize,
}

impl Parser {
    fn skip(&mut self) {
        let n = self.chars.len();
        while self.i < n {
            let c = self.chars[self.i];
            if c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == '\u{feff}' {
                self.i += 1;
            } else if c == '/' && self.chars.get(self.i + 1) == Some(&'/') {
                while self.i < n && self.chars[self.i] != '\n' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn token(&mut self) -> Option<Token> {
        self.skip();
        let n = self.chars.len();
        if self.i >= n {
            return None;
        }
        let c = self.chars[self.i];
        if c == '{' {
            self.i += 1;
            return Some(Token::Open);
        }
        if c == '}' {
            self.i += 1;
            return Some(Token::Close);
        }
        if c == '"' {
            self.i += 1;
            let mut out = String::new();
            while self.i < n && self.chars[self.i] != '"' {
                if self.chars[self.i] == '\\' && self.i + 1 < n {
                    let e = self.chars[self.i + 1];
                    out.push(match e {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    self.i += 2;
                } else {
                    out.push(self.chars[self.i]);
                    self.i += 1;
                }
            }
            self.i += 1; // closing quote
            return Some(Token::Str(out));
        }
        // Unquoted token (rare, but valid).
        let mut out = String::new();
        while self.i < n && !matches!(self.chars[self.i], ' ' | '\t' | '\r' | '\n' | '{' | '}' | '"') {
            out.push(self.chars[self.i]);
            self.i += 1;
        }
        Some(Token::Str(out))
    }

    fn object(&mut self, nested: bool) -> HashMap<String, Value> {
        let mut obj = HashMap::new();
        loop {
            let key = match self.token() {
                None => return obj,
                Some(Token::Close) => {
                    if nested {
                        return obj;
                    }
                    continue;
                }
                Some(Token::Open) => continue, // not a valid key; JS's `k.type !== 'str'` skips it too
                Some(Token::Str(s)) => s,
            };
            let value = match self.token() {
                None => return obj,
                Some(t) => t,
            };
            // Skip conditionals like [$WIN32] that may follow a value.
            self.skip();
            if self.chars.get(self.i) == Some(&'[') {
                let n = self.chars.len();
                while self.i < n && self.chars[self.i] != ']' {
                    self.i += 1;
                }
                self.i += 1;
            }
            match value {
                Token::Open => {
                    obj.insert(key, Value::Object(self.object(true)));
                }
                Token::Str(s) => {
                    obj.insert(key, Value::Str(s));
                }
                Token::Close => {} // a stray '}' as a value assigns nothing, same as the JS version
            }
        }
    }
}

pub fn parse(text: &str) -> Value {
    let mut p = Parser { chars: text.chars().collect(), i: 0 };
    Value::Object(p.object(false))
}

/// Case-insensitive property lookup along a path:
/// `get(&data, &["UserLocalConfigStore", "Software", "Valve"])`.
pub fn get<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut cur = Some(value);
    for key in keys {
        let obj = match cur {
            Some(Value::Object(o)) => o,
            _ => return None,
        };
        cur = match obj.get(*key) {
            Some(v) => Some(v),
            None => {
                let lower = key.to_lowercase();
                obj.iter().find(|(k, _)| k.to_lowercase() == lower).map(|(_, v)| v)
            }
        };
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
        get(value, keys).and_then(Value::as_str)
    }

    // Parity check for the JS test "vdf parser handles nesting, escapes, comments and case"
    // (test/games.test.js). Same source text, same assertions, translated to Rust.
    #[test]
    fn parity_nesting_escapes_comments_and_case() {
        let data = parse(
            r#"
    // comment
    "UserLocalConfigStore"
    {
      "Software" { "Valve" { "Steam" { "Apps" {
        "620" { "LastPlayed" "1700000000" "Playtime" "95" }
      } } } }
      "path"  "D:\\Steam Library"
      "quote" "say \"hi\""
    }"#,
        );
        assert_eq!(
            get_str(&data, &["userlocalconfigstore", "software", "valve", "steam", "apps", "620", "playtime"]),
            Some("95")
        );
        assert_eq!(get_str(&data, &["UserLocalConfigStore", "path"]), Some("D:\\Steam Library"));
        assert_eq!(get_str(&data, &["UserLocalConfigStore", "quote"]), Some("say \"hi\""));
        assert_eq!(get(&data, &["nope", "x"]), None);
    }

    #[test]
    fn get_with_no_keys_returns_the_value_itself() {
        let data = parse(r#""a" "1""#);
        assert_eq!(get(&data, &[]), Some(&data));
    }

    #[test]
    fn get_stops_as_soon_as_a_string_is_reached() {
        let data = parse(r#""a" "1""#);
        // "a" resolves to a string; asking for a further key below it must yield None, not panic.
        assert_eq!(get(&data, &["a", "b"]), None);
    }

    #[test]
    fn unquoted_tokens_are_accepted() {
        // Real VDF files are always quoted, but the format allows bare tokens too.
        let data = parse("key value");
        assert_eq!(get_str(&data, &["key"]), Some("value"));
    }
}
