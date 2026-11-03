use super::*;
use std::error::Error as _;

fn signature(error: Error) -> (ErrorCode, Vec<String>) {
    let mut messages = vec![error.to_string()];
    let mut next = error.source();
    while let Some(source) = next {
        messages.push(source.to_string());
        next = source.source();
    }
    (error.code(), messages)
}

#[test]
fn borrowed_strict_json_preserves_values_and_original_errors() {
    let mut cases = vec![
        br#"null"#.to_vec(),
        br#"true"#.to_vec(),
        br#"[]"#.to_vec(),
        br#"{}"#.to_vec(),
        br#"{"a":[0,1,255],"b":"\ud83d\ude00"}"#.to_vec(),
        br#"{"a":0,"\u0061":1e400}"#.to_vec(),
        br#"{"a":0,"a":"\ud800"}"#.to_vec(),
        br#"{"\ud800":0}"#.to_vec(),
        br#"{"\udc00":0}"#.to_vec(),
        br#"{"a":[{"x":1,"x":2}]}"#.to_vec(),
        br#"-0"#.to_vec(),
        br#"1.0"#.to_vec(),
        br#"1e0"#.to_vec(),
        br#"1e400"#.to_vec(),
        br#"18446744073709551615"#.to_vec(),
        br#"18446744073709551616"#.to_vec(),
        br#"-9223372036854775808"#.to_vec(),
        br#"9007199254740993"#.to_vec(),
        br#"0 true"#.to_vec(),
        b"\xff".to_vec(),
        b"\"\xff\"".to_vec(),
    ];
    for depth in [126, 127, 128, 129] {
        cases.push(format!("{}0{}", "[".repeat(depth), "]".repeat(depth)).into_bytes());
    }
    cases.push(json_codec::to_vec(&(0..65536).map(|i| i as u8).collect::<Vec<_>>()).unwrap());
    for input in cases {
        let old = strict_json(&input).map_err(signature);
        let new = StrictJson::parse(&input)
            .and_then(StrictJson::decode::<json_codec::Value>)
            .map_err(signature);
        assert_eq!(old, new, "input={:?}", String::from_utf8_lossy(&input));
    }
}

#[test]
fn admitted_json_can_borrow_strings_without_owning_the_payload() {
    let payload = String::from(r#"{"name":"borrowed"}"#);
    let fields: std::collections::BTreeMap<&str, &str> = StrictJson::parse(payload.as_bytes())
        .unwrap()
        .decode()
        .unwrap();
    let value = fields["name"];
    assert_eq!(value, "borrowed");
    let range = payload.as_bytes().as_ptr_range();
    assert!(range.contains(&value.as_ptr()));
}
