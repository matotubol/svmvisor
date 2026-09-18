//! Minimal ordered JSON value whose text form matches Python's
//! `json.dumps(value, indent=2)` (the format of the evidence files that
//! firmware/card/verify-resident-build.py and earlier builds use).

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Value {
    pub fn str(text: &str) -> Value {
        Value::Str(text.to_string())
    }
    pub fn strs(items: &[&str]) -> Value {
        Value::List(items.iter().map(|item| Value::str(item)).collect())
    }
    pub fn ints(items: impl IntoIterator<Item = i64>) -> Value {
        Value::List(items.into_iter().map(Value::Int).collect())
    }
    #[cfg(test)]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => {
                entries.iter().find(|(name, _)| name == key).map(|(_, value)| value)
            }
            _ => None,
        }
    }
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }
    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Value::Int(value) => out.push_str(&value.to_string()),
            Value::Str(value) => string(out, value),
            Value::List(items) if items.is_empty() => out.push_str("[]"),
            Value::Map(entries) if entries.is_empty() => out.push_str("{}"),
            Value::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&"  ".repeat(depth + 1));
                    item.write(out, depth + 1);
                }
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
                out.push(']');
            }
            Value::Map(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    out.push_str(if index == 0 { "\n" } else { ",\n" });
                    out.push_str(&"  ".repeat(depth + 1));
                    string(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
                out.push('}');
            }
        }
    }
}

/// `ensure_ascii=True` string escaping.
fn string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::Value;

    #[test]
    fn matches_python_indent_two_layout() {
        let value = Value::Map(vec![
            ("a".into(), Value::Bool(false)),
            ("b".into(), Value::ints([1, 2])),
            (
                "c".into(),
                Value::Map(vec![("d".into(), Value::str("x\\y\"\u{e9}\n\u{1f600}\u{1}"))]),
            ),
            ("e".into(), Value::List(vec![])),
            ("f".into(), Value::Map(vec![])),
        ]);
        // Generated with Python's json.dumps(value, indent=2); see expected-json.txt.
        let expected = include_str!("expected-json.txt");
        assert_eq!(value.dump(), expected.replace("\r\n", "\n").trim_end());
    }
}
