use super::*;
use crate::r3::{Body, DisplayName};
use serde_json::{Value, json};

fn intent() -> Intent {
    Intent {
        container_id: Id::parse(&"1".repeat(32)).unwrap(),
        operation_id: Id::parse(&"2".repeat(32)).unwrap(),
        node_id: Id::parse(&"3".repeat(32)).unwrap(),
        seen_sequence: 0,
        target: Target::FirstMain {
            name: DisplayName::parse("测试.txt").unwrap(),
        },
        body: Body {
            length: BLOCK + 2,
            sha256: Digest::parse(&"0".repeat(64)).unwrap(),
        },
    }
}

fn response(i: &Intent) -> Value {
    json!({"upload_id":"a".repeat(64), "container_id":i.container_id, "operation_id":i.operation_id,
           "node_id":i.node_id, "target":i.target, "length":i.body.length, "sha256":i.body.sha256,
           "state":"open", "next_offset":BLOCK})
}

#[test]
fn flat_start_bounded_tail_and_exact_commit_use_different_scopes() {
    let i = intent();
    let u: UploadId = serde_json::from_value(json!("a".repeat(64))).unwrap();
    let begin = begin(&i).unwrap();
    assert_eq!(begin.request.method(), Method::POST);
    assert_eq!(begin.request.uri().path(), UPLOADS);
    assert_eq!(
        begin.request.headers()["zxs-op-id"],
        i.operation_id.as_str()
    );
    assert!(!begin.request.headers().contains_key("zxs-container-id"));
    assert!(!begin.request.headers().contains_key("zxs-operation"));
    let body: Value = serde_json::from_slice(&begin.bytes).unwrap();
    assert_eq!(
        body,
        json!({"container_id":i.container_id,"node_id":i.node_id,"target":i.target,
                            "length":i.body.length,"sha256":i.body.sha256})
    );

    let discover = status(&i, None).unwrap();
    assert_eq!(discover.request.uri().path(), UPLOADS);
    assert_eq!(
        discover.request.headers()["zxs-container-id"],
        i.container_id.as_str()
    );
    assert!(!discover.request.headers().contains_key("content-length"));
    assert!(discover.bytes.is_empty());
    assert_eq!(
        status(&i, Some(&u)).unwrap().request.uri().path(),
        format!("{UPLOADS}/{}", u.0)
    );
    let chunk = put(&i, &u, BLOCK, Bytes::from_static(b"hi")).unwrap();
    assert_eq!(chunk.request.uri().query(), Some("offset=8388608"));
    assert_eq!(chunk.request.headers()["content-length"], "2");
    assert_eq!(
        chunk.request.headers()["zxs-chunk-sha256"]
            .to_str()
            .unwrap(),
        hex(&Sha256::digest(b"hi"))
    );
    assert!(put(&i, &u, 1, Bytes::from_static(b"hi")).is_err());
    assert!(put(&i, &u, BLOCK, Bytes::from_static(b"h")).is_err());
    let commit = commit(&i, &u).unwrap();
    assert_eq!(
        commit.request.uri().path(),
        format!("{}/{}/commits", r3::ROOT, i.container_id)
    );
    assert!(!commit.request.headers().contains_key("zxs-container-id"));
    assert_eq!(
        serde_json::from_slice::<Value>(&commit.bytes).unwrap(),
        json!({"upload_id":u})
    );
}

#[test]
fn rejects_wrong_intent_and_impossible_or_noncanonical_progress() {
    let i = intent();
    let ok = response(&i);
    let parse = |v: &Value, id: Option<&UploadId>| {
        Progress::decode(&serde_json::to_vec(v).unwrap(), &i, id)
    };
    let p = parse(&ok, None).unwrap();
    assert_eq!(p.next_offset, BLOCK);
    for (key, bad) in [
        ("upload_id", json!("0".repeat(64))),
        ("upload_id", json!("A".repeat(64))),
        ("container_id", json!("4".repeat(32))),
        ("operation_id", json!("4".repeat(32))),
        ("node_id", json!("4".repeat(32))),
        ("target", json!({"kind":"first_main","name":"other"})),
        ("sha256", json!("b".repeat(64))),
        ("length", json!(0)),
        ("next_offset", json!(1)),
        ("next_offset", json!(BLOCK + 2)),
        ("state", json!("complete")),
        ("state", json!("committed")),
        ("extra", json!(true)),
    ] {
        let mut v = ok.clone();
        v[key] = bad;
        assert!(parse(&v, None).is_err(), "{key}");
    }
    let mut wrong = ok.clone();
    wrong["upload_id"] = json!("b".repeat(64));
    assert!(parse(&wrong, Some(&p.upload_id)).is_err());
    for state in ["complete", "committed"] {
        let mut v = ok.clone();
        v["next_offset"] = json!(i.body.length);
        v["state"] = json!(state);
        assert!(parse(&v, Some(&p.upload_id)).is_ok());
        assert_eq!(
            r3::outcome(
                &i,
                http::StatusCode::OK,
                &serde_json::to_vec(&v).unwrap(),
                true
            ),
            r3::Lookup::Unknown
        ); // progress is never a save_node receipt
    }
}
