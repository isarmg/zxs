use super::*;
use crate::headless_auth::{FIXTURE_BASIC, fixture_account};
use std::{
    net::TcpListener,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

const LIST: &str = "/zxs/v1/containers";
const RECEIPT: &str = "/zxs/v1/containers/11111111111111111111111111111111/operations/22222222222222222222222222222222";
const CONTAINER: &str = "/zxs/v1/containers/11111111111111111111111111111111";
const SAVE: &str = "/zxs/v1/containers/11111111111111111111111111111111/branches/main/nodes";
const NODE: &str = "33333333333333333333333333333333";

fn local_exchange(
    dispatch: impl Fn(Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome + Send,
    inspect: impl FnOnce(&mut TcpStream, &mut BufReader<TcpStream>),
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            serve_connection(
                &fixture_account(),
                &stream,
                &|| false,
                &ReuseBudget::default(),
                dispatch,
            )
            .unwrap();
        });
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        inspect(&mut stream, &mut reader);
        drop(reader);
        drop(stream);
        worker.join().unwrap();
    });
}

fn response(reader: &mut BufReader<TcpStream>) -> (String, Vec<u8>) {
    let raw = read_head(reader, || Ok(())).unwrap().unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let count = raw
        .split("\r\n")
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mut body = vec![0; count];
    reader.read_exact(&mut body).unwrap();
    (raw, body)
}

