use super::*;
use crate::{
    headless_auth::{Account, FIXTURE_BASIC, fixture_account},
    headless_http::{
        Incoming, Outcome,
        upload::{Ingress, fixture_exchange},
    },
    test_support::PrivateTemp,
    workspace_protocol,
};
use std::{
    cell::Cell,
    io::{BufRead, Cursor},
    time::{Duration, Instant},
};

const C: &str = "11111111111111111111111111111111";
const O: &str = "22222222222222222222222222222222";
const N: &str = "33333333333333333333333333333333";
const PATH: &str = "/zxs/v1/uploads";
const RAW: &[u8] = b"opaque12byte";
const OP: Mutation = Mutation {
    container: [0x11; 16],
    request: [0x22; 16],
};

struct Fixture {
    _temp: PrivateTemp,
    system: HeldDir,
    data: PathBuf,
    account: Account,
    bearer: String,
}

impl Fixture {
    fn new() -> Self {
        let temporary = PrivateTemp::under(&std::env::temp_dir());
        let system = temporary.0.join("system");
        let data = temporary.0.join("data");
        ensure_dir(&system).unwrap();
        ensure_dir(&data).unwrap();
        let account = fixture_account();
        let token = account
            .issue_session(&[("authorization", FIXTURE_BASIC)], &|| false)
            .unwrap();
        Self {
            system: HeldDir::trusted_storage(&system).unwrap(),
            data,
            _temp: temporary,
            account,
            bearer: format!("Bearer {token}"),
        }
    }

    fn store(&self) -> Store {
        let exact_system_owner = self.system.opaque_owner(c"container-owner").unwrap();
        let role = DataRoot {
            token: [0x39; 32],
            root: HeldDir::trusted_storage(&self.data).unwrap(),
        };
        Store::open_roster(&self.system, vec![role], exact_system_owner).unwrap()
    }

    fn file(&self, upload: &str) -> PathBuf {
        self.data
            .join("staging")
            .join(format!("u-{upload}"))
            .join("payload")
    }

    fn request(
        &self,
        store: &Store,
        method: &str,
        path: &str,
        headers: &str,
        body: &[u8],
    ) -> String {
        let (bytes, head) = wire(method, path, Some(&self.bearer), headers, body);
        let mut input = Cursor::new(bytes);
        let output = self.inspect(store, &mut input, &Instant::now);
        assert_eq!(input.position(), (head + body.len()) as u64);
        output
    }

    fn inspect(
        &self,
        store: &Store,
        input: &mut impl BufRead,
        now: &dyn Fn() -> Instant,
    ) -> String {
        let mut response = Vec::new();
        fixture_exchange(
            (store, &self.system),
            &self.account,
            input,
            &mut response,
            Ingress {
                now,
                wait: &mut |bound| {
                    assert!(bound > Duration::ZERO && bound <= Duration::from_millis(250));
                    Ok(())
                },
                stopping: &|| false,
            },
        )
        .unwrap();
        String::from_utf8(response).unwrap()
    }
}

fn wire(
    method: &str,
    path: &str,
    authorization: Option<&str>,
    headers: &str,
    body: &[u8],
) -> (Vec<u8>, usize) {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: fixture.zxs\r\nZxs-Op-Id: {O}\r\n");
    if let Some(authorization) = authorization {
        head.push_str("Authorization: ");
        head.push_str(authorization);
        head.push_str("\r\n");
    }
    head.push_str(headers);
    head.push_str("\r\n");
    let width = head.len();
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    (bytes, width)
}

fn digest(body: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(body).unwrap();
    hex(hash.finish())
}

fn begin_bytes() -> String {
    format!(
        r#"{{"container_id":"{C}","node_id":"{N}","target":{{"kind":"first_main","name":"raw.bin"}},"length":{},"sha256":"{}"}}"#,
        RAW.len(),
        digest(RAW)
    )
}

