use super::*;
use crate::{
    container_store::{Body as NodeBody, Node},
    headless_auth::{FIXTURE_BASIC, fixture_account},
    headless_http::{Outcome, Problem, head, profile, resource, write_outcome},
    test_support::PrivateTemp,
    workspace_protocol::{self, Request},
};
use std::fs;

const TARGET: &str = "/zxs/v1/containers/11111111111111111111111111111111/nodes/22222222222222222222222222222222/payload";
const SHA: [u8; 32] = [0xcd; 32];

fn bytes(file: File, length: u64) -> NodeBytes {
    NodeBytes {
        file,
        node: Node {
            prior: None,
            original_branch: "main".into(),
            body: NodeBody {
                length,
                sha256: SHA,
            },
            sequence: 1,
            request: [2; 16],
        },
    }
}

fn framed(output: &[u8]) -> (&str, &[u8]) {
    let cut = output
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap()
        + 4;
    (std::str::from_utf8(&output[..cut]).unwrap(), &output[cut..])
}

#[test]
fn partial_and_complete_headers_are_distinct_and_have_exact_extent() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let path = temp.0.join("opaque");
    fs::write(&path, b"abcdef").unwrap();
    let etag = format!("ETag: \"{}\"\r\n", "cd".repeat(32));
    let mut out = Vec::new();
    let Request::NodePayload(range) =
        workspace_protocol::payload_request("GET", TARGET, &[("Range", "bytes=1-3")]).unwrap()
    else {
        panic!("wrong typed read");
    };
    write_payload(
        &mut out,
        PayloadReply {
            bytes: bytes(File::open(&path).unwrap(), 6),
            window: range.window(6, SHA),
        },
        false,
    )
    .unwrap();
    let (head, body) = framed(&out);
    assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"));
    assert!(head.contains("Content-Range: bytes 1-3/6\r\n"));
    assert!(head.contains("Content-Length: 3\r\n"));
    assert!(head.contains(&etag));
    assert!(!head.contains("Zxs-Sha256"));
    assert_eq!(body, b"bcd");

    out.clear();
    write_payload(
        &mut out,
        PayloadReply {
            bytes: bytes(File::open(&path).unwrap(), 6),
            window: PayloadWindow::Full,
        },
        false,
    )
    .unwrap();
    let (head, body) = framed(&out);
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("Content-Length: 6\r\n"));
    assert!(head.contains(&etag));
    assert!(head.contains(&format!("Zxs-Sha256: {}\r\n", "cd".repeat(32))));
    assert!(!head.contains("Content-Range"));
    assert_eq!(body, b"abcdef");
}

#[test]
fn only_qualified_unsatisfiable_shape_and_no_payload_bytes() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let path = temp.0.join("opaque");
    fs::write(&path, b"").unwrap();
    let mut out = Vec::new();
    write_payload(
        &mut out,
        PayloadReply {
            bytes: bytes(File::open(path).unwrap(), 0),
            window: PayloadWindow::Unsatisfiable,
        },
        false,
    )
    .unwrap();
    let (head, body) = framed(&out);
    assert!(head.starts_with("HTTP/1.1 416 Range Not Satisfiable\r\n"));
    assert!(head.contains("Content-Range: bytes */0\r\n"));
    assert!(head.contains("Content-Length: 0\r\n"));
    assert!(!head.contains("ETag:"));
    assert_eq!(body, b"");
}

#[test]
fn no_legacy_auth_or_unauthenticated_metadata_on_the_new_range_path() {
    let account = fixture_account();
    for auth in [None, Some(FIXTURE_BASIC), Some("Bearer invalid")] {
        let line = auth.map_or_else(String::new, |value| format!("Authorization: {value}\r\n"));
        let raw =
            format!("GET {TARGET} HTTP/1.1\r\nHost: fixture\r\nRange: bytes=4-9\r\n{line}\r\n");
        let head = head(raw.as_bytes()).ok().unwrap();
        let answer = resource(
            &account,
            &head,
            &mut io::empty(),
            profile(&head),
            &|| false,
            &|_, _, _| panic!("no selected node before valid Bearer"),
        )
        .unwrap();
        assert!(matches!(
            answer,
            Outcome::Failed(Problem::BearerUnauthorized)
        ));
        let mut output = Vec::new();
        write_outcome(&mut output, answer, false).unwrap();
        let (headers, body) = framed(&output);
        assert!(headers.contains("WWW-Authenticate: Bearer\r\n"));
        assert!(!headers.contains("ETag:"));
        assert!(!headers.contains("Content-Range"));
        assert_eq!(body, b"");
    }
}
