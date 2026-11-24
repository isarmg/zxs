use super::*;

const C: &str = "11111111111111111111111111111111";
const O: &str = "22222222222222222222222222222222";
const N: &str = "33333333333333333333333333333333";
const U: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const SHA: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const SCOPE: Scope = Scope {
    pool: [6; 16],
    epoch: [7; 16],
};

#[test]
fn exactly_the_new_family_is_routed_with_precise_control_or_raw_lengths() {
    assert!(is_target(ROOT));
    assert!(is_target(&format!("{ROOT}/{U}?offset=0")));
    assert!(is_target(&format!("{CONTAINERS}{C}/commits")));
    for legacy in [
        format!("{CONTAINERS}{C}/branches/main/nodes"),
        format!("{CONTAINERS}{C}/nodes/{N}/data"),
        format!("{CONTAINERS}{C}/operations/{O}"),
        format!("{ROOT}suffix"),
    ] {
        assert!(!is_target(&legacy));
    }
    assert!(matches!(
        frame("GET", ROOT, &[(OP, O), (CONTAINER, C)]),
        Ok(Frame::Status { upload: None, .. })
    ));
    assert!(matches!(
        frame(
            "GET",
            &format!("{ROOT}/{U}"),
            &[(OP, O), (CONTAINER, C), ("content-length", "0")]
        ),
        Ok(Frame::Status {
            upload: Some(_),
            ..
        })
    ));
    let heads = [
        (OP, O),
        (CONTAINER, C),
        (CHUNK_DIGEST, SHA),
        ("content-type", "application/octet-stream"),
        ("content-length", "8388608"),
    ];
    let put = frame("PUT", &format!("{ROOT}/{U}?offset=8388608"), &heads)
        .ok()
        .unwrap();
    assert!(put.raw());
    assert_eq!(put.extent(), CHUNK);
    let expected = Chunk::new(
        mutation(Id::parse(C).unwrap(), Id::parse(O).unwrap()),
        [0x44; 32],
        CHUNK,
        CHUNK,
        [0x55; 32],
    );
    assert!(matches!(put, Frame::Put(chunk) if chunk == expected));
    let controls = [
        (OP, O),
        ("content-type", "application/json"),
        ("content-length", "9"),
    ];
    assert!(matches!(
        frame("POST", ROOT, &controls),
        Ok(Frame::Begin { length: 9, .. })
    ));
    assert!(matches!(
        frame("POST", &format!("{CONTAINERS}{C}/commits"), &controls),
        Ok(Frame::Commit { length: 9, .. })
    ));
}

#[test]
fn duplicate_legacy_headers_wrong_scope_and_noncanonical_offsets_are_rejected() {
    let base = [(OP, O), (CONTAINER, C)];
    assert!(matches!(
        frame("GET", ROOT, &[("zxs-operation", O), (CONTAINER, C)]),
        Err(Problem::Malformed)
    ));
    for extras in [
        [(OP, O), ("content-length", "0")],
        [(CONTAINER, C), ("content-length", "0")],
        [("content-length", "0"), ("Content-Length", "0")],
        [("content-length", "1"), ("connection", "close")],
        [("content-encoding", "identity"), ("connection", "close")],
        [("range", "bytes=0-1"), ("connection", "close")],
        [
            ("content-type", "application/json"),
            ("connection", "close"),
        ],
    ] {
        let mut combined = base.to_vec();
        combined.extend(extras);
        assert!(matches!(
            frame("GET", ROOT, &combined),
            Err(Problem::Malformed)
        ));
    }
    for invalid in [
        "",
        "0&other=1",
        "00",
        "01",
        "+1",
        "-1",
        "18446744073709551616",
    ] {
        let h = [
            (OP, O),
            (CONTAINER, C),
            (CHUNK_DIGEST, SHA),
            ("content-type", "application/octet-stream"),
            ("content-length", "1"),
        ];
        assert!(matches!(
            frame("PUT", &format!("{ROOT}/{U}?offset={invalid}"), &h),
            Err(Problem::Malformed)
        ));
    }
    assert!(matches!(
        frame("DELETE", ROOT, &base),
        Err(Problem::Missing)
    ));
    assert!(matches!(
        frame("GET", &format!("{ROOT}/{}", "0".repeat(64)), &base),
        Err(Problem::Malformed)
    ));
}

#[test]
fn json_allows_field_order_not_duplicate_keys_aliases_or_another_object() {
    let begin = format!(
        r#"{{"sha256":"{SHA}","length":8,"target":{{"name":"文档","kind":"first_main"}},"node_id":"{N}","container_id":"{C}"}}"#
    );
    let op = Id::parse(O).unwrap();
    let plan = begin_json(&begin, SCOPE, op).unwrap();
    assert_eq!(plan.scope, SCOPE);
    assert_eq!(plan.mutation, mutation(Id::parse(C).unwrap(), op));
    assert_eq!(
        plan.target,
        Target::FirstMain {
            name: "文档".to_owned()
        }
    );
    for bad in [
        begin.replace("\"length\":8", "\"length\":8,\"length\":8"),
        begin.replace("\"length\":8", "\"length\":1.0"),
        begin.replace("\"length\":8", "\"length\":08"),
        begin.replace("\"length\":8", "\"length\":-1"),
        begin.replace("\"length\":8", "\"unknown\":8"),
        begin.replace(
            "\"kind\":\"first_main\"",
            "\"kind\":\"first_main\",\"branch\":\"main\"",
        ),
        begin.replace("文档", "a/b"),
        begin.replace(N, &"0".repeat(32)),
        begin.clone() + "{}",
    ] {
        assert!(begin_json(&bad, SCOPE, op).is_none(), "{bad}");
    }
    let commit = format!(r#"{{"upload_id":"{U}"}}"#);
    assert_eq!(commit_json(&commit), Some([0x44; 32]));
    for bad in [
        format!(r#"{{"upload_id":"{U}","upload_id":"{U}"}}"#),
        format!(r#"{{"uploadId":"{U}"}}"#),
        commit + "null",
    ] {
        assert!(commit_json(&bad).is_none());
    }
}
