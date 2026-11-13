//! Firestore REST 값 인코딩/디코딩
use serde_json::{json, Map, Value};

pub fn encode_value(v: &Value) -> Value {
    match v {
        Value::Null => json!({ "nullValue": null }),
        Value::Bool(b) => json!({ "booleanValue": b }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                json!({ "integerValue": i.to_string() })
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                if f.fract() == 0.0 && f.abs() < 9.0e15 {
                    json!({ "integerValue": (f as i64).to_string() })
                } else {
                    json!({ "doubleValue": f })
                }
            }
        }
        Value::String(s) => json!({ "stringValue": s }),
        Value::Array(a) => json!({ "arrayValue": { "values": a.iter().map(encode_value).collect::<Vec<_>>() } }),
        Value::Object(o) => json!({ "mapValue": { "fields": encode_fields(o) } }),
    }
}

pub fn encode_fields(o: &Map<String, Value>) -> Value {
    Value::Object(o.iter().map(|(k, v)| (k.clone(), encode_value(v))).collect())
}

pub fn decode_value(v: &Value) -> Value {
    let Some(o) = v.as_object() else { return Value::Null };
    if o.contains_key("nullValue") {
        return Value::Null;
    }
    if let Some(b) = o.get("booleanValue") {
        return b.clone();
    }
    if let Some(i) = o.get("integerValue") {
        let n: i64 = i.as_str().and_then(|s| s.parse().ok()).or_else(|| i.as_i64()).unwrap_or(0);
        return json!(n);
    }
    if let Some(d) = o.get("doubleValue") {
        return d.clone();
    }
    if let Some(s) = o.get("stringValue") {
        return s.clone();
    }
    if let Some(s) = o.get("timestampValue") {
        return s.clone();
    }
    if let Some(a) = o.get("arrayValue") {
        return Value::Array(a.get("values").and_then(|x| x.as_array()).map(|x| x.iter().map(decode_value).collect()).unwrap_or_default());
    }
    if let Some(m) = o.get("mapValue") {
        return decode_fields(m.get("fields").unwrap_or(&Value::Null));
    }
    Value::Null
}

pub fn decode_fields(fields: &Value) -> Value {
    let mut out = Map::new();
    if let Some(o) = fields.as_object() {
        for (k, v) in o {
            out.insert(k.clone(), decode_value(v));
        }
    }
    Value::Object(out)
}

/// 문서 → { id, ...필드 }
pub fn decode_doc(doc: &Value) -> Option<Value> {
    let name = doc.get("name")?.as_str()?;
    let id = name.rsplit('/').next()?.to_string();
    let mut v = decode_fields(doc.get("fields").unwrap_or(&Value::Null));
    v.as_object_mut()?.insert("id".into(), Value::String(id));
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = json!({ "a": 1, "b": 2.5, "c": "x", "d": [true, null], "e": { "f": 3.0 } });
        let enc = encode_fields(v.as_object().unwrap());
        let dec = decode_fields(&enc);
        assert_eq!(dec["a"], 1);
        assert_eq!(dec["b"], 2.5);
        assert_eq!(dec["d"][0], true);
        assert_eq!(dec["e"]["f"], 3);
    }
}
