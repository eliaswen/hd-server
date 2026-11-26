//! Schema-driven command boundaries. Semantics are deliberately separate.
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};
#[derive(Deserialize)]
struct Field {
    t: String,
    #[serde(default)]
    count: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    elem: Vec<Field>,
    #[serde(default)]
    fields: Vec<Field>,
}
fn schema() -> &'static BTreeMap<String, Vec<Field>> {
    static DATA: OnceLock<BTreeMap<String, Vec<Field>>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("command-schema.json")).unwrap()
    })
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    bit: usize,
    budget: usize,
}
impl Reader<'_> {
    fn raw(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.pos.checked_add(n)?;
        let result = self.bytes.get(self.pos..end)?;
        self.pos = end;
        self.bit = 0;
        Some(result)
    }
    fn number(&mut self, t: &str) -> Option<i64> {
        Some(match t {
            "vint" => {
                self.bit = 0;
                let mut rest = self.bytes.get(self.pos..)?;
                let before = rest.len();
                let n = crate::protocol::messages::turn_vint(&mut rest)? as i32;
                self.pos += before - rest.len();
                i64::from(n)
            }
            "int8" => i64::from(self.raw(1)?[0] as i8),
            "u16" => i64::from(u16::from_be_bytes(self.raw(2)?.try_into().ok()?)),
            "u32" => i64::from(u32::from_be_bytes(self.raw(4)?.try_into().ok()?)),
            _ => return None,
        })
    }
    fn fields(&mut self, fields: &[Field], depth: usize) -> Option<Vec<Value>> {
        if depth > 16 {
            return None;
        }
        let mut result = Vec::new();
        for f in fields {
            self.budget = self.budget.checked_sub(1)?;
            result.push(match f.t.as_str() {
                "vint" | "int8" | "u16" | "u32" => json!(self.number(&f.t)?),
                "bool" => {
                    if self.bit == 0 {
                        self.pos = self.pos.checked_add(1)?;
                    }
                    let byte = *self.bytes.get(self.pos.checked_sub(1)?)?;
                    let value = byte & (1 << self.bit) != 0;
                    self.bit = (self.bit + 1) % 8;
                    json!(value)
                }
                "pair" => json!([self.number("u32")?, self.number("u32")?]),
                "string" | "blob" => {
                    let n = self.number("u32")? as u32 as i32;
                    if n == -1 {
                        Value::Null
                    } else {
                        if !(0..=900000).contains(&n) {
                            return None;
                        }
                        // Retain bytes, including non-UTF8 string payloads.
                        json!(self.raw(n as usize)?)
                    }
                }
                "nested" => json!(self.fields(&f.fields, depth + 1)?),
                "list" => {
                    let n = self.number(&f.count)?;
                    if !(0..=10000).contains(&n) {
                        return None;
                    }
                    // The static schema did not trace the element decoder here.
                    if n != 0 && f.elem.is_empty() {
                        return None;
                    }
                    let mut items = Vec::new();
                    for _ in 0..n {
                        items.push(self.fields(&f.elem, depth + 1)?);
                    }
                    json!(items)
                }
                _ => return None,
            });
        }
        Some(result)
    }
}
/// No input is committed on failure, including invalid/special factory arms.
pub fn decode(input: &mut &[u8]) -> Option<(u32, Vec<Value>, Vec<u8>)> {
    let original = *input;
    let mut r = Reader {
        bytes: original,
        pos: 0,
        bit: 0,
        budget: 100000,
    };
    let kind = u32::try_from(r.number("vint")?).ok()?;
    let fields = schema().get(&kind.to_string())?;
    // Empty schemas are invalid or special handlers, not zero-byte commands.
    if fields.is_empty() {
        return None;
    }
    let values = r.fields(fields, 0)?;
    let raw = original[..r.pos].to_vec();
    *input = &original[r.pos..];
    Some((kind, values, raw))
}
/// Read the tick at the location identified by the command's wire schema.
pub fn tick(kind: u32, values: &[Value]) -> Option<u32> {
    fn find(fields: &[Field], values: &[Value]) -> Option<u32> {
        for (field, value) in fields.iter().zip(values) {
            if field.role == "tick" {
                return u32::try_from(value.as_i64()?).ok();
            }
            if field.t == "nested" {
                if let Some(tick) = find(&field.fields, value.as_array()?) {
                    return Some(tick);
                }
            }
        }
        None
    }

    find(schema().get(&kind.to_string())?, values)
}

#[path = "command_names.rs"]
mod names;
pub use names::name;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_ticks_follow_the_schema() {
        assert_eq!(tick(235, &[json!(91), json!(8)]), Some(91));
        assert_eq!(
            tick(226, &[json!(4), json!(92), json!(1_000_000)]),
            Some(92)
        );
        assert_eq!(tick(296, &[json!([93, [0, 1], 8, [1]])]), Some(93));
        assert_eq!(tick(272, &[json!([false, [0, 1]])]), None);
        use crate::protocol::stream::Writer;
        let mut command = Writer::new();
        command.vint(231).vint(0).long(0).bool(false).long(0).vint(0).vint(0);
        let mut zero = command.clone();
        zero.vint(0);
        assert_eq!(decode(&mut zero.into_bytes().as_slice()).unwrap().0, 231);
        // An unresolved element decoder cannot be treated as reading zero bytes.
        command.vint(1).vint(0);
        let bytes = command.into_bytes();
        let mut input = bytes.as_slice();
        assert!(decode(&mut input).is_none());
        assert_eq!(input, bytes);
    }

    #[test]
    fn primitives_and_bounds() {
        let fs: Vec<Field> = serde_json::from_value(json!([
            {"t":"bool"},{"t":"bool"},{"t":"int8"},{"t":"u16"},{"t":"u32"},
            {"t":"pair"},{"t":"string"},{"t":"blob"},
            {"t":"list","count":"vint","elem":[{"t":"bool"},{"t":"bool"}]}
        ]))
        .unwrap();
        let bytes = [
            3, 255, 1, 2, 0, 0, 0, 9, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 2, 0xff, 0x41, 255, 255,
            255, 255, 2, 1,
        ];
        let mut r = Reader {
            bytes: &bytes,
            pos: 0,
            bit: 0,
            budget: 100,
        };
        let v = r.fields(&fs, 0).unwrap();
        assert_eq!(
            v,
            json!([
                true,
                true,
                -1,
                258,
                9,
                [1, 2],
                [255, 65],
                null,
                [[true, false], [false, false]]
            ])
            .as_array()
            .unwrap()
            .clone()
        );
        assert_eq!(r.pos, bytes.len());
        for kind in [76, 78, 84, 85, 108, 117, 224, 275, 276, 277, 305] {
            let mut w = crate::protocol::stream::Writer::new();
            w.vint(kind);
            let bytes = w.into_bytes();
            let mut input = bytes.as_slice();
            assert!(decode(&mut input).is_none());
            assert_eq!(input, bytes);
        }
    }
}
