use super::*;
use crate::protocol::{AuthorizedMetadata, DirectoryPage, StatResult};
use std::{fs::File, os::fd::AsFd};

#[derive(Clone, Copy, Debug)]
enum MetadataRequest {
    Stat,
    Handle,
    Directory,
}
impl MetadataRequest {
    fn call(self, client: &Client) -> io::Result<Value> {
        // Only the test oracle converts typed results back to JSON.
        match self {
            Self::Stat => client
                .stat("file".into())
                .map(|value| json_codec::to_value(value).unwrap()),
            Self::Handle => client
                .authorize_read(7)
                .map(|value| json_codec::to_value(value).unwrap()),
            Self::Directory => client
                .read_directory(7, true)
                .map(|value| json_codec::to_value(value).unwrap()),
        }
    }
    fn result(self) -> Value {
        match self {
            Self::Stat => {
                json_codec::json!({"entry_id":"entry", "kind":"file", "path":"file", "name":"file", "size":"3", "mtime":{"state":"unpublished"}})
            }
            Self::Handle => json_codec::json!({"mtime":{"state":"unpublished"}}),
            Self::Directory => json_codec::json!({"entries":[], "more":false}),
        }
    }
    fn require_request(self, request: Request) {
        match (self, request) {
            (Self::Stat, Request::Stat { path }) => assert_eq!(path, "file"),
            (Self::Handle, Request::AuthorizeRead { handle: 7 }) => {}
            (
                Self::Directory,
                Request::ReadDirectory {
                    handle: 7,
                    restart: true,
                },
            ) => {}
            other => panic!("incorrect typed request: {other:?}"),
        }
    }
}

#[test]
fn typed_metadata_keeps_business_errors_and_reuses_the_connection() {
    for kind in [
        MetadataRequest::Stat,
        MetadataRequest::Handle,
        MetadataRequest::Directory,
    ] {
        let (one, two) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = FrameStream::new(two).unwrap();
            for failed in [true, false] {
                let (request, fd) = stream.receive::<Request>().unwrap();
                assert!(fd.is_none());
                kind.require_request(request);
                let response = if failed {
                    Response::Error {
                        code: ErrorCode::Forbidden,
                        message: "revoked metadata".into(),
                        retryable: false,
                        operation_id: Some("original".into()),
                    }
                } else {
                    Response::Ok {
                        result: kind.result(),
                    }
                };
                stream.send(&response, None).unwrap();
            }
        });
        let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
        let clone = client.clone();
        let error = kind.call(&client).unwrap_err();
        let remote = error
            .get_ref()
            .unwrap()
            .downcast_ref::<RemoteError>()
            .unwrap();
        assert_eq!(remote.code, RemoteErrorCode::Server(ErrorCode::Forbidden));
        assert_eq!(remote.operation_id.as_deref(), Some("original"));
        assert!(!remote.retryable);
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(kind.call(&clone).unwrap(), kind.result());
        server.join().unwrap();
    }
}

#[test]
fn malformed_metadata_or_extra_descriptor_closes_every_clone_before_reuse() {
    for kind in [
        MetadataRequest::Stat,
        MetadataRequest::Handle,
        MetadataRequest::Directory,
    ] {
        for extra_fd in [false, true] {
            let (one, two) = UnixStream::pair().unwrap();
            let server = std::thread::spawn(move || {
                let mut stream = FrameStream::new(two).unwrap();
                let (request, _) = stream.receive::<Request>().unwrap();
                kind.require_request(request);
                let file = File::open("/dev/null").unwrap();
                stream
                    .send(
                        &Response::Ok {
                            result: if extra_fd {
                                kind.result()
                            } else {
                                json_codec::json!({"unexpected":true})
                            },
                        },
                        extra_fd.then(|| file.as_fd()),
                    )
                    .unwrap();
                assert_eq!(
                    stream.receive::<Request>().unwrap_err().kind(),
                    io::ErrorKind::UnexpectedEof
                );
            });
            let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
            let clone = client.clone();
            assert_eq!(
                kind.call(&client).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(
                kind.call(&clone).unwrap_err().kind(),
                io::ErrorKind::NotConnected
            );
            server.join().unwrap();
        }
    }
}

#[test]
fn nonprogressing_directory_page_is_rejected_at_locked_admission() {
    let (one, two) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = FrameStream::new(two).unwrap();
        MetadataRequest::Directory.require_request(stream.receive::<Request>().unwrap().0);
        stream
            .send(
                &Response::Ok {
                    result: json_codec::json!({"entries":[], "more":true}),
                },
                None,
            )
            .unwrap();
        assert_eq!(
            stream.receive::<Request>().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    });
    let client = Client(Arc::new(Mutex::new(Some(FrameStream::new(one).unwrap()))));
    let clone = client.clone();
    assert_eq!(
        client.read_directory(7, true).unwrap_err().raw_os_error(),
        Some(libc::EINVAL)
    );
    assert_eq!(
        clone.read_directory(7, true).unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
    server.join().unwrap();
}

fn decoded<R: data_serialization::de::DeserializeOwned + data_serialization::Serialize>(
    response: Response,
    raw: Option<&json_codec::value::RawValue>,
) -> io::Result<Value> {
    match response {
        Response::Ok { result } => super::super::response::decode_result::<R>(result, raw)
            .map(|result| json_codec::to_value(result).unwrap())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        error => Ok(json_codec::to_value(error).unwrap()),
    }
}
fn compare<R: data_serialization::de::DeserializeOwned + data_serialization::Serialize>(
    raw: &[u8],
) {
    let old = storage_contract::decode_strict::<Response>(raw)
        .map_err(io::Error::other)
        .and_then(|response| decoded::<R>(response, None))
        .map_err(super::super::response::tests::signature);
    let new = super::super::response::decode(raw)
        .map_err(io::Error::other)
        .and_then(|(response, raw)| decoded::<R>(response, raw))
        .map_err(super::super::response::tests::signature);
    assert_eq!(new, old, "payload={:?}", String::from_utf8_lossy(raw));
}

#[test]
fn shared_decoder_preserves_metadata_admission_and_diagnostic_sources() {
    let mut results = vec![
        "null".to_owned(),
        "[]".into(),
        "false".into(),
        "{}".into(),
        r#"{"mtime":{"state":"future"},"unknown":0}"#.into(),
        r#"{"mtime":{"state":"saved","unix_seconds":"253402300800"}}"#.into(),
        r#"{"entries":[],"more":true,"more":false}"#.into(),
    ];
    for kind in [
        MetadataRequest::Stat,
        MetadataRequest::Handle,
        MetadataRequest::Directory,
    ] {
        results.push(kind.result().to_string());
        let mut extra = kind.result();
        extra["unknown"] = Value::Bool(true);
        results.push(extra.to_string());
    }
    results.push(json_codec::json!({"entry_id":"entry","kind":"file","path":"file","handle":"7","size":"3","max_file_bytes":"100","version_id":null,"workspace_id":"workspace","mtime":{"state":"unpublished"}}).to_string());
    for result in results {
        for payload in [
            format!(r#"{{"status":"ok","result":{result}}}"#),
            format!(r#"{{"result":{result},"status":"ok"}}"#),
        ] {
            compare::<OpenResult>(payload.as_bytes());
            compare::<StatResult>(payload.as_bytes());
            compare::<AuthorizedMetadata>(payload.as_bytes());
            compare::<DirectoryPage>(payload.as_bytes());
        }
    }
}