fn json_headers(body: &str) -> String {
    format!(
        "Content-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    )
}

fn put_headers(data: &[u8]) -> String {
    format!(
        "Zxs-Container-Id: {C}\r\nZxs-Chunk-Sha256: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n",
        digest(data),
        data.len()
    )
}

fn success(body: &str) -> &str {
    assert!(body.starts_with("HTTP/1.1 200 OK\r\n"), "{body}");
    assert!(body.contains("\r\nCache-Control: no-store\r\n"));
    assert!(body.contains("\r\nContent-Type: application/json\r\n"));
    let (head, body) = body.split_once("\r\n\r\n").unwrap();
    assert!(
        head.lines()
            .any(|line| line == format!("Content-Length: {}", body.len()))
    );
    body
}

fn progress_body(id: &str, offset: usize, state: &str) -> String {
    format!(
        r#"{{"upload_id":"{id}","container_id":"{C}","operation_id":"{O}","node_id":"{N}","target":{{"kind":"first_main","name":"raw.bin"}},"length":{},"sha256":"{}","next_offset":{offset},"state":"{state}"}}"#,
        RAW.len(),
        digest(RAW)
    )
}

fn upload(f: &Fixture, store: &Store) -> String {
    let json = begin_bytes();
    let response = f.request(store, "POST", PATH, &json_headers(&json), json.as_bytes());
    let progress = success(&response);
    let id = progress
        .strip_prefix("{\"upload_id\":\"")
        .unwrap()
        .split_once('"')
        .unwrap()
        .0
        .to_owned();
    assert_eq!(id.len(), 64);
    assert_eq!(progress, progress_body(&id, 0, "open"));
    id
}

fn discovery(f: &Fixture, store: &Store) -> String {
    f.request(
        store,
        "GET",
        PATH,
        &format!("Zxs-Container-Id: {C}\r\n"),
        &[],
    )
}

#[test]
fn upload_exchange_consumes_only_declared_frames_and_returns_the_existing_receipt_dto() {
    let f = Fixture::new();
    let store = f.store();
    let missing = discovery(&f, &store);
    assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(missing.ends_with("Content-Length: 0\r\n\r\n"));
    let id = upload(&f, &store);
    assert_eq!(
        success(&discovery(&f, &store)),
        progress_body(&id, 0, "open")
    );

    let path = format!("{PATH}/{id}?offset=0");
    let (mut bytes, header) = wire("PUT", &path, Some(&f.bearer), &put_headers(RAW), RAW);
    let next = b"unread subsequent HTTP exchange";
    bytes.extend_from_slice(next);
    let mut physical = Cursor::new(bytes);
    let got = f.inspect(&store, &mut physical, &Instant::now);
    assert_eq!(physical.position(), (header + RAW.len()) as u64);
    let complete = progress_body(&id, RAW.len(), "complete");
    assert_eq!(success(&got), complete);
    let exact = f.request(
        &store,
        "GET",
        &format!("{PATH}/{id}"),
        &format!("Zxs-Container-Id: {C}\r\n"),
        &[],
    );
    assert_eq!(success(&exact), complete);
    assert_eq!(fs::read(f.file(&id)).unwrap(), RAW);
    assert!(store.operation_receipt(OP).unwrap().is_none());

    let mut changed = RAW.to_vec();
    changed[0] ^= 1;
    let bad_replay = f.request(&store, "PUT", &path, &put_headers(&changed), &changed);
    assert!(bad_replay.starts_with("HTTP/1.1 409 Conflict\r\n"));
    assert_eq!(fs::read(f.file(&id)).unwrap(), RAW);

    let commit = format!(r#"{{"upload_id":"{id}"}}"#);
    let chosen = f.request(
        &store,
        "POST",
        &format!("/zxs/v1/containers/{C}/commits"),
        &json_headers(&commit),
        commit.as_bytes(),
    );
    let body = success(&chosen);
    let query = workspace_protocol::request(
        "GET",
        &format!("/zxs/v1/containers/{C}/operations/{O}"),
        &[],
    )
    .ok()
    .unwrap();
    let prior =
        workspace_protocol::http::dispatch_request(&store, query, &mut Incoming::Empty, &|| true);
    let Outcome::Json(receipt) = prior else {
        panic!("missing original selected receipt")
    };
    assert_eq!(body.as_bytes(), receipt);
    assert!(!body.contains("upload_id"));
    assert!(!f.file(&id).exists());
    let unavailable_stage = discovery(&f, &store);
    assert!(unavailable_stage.starts_with("HTTP/1.1 409 Conflict\r\n"));
}

#[test]
fn basic_missing_or_expired_credentials_cannot_consume_or_create_an_upload() {
    let f = Fixture::new();
    let store = f.store();
    let json = begin_bytes();
    let headers = json_headers(&json);
    let current = Instant::now;
    let expired = || Instant::now() + Duration::from_secs(901);
    let cases: [(Option<&str>, &dyn Fn() -> Instant); 3] = [
        (Some(FIXTURE_BASIC), &current),
        (None, &current),
        (Some(&f.bearer), &expired),
    ];
    for (authorization, clock) in cases {
        let (bytes, header) = wire("POST", PATH, authorization, &headers, json.as_bytes());
        let mut physical = Cursor::new(bytes);
        let denied = f.inspect(&store, &mut physical, clock);
        assert!(denied.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(denied.contains("\r\nWWW-Authenticate: Bearer\r\n"));
        assert!(denied.ends_with("Content-Length: 0\r\n\r\n"));
        assert_eq!(physical.position(), header as u64);
        assert!(!f.system.path.join("upload-intents").exists());
        assert_eq!(store.routes.get(OP.container).unwrap(), None);
        assert!(store.operation_receipt(OP).unwrap().is_none());
    }
}

#[test]
fn exact_frame_eof_cannot_create_an_intent_or_acknowledge_a_partial_block() {
    let f = Fixture::new();
    let store = f.store();
    let json = begin_bytes();
    let start = f.request(
        &store,
        "POST",
        PATH,
        &json_headers(&json),
        &json.as_bytes()[..json.len() - 1],
    );
    assert!(start.starts_with("HTTP/1.1 422 Unprocessable Content\r\n"));
    assert!(!f.system.path.join("upload-intents").exists());

    let id = upload(&f, &store);
    let partial = f.request(
        &store,
        "PUT",
        &format!("{PATH}/{id}?offset=0"),
        &put_headers(RAW),
        &RAW[..RAW.len() - 1],
    );
    assert!(partial.starts_with("HTTP/1.1 422 Unprocessable Content\r\n"));
    assert_eq!(fs::read(f.file(&id)).unwrap(), []);
    assert_eq!(
        success(&discovery(&f, &store)),
        progress_body(&id, 0, "open")
    );
    assert_eq!(store.routes.get(OP.container).unwrap(), None);
    assert!(store.operation_receipt(OP).unwrap().is_none());
}

struct Pace<'a> {
    input: Cursor<Vec<u8>>,
    clock: &'a Cell<Instant>,
    step: Duration,
}
impl Read for Pace<'_> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let take = target.len().min(3);
        self.clock.set(self.clock.get() + self.step);
        self.input.read(&mut target[..take])
    }
}
impl BufRead for Pace<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.input.fill_buf()
    }
    fn consume(&mut self, count: usize) {
        self.input.consume(count);
    }
}

