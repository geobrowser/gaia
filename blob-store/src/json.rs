//! Canonical JSON bytes: object keys sorted at every level, no whitespace.
//!
//! Postgres `jsonb` reorders keys, so a value read back from it only compares equal to the
//! value that was written once both are put in a fixed order.

use serde_json::Value;

pub fn canonical_bytes(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(v, &mut out);
    out
}

fn write(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(Value::String(k.clone()).to_string().as_bytes());
                out.push(b':');
                write(&map[k], out);
            }
            out.push(b'}');
        }
        Value::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write(x, out);
            }
            out.push(b']');
        }
        other => out.extend_from_slice(other.to_string().as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_matter() {
        let a = json!({"b": [1, {"y": 1, "x": 2}], "a": "s"});
        let b: Value = serde_json::from_str(r#"{"a":"s","b":[1,{"x":2,"y":1}]}"#).unwrap();
        assert_eq!(canonical_bytes(&a), canonical_bytes(&b));
        assert_eq!(
            String::from_utf8(canonical_bytes(&a)).unwrap(),
            r#"{"a":"s","b":[1,{"x":2,"y":1}]}"#
        );
    }
}
