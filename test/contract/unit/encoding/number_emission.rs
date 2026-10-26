use super::*;

fn fixtures() -> Vec<(String, Value)> {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../../docs/contracts/vectors.json")).unwrap();
    let mut cases = vectors["canonical"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["name"].as_str().unwrap().to_owned(), v["value"].clone()))
        .collect::<Vec<_>>();
    let numbers = (0..1024)
        .map(|n| json_codec::json!([n, -n, 9_007_199_254_740_991u64, -9_007_199_254_740_991i64]))
        .collect::<Vec<_>>();
    cases.push(("integer-array".into(), Value::Array(numbers)));
    let mixed = (0..256)
        .map(|n| {
            (
                format!("shared-prefix-{n:04}-\u{10000}\u{e000}"),
                json_codec::json!({
                    "\u{d7ff}": n, "\u{10000}": -n, "\u{10ffff}": 65536,
                    "\u{e000}": "\0\u{1}\u{8}\u{b}\u{c}\u{1f}\n\r\t\"\\中文",
                    "size": "18446744073709551615"
                }),
            )
        })
        .collect::<Map<_, _>>();
    cases.push(("mixed-unicode-object".into(), Value::Object(mixed)));
    cases
}

#[test]
fn complete_encoder_matches_frozen_baseline() {
    for (name, value) in fixtures() {
        let expected = baseline::canonical(&value).unwrap();
        assert_eq!(canonical_json_value(&value).unwrap(), expected, "{name}");
        assert_eq!(canonical_json(&value).unwrap(), expected, "{name}");
        for domain in ["state-page", "request", "version"] {
            let mut h = blake3::Hasher::new();
            h.update(format!("zxs:{domain}\0").as_bytes());
            h.update(&expected);
            assert_eq!(
                object_hash_value(domain, &value).unwrap(),
                h.finalize().to_hex().as_str()
            );
        }
    }
}

#[test]
fn integer_boundaries_and_unicode_keys_have_fixed_bytes() {
    let value = json_codec::json!({
        "\u{e000}": "18446744073709551615",
        "\u{10ffff}": 9_007_199_254_740_991u64,
        "\u{10000}": -9_007_199_254_740_991i64,
        "\u{d7ff}": [0, 1, -1, 9, 10, 99, 100, 65536],
        "\0\n": "\u{1}\u{8}\u{b}\u{c}\u{1f}\r\t\"\\/"
    });
    let expected = concat!(
        r#"{"\u0000\n":"\u0001\b\u000b\f\u001f\r\t\"\\/","#,
        "\"\u{d7ff}\":[0,1,-1,9,10,99,100,65536],",
        "\"\u{10000}\":-9007199254740991,\"\u{10ffff}\":9007199254740991,",
        "\"\u{e000}\":\"18446744073709551615\"}"
    );
    assert_eq!(canonical_json_value(&value).unwrap(), expected.as_bytes());
    assert_eq!(canonical_json(&value).unwrap(), expected.as_bytes());
    for text in [
        "-9007199254740992",
        "9007199254740992",
        "18446744073709551615",
        "-9223372036854775808",
        "1.0",
        "-0.0",
        "1.5",
    ] {
        let value: Value = json_codec::from_str(text).unwrap();
        for value in [value.clone(), json_codec::json!({"nested": [value]})] {
            let old = baseline::canonical(&value).unwrap_err();
            for new in [
                canonical_json_value(&value).unwrap_err(),
                canonical_json(&value).unwrap_err(),
            ] {
                assert_eq!(new.code(), old.code());
                assert_eq!(new.message(), old.message());
            }
        }
    }
}

#[test]
#[ignore = "manual complete canonical encoder comparison; run release, isolated, --nocapture"]
fn measure_complete_canonical_encoder() {
    use std::{hint::black_box, time::Instant};
    type Encoder = fn(&Value) -> Result<Vec<u8>>;
    let cases = fixtures();
    if let Some(path) = std::env::var_os("ZXS_CANONICAL_FIXTURES") {
        std::fs::write(path, json_codec::to_vec_pretty(&cases).unwrap()).unwrap();
    }
    println!("case,sample,implementation,iterations,elapsed_ns,bytes");
    for (name, value) in cases {
        let bytes = canonical_json_value(&value).unwrap();
        assert_eq!(baseline::canonical(&value).unwrap(), bytes);
        let iterations = if bytes.len() > 10_000 { 500 } else { 20_000 };
        let implementations: [(&str, Encoder); 2] = [
            ("baseline", baseline::canonical),
            ("direct", canonical_json_value),
        ];
        for (_, encode) in implementations {
            for _ in 0..100 {
                black_box(encode(black_box(&value)).unwrap());
            }
        }
        for sample in 0..12 {
            for index in 0..2 {
                let (implementation, encode) = implementations[(sample + index) % 2];
                let start = Instant::now();
                for _ in 0..iterations {
                    black_box(encode(black_box(&value)).unwrap());
                }
                println!(
                    "{name},{sample},{implementation},{iterations},{},{size}",
                    start.elapsed().as_nanos(),
                    size = bytes.len()
                );
            }
        }
    }
}
