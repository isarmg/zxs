use super::*;
use std::{error::Error as _, io};

#[derive(Debug, PartialEq)]
enum Outcome {
    Bytes(Vec<u8>),
    Business(storage_contract::ErrorCode, String, bool, Option<String>),
}

fn finish(response: Response, raw: Option<&RawValue>) -> io::Result<Outcome> {
    match response {
        Response::Ok { result } => decode_result(result, raw)
            .map(|result| Outcome::Bytes(result.bytes))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Response::Error {
            code,
            message,
            retryable,
            operation_id,
        } => Ok(Outcome::Business(code, message, retryable, operation_id)),
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct ErrorSignature {
    io_kind: io::ErrorKind,
    source_kind: &'static str,
    code: Option<storage_contract::ErrorCode>,
    json: Option<(usize, usize, String)>,
    chain: Vec<String>,
}

pub(crate) fn signature(error: io::Error) -> ErrorSignature {
    let inner = error.get_ref();
    let contract = inner.and_then(|source| source.downcast_ref::<storage_contract::Error>());
    let json = inner
        .and_then(|source| source.downcast_ref::<json_codec::Error>())
        .or_else(|| {
            contract
                .and_then(|error| error.source())
                .and_then(|source| source.downcast_ref::<json_codec::Error>())
        });
    let kind = if contract.is_some() {
        "contract"
    } else if json.is_some() {
        "json"
    } else {
        "message"
    };
    let code = contract.map(|error| error.code());
    let json = json.map(|error| {
        (
            error.line(),
            error.column(),
            format!("{:?}", error.classify()),
        )
    });
    let mut chain = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        chain.push(error.to_string());
        source = error.source();
    }
    ErrorSignature {
        io_kind: error.kind(),
        source_kind: kind,
        code,
        json,
        chain,
    }
}

#[test]
fn borrowed_read_preserves_strict_admission_and_error_chains() {
    let mut cases: Vec<Vec<u8>> = vec![];
    for root in [
        "null",
        "true",
        "1",
        "\"ok\"",
        "[]",
        "[\"ok\",{\"bytes\":[1]}]",
        "{}",
    ] {
        cases.push(root.as_bytes().to_vec())
    }
    let results = [
        "null",
        "false",
        "0",
        "\"bytes\"",
        "[]",
        "[[1]]",
        "[[]]",
        "{}",
        "{\"bytes\":[]}",
        "{\"bytes\":[0,1,255]}",
        "{\"bytes\":null}",
        "{\"bytes\":true}",
        "{\"bytes\":\"a\"}",
        "{\"bytes\":{}}",
        "{\"bytes\":[256]}",
        "{\"bytes\":[-1]}",
        "{\"bytes\":[-0]}",
        "{\"bytes\":[1.0]}",
        "{\"bytes\":[1e0]}",
        "{\"bytes\":[1e400]}",
        "{\"bytes\":[18446744073709551615]}",
        "{\"bytes\":[18446744073709551616]}",
        "{\"bytes\":[-9223372036854775808]}",
        "{\"bytes\":[9007199254740991,9007199254740993]}",
        "{\"bytes\":[null]}",
        "{\"bytes\":[[]]}",
        "{\"bytes\":[{}]}",
        "{\"bytes\":[true]}",
        "{\"bytes\":[\"1\"]}",
        "{\"bytes\":[],\"bytes\":[1]}",
        "{\"bytes\":[],\"b\\u0079tes\":[1]}",
        "{\"b\\u0079tes\":[1]}",
        "{\"z\":0,\"bytes\":[256]}",
        "{\"bytes\":[256],\"z\":0}",
        "{\"a\":0,\"bytes\":[256]}",
        "{\"bytes\":[256],\"a\":0}",
        "{\"bytes\":[],\"z\":{\"a\":1,\"a\":2}}",
        "{\"bytes\":[],\"z\":\"\\ud800\"}",
        "{\"bytes\":[],\"z\":\"\\ud83d\\ude00\"}",
    ];
    for result in results {
        for status in ["\"ok\"", "\"error\"", "\"other\"", "null", "0"] {
            for input in [
                format!("{{\"status\":{status},\"result\":{result}}}"),
                format!("{{\"res\\u0075lt\":{result},\"stat\\u0075s\":{status}}}"),
            ] {
                cases.push(input.into_bytes())
            }
        }
    }
    for body in [
        "{\"status\":\"ok\"}",
        "{\"result\":{\"bytes\":[]}}",
        "{\"status\":\"ok\",\"status\":\"error\",\"result\":null}",
        "{\"status\":\"ok\",\"result\":{\"bytes\":[]},\"unknown\":1}",
        "{\"status\":\"error\",\"code\":\"CONTENT_UNAVAILABLE\",\"message\":\"failed\",\"retryable\":false,\"operation_id\":null}",
        "{\"status\":\"error\",\"code\":\"CONTENT_UNAVAILABLE\",\"message\":\"failed\",\"retryable\":false,\"operation_id\":\"op1\",\"result\":{\"bytes\":[]}}",
    ] {
        cases.push(body.as_bytes().to_vec())
    }
    for bad in [
        r#"{"bytes":[],"b\u0079tes":1e400}"#,
        r#"{"bytes":[],"bytes":"\ud800"}"#,
        r#"{"bytes":[],"\ud800":1}"#,
        r#"{"bytes":[],"\udc00":1}"#,
        r#"{"bytes":[],"a":0,"\u0061":1e400}"#,
    ] {
        cases.push(format!(r#"{{"status":"ok","result":{bad}}}"#).into_bytes());
    }
    for op in ["null", r#""op1""#, "0", "[]", "{}", "false", r#""\ud800""#] {
        for result in ["", r#", "result":{"bytes":[]}"#] {
            cases.push(format!(r#"{{"status":"error","code":"CONTENT_UNAVAILABLE","message":"failed","retryable":false,"operation_id":{op}{result}}}"#).into_bytes());
        }
    }
    for bad in [
        r#"{"status":"ok","sta\u0074us":1e400,"result":{"bytes":[]}}"#,
        r#"{"status":"ok","result":{},"res\u0075lt":"\ud800"}"#,
        r#"{"status":"ok","result":{"bytes":[]},"\ud800":0}"#,
    ] {
        cases.push(bad.as_bytes().to_vec());
    }
    let normal = b"{\"status\":\"ok\",\"result\":{\"bytes\":[1]}}";
    for suffix in [&b" "[..], b" true", b"x", b"\0"] {
        let mut raw = normal.to_vec();
        raw.extend_from_slice(suffix);
        cases.push(raw)
    }
    cases.push(b"{\"status\":\"ok\",\"result\":{\"bytes\":[],\"z\":\"\xff\"}}".to_vec());
    for depth in [124, 125, 126, 127, 128, 129] {
        cases.push(
            format!(
                "{{\"status\":\"ok\",\"result\":{{\"bytes\":[],\"z\":{}0{}}}}}",
                "[".repeat(depth),
                "]".repeat(depth)
            )
            .into_bytes(),
        )
    }

    for fields in [
        r#""code":0,"message":0,"operation_id":0,"retryable":0"#,
        r#""retryable":0,"operation_id":0,"message":0,"code":0"#,
        r#""z":0,"code":0,"message":0,"operation_id":0,"retryable":0,"a":0"#,
        r#""a":0,"retryable":0,"operation_id":0,"message":0,"code":0,"result":{}"#,
    ] {
        cases.push(format!(r#"{{"status":"error",{fields}}}"#).into_bytes());
    }
    for operation in [
        "",
        r#","operation_id":"""#,
        r#","operation_id":"\u4f60\u597d""#,
    ] {
        cases.push(format!(r#"{{"status":"error","code":"FILE_WRITE_BUSY","message":"busy","retryable":true{operation}}}"#).into_bytes());
    }
    for size in [0, 1, 255, 256, 65536] {
        let bytes: Vec<u8> = (0..size).map(|i| i as u8).collect();
        cases.push(
            json_codec::to_vec(&Response::Ok {
                result: json_codec::json!({ "bytes":bytes }),
            })
            .unwrap(),
        );
    }
    assert_eq!(cases.len(), 448);
    for raw in cases {
        let old = storage_contract::decode_strict::<Response>(&raw)
            .map_err(io::Error::other)
            .and_then(|response| finish(response, None))
            .map_err(signature);
        let new = decode(&raw)
            .map_err(io::Error::other)
            .and_then(|(response, raw)| finish(response, raw))
            .map_err(signature);
        assert_eq!(new, old, "payload={:?}", String::from_utf8_lossy(&raw));
    }
}