fn login(stream: &mut TcpStream, reader: &mut BufReader<TcpStream>) -> String {
    write!(
        stream,
        "POST /zxs/v1/sessions HTTP/1.1\r\nHost: fixture\r\nContent-Length: 0\r\nAuthorization: {FIXTURE_BASIC}\r\n\r\n"
    )
    .unwrap();
    let (headers, body) = response(reader);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(headers.contains("Cache-Control: no-store\r\n"));
    assert!(headers.contains("Content-Type: application/json\r\n"));
    assert!(headers.contains("Connection: keep-alive\r\n"));
    let body = String::from_utf8(body).unwrap();
    let token = body
        .strip_prefix(r#"{"access_token":""#)
        .and_then(|tail| tail.strip_suffix(r#"","token_type":"Bearer","expires_in":900}"#))
        .unwrap();
    assert_eq!(token.len(), 43);
    format!("Bearer {token}")
}

fn get(path: &str, auth: Option<&str>, extra: &str) -> String {
    let mut head = format!("GET {path} HTTP/1.1\r\nHost: fixture\r\n{extra}");
    if let Some(auth) = auth {
        use std::fmt::Write;
        write!(head, "Authorization: {auth}\r\n").unwrap();
    }
    head.push_str("\r\n");
    head
}

fn receipt(auth: Option<&str>) -> String {
    get(RECEIPT, auth, "")
}

fn list(auth: Option<&str>) -> String {
    get(LIST, auth, "Zxs-View: compact\r\n")
}

fn delete(auth: &str) -> String {
    format!(
        "DELETE {CONTAINER} HTTP/1.1\r\nHost: fixture\r\nAuthorization: {auth}\r\n\
         Content-Length: 0\r\nZxs-Operation: 22222222222222222222222222222222\r\n\
         Zxs-Base-Generation: 1\r\n\r\n"
    )
}

fn put_body(auth: &str) -> String {
    let digest = "ab".repeat(32);
    format!(
        "POST {SAVE} HTTP/1.1\r\nHost: fixture\r\nAuthorization: {auth}\r\n\
         Content-Type: application/octet-stream\r\nContent-Length: 4\r\n\
         Zxs-Operation: 22222222222222222222222222222222\r\n\
         Zxs-Node: 33333333333333333333333333333333\r\n\
         Zxs-Base-Node: 0\r\nZxs-Initial-Name: 66697874757265\r\nZxs-Sha256: {digest}\r\n\r\nbody"
    )
}

fn control_write(auth: &str, path: &str, body: &str, metadata: bool) -> String {
    let base = if metadata {
        "Zxs-Base-Generation: 1\r\n"
    } else {
        ""
    };
    format!(
        "PUT {path} HTTP/1.1\r\nHost: fixture\r\nAuthorization: {auth}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\
         Zxs-Operation: 22222222222222222222222222222222\r\n{base}\r\n{body}",
        body.len()
    )
}

fn closed(reader: &mut BufReader<TcpStream>) {
    assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
}

#[test]
fn reused_receipt_reads_keep_original_replies_and_never_lend_authority() {
    let calls = AtomicUsize::new(0);
    local_exchange(
        |request, inbound, live| {
            assert!(live());
            assert!(matches!(request, Request::Receipt { .. }));
            assert!(matches!(inbound, Incoming::Empty));
            if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                Outcome::ReceiptAbsent
            } else {
                Outcome::Json(b"{}".to_vec())
            }
        },
        |stream, reader| {
            let auth = login(stream, reader);
            let wire = receipt(Some(&auth)) + &receipt(Some(&auth)) + &receipt(None);
            stream.write_all(wire.as_bytes()).unwrap();
            let (headers, body) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 404 Not Found\r\n"));
            assert!(headers.contains("Connection: keep-alive\r\n"));
            assert_eq!(body, br#"{"state":"not_found"}"#);
            let (headers, body) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(headers.contains("Connection: keep-alive\r\n"));
            assert_eq!(body, b"{}");
            let (headers, body) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
            assert!(headers.contains("Connection: close\r\n"));
            assert_eq!(body, [] as [u8; 0]);
            closed(reader);
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[test]
fn valid_bearer_cannot_authorize_legacy_writes_or_data_or_drain_their_body() {
    let blocked: [fn(&str) -> String; 6] = [
        put_body,
        delete,
        |auth| get(&format!("{CONTAINER}/nodes/{NODE}/data"), Some(auth), ""),
        |auth| {
            control_write(
                auth,
                &format!("{CONTAINER}/name"),
                "{\"name\":\"new\"}",
                true,
            )
        },
        |auth| control_write(auth, &format!("{CONTAINER}/tags"), "[]", true),
        |auth| {
            control_write(
                auth,
                &format!("{CONTAINER}/branches/44444444444444444444444444444444"),
                &format!("{{\"from\":\"{NODE}\"}}"),
                false,
            )
        },
    ];
    for request in blocked {
        local_exchange(
            |_, _, _| panic!("no legacy mutable or body alias can dispatch as Bearer"),
            |stream, reader| {
                let auth = login(stream, reader);
                let wire = request(&auth) + &receipt(Some(&auth));
                stream.write_all(wire.as_bytes()).unwrap();
                let (headers, body) = response(reader);
                assert!(headers.starts_with("HTTP/1.1 403 Forbidden\r\n"));
                assert!(headers.contains("Connection: close\r\n"));
                assert!(!headers.contains("WWW-Authenticate"));
                assert_eq!(body, [] as [u8; 0]);
            },
        );
    }
}

#[test]
fn bad_bearer_is_never_retried_as_basic_or_sticky_session_authority() {
    local_exchange(
        |_, _, _| panic!("unverified bearer must not dispatch"),
        |stream, reader| {
            let auth = login(stream, reader);
            let forged = format!("Bearer {}A", "A".repeat(42));
            let wire = receipt(Some(&forged)) + &receipt(Some(&auth));
            stream.write_all(wire.as_bytes()).unwrap();
            let (headers, body) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
            assert!(headers.contains("WWW-Authenticate: Bearer\r\n"));
            assert!(headers.contains("Connection: close\r\n"));
            assert!(!headers.contains("WWW-Authenticate: Basic"));
            assert_eq!(body, [] as [u8; 0]);
        },
    );
}

#[test]
fn legacy_basic_still_stops_at_exact_body_and_closes_before_a_following_request() {
    let calls = AtomicUsize::new(0);
    local_exchange(
        |request, inbound, _| {
            assert!(matches!(request, Request::SaveNode(_)));
            calls.fetch_add(1, Ordering::Relaxed);
            let Incoming::Blob(body) = inbound else {
                panic!("exact R3 blob was not framed");
            };
            let mut actual = Vec::new();
            body.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, b"body");
            Outcome::Json(b"{}".to_vec())
        },
        |stream, reader| {
            let one = put_body(FIXTURE_BASIC).replacen(
                "\r\n\r\n",
                "\r\nConnection: keep-alive\r\n\r\n",
                1,
            );
            let wire = one + &list(Some(FIXTURE_BASIC));
            stream.write_all(wire.as_bytes()).unwrap();
            let (headers, body) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(headers.contains("Connection: close\r\n"));
            assert_eq!(body, b"{}");
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn legacy_unconsumed_body_is_not_accepted_and_no_following_request_dispatches() {
    let calls = AtomicUsize::new(0);
    local_exchange(
        |request, _, _| {
            assert!(matches!(request, Request::SaveNode(_)));
            calls.fetch_add(1, Ordering::Relaxed);
            Outcome::Json(b"{}".to_vec())
        },
        |stream, reader| {
            let wire = put_body(FIXTURE_BASIC) + &list(Some(FIXTURE_BASIC));
            stream.write_all(wire.as_bytes()).unwrap();
            let (headers, body) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
            assert!(headers.contains("Connection: close\r\n"));
            assert_eq!(body, [] as [u8; 0]);
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn login_throttle_has_the_exact_bounded_empty_reply() {
    let mut bytes = Vec::new();
    write_problem(&mut bytes, Problem::LoginThrottled(17)).unwrap();
    assert_eq!(
        bytes,
        b"HTTP/1.1 429 Too Many Requests\r\nConnection: close\r\nCache-Control: no-store\r\nRetry-After: 17\r\nContent-Length: 0\r\n\r\n"
    );
}

fn seeded_browse_store(
    local: &crate::test_support::PrivateTemp,
    archive: &crate::test_support::PrivateTemp,
) -> crate::container_store::Store {
    use crate::{
        container_store::{Body, Change, DataRoot, Mutation, Store, Target},
        fs::HeldDir,
        hash::Sha256,
    };
    use std::{fs::DirBuilder, os::unix::fs::DirBuilderExt};
    let directory = |parent: &crate::test_support::PrivateTemp, name: &str| {
        let path = parent.0.join(name);
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        HeldDir::trusted_storage(&path).unwrap()
    };
    let store = Store::open_with_recycle(
        directory(local, "system"),
        vec![DataRoot {
            root: directory(local, "data"),
            token: [0xad; 32],
        }],
        directory(archive, "archive"),
        [7; 16],
        [0xcb; 32],
    )
    .unwrap();
    let raw = b"opaque".as_slice();
    let mut digest = Sha256::new();
    digest.update(raw).unwrap();
    store
        .save(
            Mutation {
                container: [0x11; 16],
                request: [0x22; 16],
            },
            Change::Save {
                target: Target::FirstMain {
                    name: "sealed.bin".into(),
                },
                node: [0x33; 16],
                body: Body {
                    length: raw.len() as u64,
                    sha256: digest.finish(),
                },
            },
            raw,
            || Ok(()),
        )
        .unwrap();
    store
        .change(
            Mutation {
                container: [0x11; 16],
                request: [0x44; 16],
            },
            Change::Tags {
                expected_sequence: 1,
                tags: vec!["keep".into()],
            },
            || Ok(()),
        )
        .unwrap();
    store
}

#[test]
fn four_real_metadata_projections_match_basic_and_reauthorize_every_reused_head() {
    use crate::test_support::PrivateTemp;
    use std::path::Path;
    let local = PrivateTemp::under(&std::env::temp_dir());
    let archive = PrivateTemp::under(Path::new("/dev/shm"));
    let store = seeded_browse_store(&local, &archive);
    let tags = format!("{CONTAINER}/tags");
    let continuation = format!("Zxs-History-Head: {NODE}\r\n");
    let reads = [
        (LIST, "Zxs-View: compact\r\n"),
        (CONTAINER, "Zxs-Detail: paged\r\n"),
        (SAVE, continuation.as_str()),
        (tags.as_str(), "Content-Length: 0\r\n"),
    ];
    let dispatch = |request, inbound: &mut Incoming<'_>, live: &dyn Fn() -> bool| {
        assert!(matches!(inbound, Incoming::Empty));
        workspace_protocol::http::dispatch_request(&store, request, inbound, live)
    };
    let mut originals = Vec::new();
    for (path, fields) in reads {
        local_exchange(dispatch, |stream, reader| {
            stream
                .write_all(get(path, Some(FIXTURE_BASIC), fields).as_bytes())
                .unwrap();
            let (headers, body) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(headers.contains("Cache-Control: no-store\r\n"));
            assert!(headers.contains("Content-Type: application/json\r\n"));
            assert!(headers.contains("Connection: close\r\n"));
            originals.push(body);
            closed(reader);
        });
    }
    assert_eq!(originals[3], br#"["keep"]"#);
    let count = AtomicUsize::new(0);
    local_exchange(
        |request, incoming, live| {
            count.fetch_add(1, Ordering::Relaxed);
            dispatch(request, incoming, live)
        },
        |stream, reader| {
            let bearer = login(stream, reader);
            let mut wire = String::new();
            for (path, fields) in reads {
                wire.push_str(&get(path, Some(&bearer), fields));
            }
            wire.push_str(&get(&tags, Some("Bearer forged"), ""));
            wire.push_str(&get(&tags, Some(&bearer), ""));
            stream.write_all(wire.as_bytes()).unwrap();
            for wanted in originals {
                let (headers, actual) = response(reader);
                assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
                assert!(headers.contains("Cache-Control: no-store\r\n"));
                assert!(headers.contains("Content-Type: application/json\r\n"));
                assert!(headers.contains("Connection: keep-alive\r\n"));
                assert_eq!(actual, wanted);
            }
            let (headers, actual) = response(reader);
            assert!(headers.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
            assert!(headers.contains("WWW-Authenticate: Bearer\r\n"));
            assert!(headers.contains("Connection: close\r\n"));
            assert_eq!(actual, []);
        },
    );
    assert_eq!(count.load(Ordering::Relaxed), 4);
}
