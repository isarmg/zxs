// Frozen encoder from 23dd3639 for differential tests and manual measurements.
use super::*;

pub(super) fn canonical(value: &Value) -> Result<Vec<u8>> {
    crate::integer_json::validate(value)?;
    emit_canonical_json(value)
}

fn emit_canonical_json(value: &Value) -> Result<Vec<u8>> {
    fn number(n: &Number) -> Result<String> {
        const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
        let safe = n
            .as_i64()
            .is_some_and(|v| v.unsigned_abs() <= MAX_SAFE_INTEGER)
            || n.as_u64().is_some_and(|v| v <= MAX_SAFE_INTEGER);
        if !safe {
            return Err(Error::new(
                crate::ErrorCode::InvalidJson,
                "JSON numbers must be safe integers; encode u64 fields as decimal strings",
            ));
        }
        Ok(n.to_string())
    }
    fn emit(v: &Value, out: &mut Vec<u8>) -> Result<()> {
        match v {
            Value::Null => out.extend_from_slice(b"null"),
            Value::Bool(true) => out.extend_from_slice(b"true"),
            Value::Bool(false) => out.extend_from_slice(b"false"),
            Value::String(s) => json_codec::to_writer(&mut *out, s)?,
            Value::Number(n) => out.extend(number(n)?.as_bytes()),
            Value::Array(a) => {
                out.push(b'[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    emit(v, out)?;
                }
                out.push(b']');
            }
            Value::Object(o) => {
                out.push(b'{');
                let mut keys = o.keys().collect::<Vec<_>>();
                keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    json_codec::to_writer(&mut *out, k)?;
                    out.push(b':');
                    emit(&o[k], out)?;
                }
                out.push(b'}');
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    emit(value, &mut out)?;
    Ok(out)
}
