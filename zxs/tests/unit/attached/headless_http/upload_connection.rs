use super::*;
use crate::test_support::PrivateTemp;
use std::path::Path;

const UPLOADS: &str = "/zxs/v1/uploads";
const C: &str = "11111111111111111111111111111111";
const O: &str = "55555555555555555555555555555555";
const N: &str = "66666666666666666666666666666666";
const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn upload_path_challenges_only_bearer_before_binding_storage() {
    for authorization in [None, Some(FIXTURE_BASIC)] {
        local_exchange(
            |_, _, _| panic!("upload must not fall through to legacy dispatch"),
            |stream, reader| {
                let fields = format!("Zxs-Container-Id: {C}\r\nZxs-Op-Id: {O}\r\n");
                stream
                    .write_all(get(UPLOADS, authorization, &fields).as_bytes())
                    .unwrap();
                let (headers, bytes) = response(reader);
                assert!(headers.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
                assert!(headers.contains("WWW-Authenticate: Bearer\r\n"));
                assert!(!headers.contains("WWW-Authenticate: Basic"));
                assert_eq!(bytes, [] as [u8; 0]);
                closed(reader);
            },
        );
    }
}

#[test]
fn explicit_upload_and_original_receipt_share_the_actual_listener_and_store() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let archive = PrivateTemp::under(Path::new("/dev/shm"));
    let store = seeded_browse_store(&local, &archive);
    exchange_with_store(
        Some(&store),
        |request, inbound, live| {
            workspace_protocol::http::dispatch_request(&store, request, inbound, live)
        },
        |stream, reader| {
            let bearer = login(stream, reader);
            let mut send = |method: &str, path: &str, fields: &str, body: &str| {
                write!(
                    stream,
                    "{method} {path} HTTP/1.1\r\nHost: fixture\r\nAuthorization: {bearer}\r\n\
                     Content-Length: {}\r\n{fields}\r\n{body}",
                    body.len()
                )
                .unwrap();
                let (headers, bytes) = response(reader);
                assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
                assert!(headers.contains("Connection: keep-alive\r\n"));
                String::from_utf8(bytes).unwrap()
            };
            let op = format!("Zxs-Op-Id: {O}\r\n");
            let json = format!("{op}Content-Type: application/json\r\n");
            let lookup = format!("{op}Zxs-Container-Id: {C}\r\n");
            let begin = format!(
                r#"{{"container_id":"{C}","node_id":"{N}","target":{{"kind":"branch","branch":"main","expected_head":"{NODE}"}},"length":3,"sha256":"{DIGEST}"}}"#
            );
            let opened = send("POST", UPLOADS, &json, &begin);
            assert!(opened.contains(r#""next_offset":0,"state":"open""#));
            // A lost start response can be recovered without a mutation.
            assert_eq!(send("GET", UPLOADS, &lookup, ""), opened);
            let id = opened
                .strip_prefix(r#"{"upload_id":""#)
                .unwrap()
                .split_once('"')
                .unwrap()
                .0;
            let put_headers = format!(
                "{lookup}Zxs-Chunk-Sha256: {DIGEST}\r\nContent-Type: application/octet-stream\r\n"
            );
            let complete = send(
                "PUT",
                &format!("{UPLOADS}/{id}?offset=0"),
                &put_headers,
                "abc",
            );
            assert!(complete.contains(r#""next_offset":3,"state":"complete""#));
            let receipt = send(
                "POST",
                &format!("{CONTAINER}/commits"),
                &json,
                &format!(r#"{{"upload_id":"{id}"}}"#),
            );
            assert!(receipt.contains(r#""kind":"save_node""#));
            assert!(!receipt.contains("upload_id"));
            assert_eq!(
                send("GET", &format!("{CONTAINER}/operations/{O}"), "", ""),
                receipt
            );
        },
    );
}

// An older listener also returns an empty, uncached 404 for an unknown path.
// That reply must never authorize a client to treat a pending upload as absent.
#[test]
fn only_an_authenticated_negative_progress_get_marks_exact_absence() {
    let local = PrivateTemp::under(&std::env::temp_dir());
    let archive = PrivateTemp::under(Path::new("/dev/shm"));
    let store = seeded_browse_store(&local, &archive);
    let by_id = format!("{UPLOADS}/{}", "88".repeat(32));
    for (method, path, marked) in [
        ("GET", UPLOADS, true),
        ("GET", by_id.as_str(), true),
        ("PUT", UPLOADS, false),
        ("GET", "/zxs/v1/unimplemented", false),
    ] {
        exchange_with_store(
            Some(&store),
            |_, _, _| panic!("an unrelated operation must not be selected"),
            |stream, reader| {
                let bearer = login(stream, reader);
                write!(
                    stream,
                    "{method} {path} HTTP/1.1\r\nHost: fixture\r\nAuthorization: {bearer}\r\n\
                     Zxs-Container-Id: {C}\r\nZxs-Op-Id: {O}\r\n\r\n"
                )
                .unwrap();
                let (headers, body) = response(reader);
                assert!(headers.starts_with("HTTP/1.1 404 Not Found\r\n"));
                let mut stamps = headers
                    .lines()
                    .filter_map(|line| line.strip_prefix("Zxs-Upload-Status: "));
                assert_eq!(stamps.next(), marked.then_some("not-found"));
                assert_eq!(stamps.next(), None);
                assert!(headers.contains("Connection: close\r\n"));
                assert_eq!(body, [] as [u8; 0]);
                closed(reader);
            },
        );
    }
}
