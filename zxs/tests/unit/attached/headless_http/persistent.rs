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
fn valid_bearer_cannot_authorize_legacy_save_delete_or_list_or_drain_their_body() {
    let blocked: [fn(&str) -> String; 3] = [put_body, delete, |auth| list(Some(auth))];
    for request in blocked {
        local_exchange(
            |_, _, _| panic!("only an exact authenticated receipt GET may dispatch as Bearer"),
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