#[test]
fn raw_ingress_deadline_is_absolute_and_applies_only_to_the_new_body() {
    let f = Fixture::new();
    let store = f.store();
    let id = upload(&f, &store);
    let path = format!("{PATH}/{id}?offset=0");
    for (seconds, want_success) in [(65, false), (35, true)] {
        let clock = Cell::new(Instant::now());
        let (mut bytes, header) = wire("PUT", &path, Some(&f.bearer), &put_headers(RAW), RAW);
        let following = b"unread following request";
        bytes.extend_from_slice(following);
        let mut paced = Pace {
            input: Cursor::new(bytes),
            clock: &clock,
            step: Duration::from_secs(seconds),
        };
        let outcome = f.inspect(&store, &mut paced, &|| clock.get());
        if want_success {
            assert_eq!(success(&outcome), progress_body(&id, RAW.len(), "complete"));
            assert_eq!(paced.input.position(), (header + RAW.len()) as u64);
            assert_eq!(fs::read(f.file(&id)).unwrap(), RAW);
        } else {
            assert!(outcome.starts_with("HTTP/1.1 422 Unprocessable Content\r\n"));
            assert_eq!(paced.input.position(), (header + 9) as u64);
            assert_eq!(fs::read(f.file(&id)).unwrap(), []);
            assert_eq!(
                success(&discovery(&f, &store)),
                progress_body(&id, 0, "open")
            );
        }
        assert_eq!(&paced.input.get_ref()[header + RAW.len()..], following);
        assert_eq!(store.routes.get(OP.container).unwrap(), None);
        assert!(store.operation_receipt(OP).unwrap().is_none());
    }
}
