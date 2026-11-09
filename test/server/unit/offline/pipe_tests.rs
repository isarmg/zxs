use super::*;
use std::io::Cursor;
fn frame(body: &[u8]) -> Vec<u8> {
    let mut bytes =
        br#"{"format":"zxs-offline-maintenance-pipe-1","operation":"first_administrator_create"}"#
            .to_vec();
    bytes.push(b'\n');
    bytes.extend(body);
    bytes
}
#[test]
fn pipe_preserves_exact_bounded_body_for_its_one_typed_operation() {
    let bytes = br#"{"password":"secret-in-body-only"}"#;
    let (header, body) = read_request(&mut Cursor::new(frame(bytes))).unwrap();
    assert_eq!(header.operation, Operation::FirstAdministratorCreate);
    assert_eq!(body.expose(), bytes);
    assert!(!format!("{body:?}").contains("secret-in-body-only"));
    let error = dispatch(header.operation, body.expose()).unwrap_err();
    assert_eq!(error.kind, FailureKind::InvalidRequest);
    assert!(!error.message.contains("secret-in-body-only"));
}
#[test]
fn pipe_rejects_missing_delimiter_unknown_header_and_oversized_frames() {
    for bytes in [
        Vec::new(), b"{}".to_vec(), vec![b'x'; wire::MAX_HEADER_BYTES + 1],
        b"{\"format\":\"old\",\"operation\":\"first_administrator_create\"}\n{}".to_vec(),
        b"{\"format\":\"zxs-offline-maintenance-pipe-1\",\"operation\":\"first_administrator_create\",\"program\":\"/tmp/other\"}\n{}".to_vec(),
        frame(&vec![b'x'; wire::MAX_REQUEST_BYTES + 1]),
    ] {
        assert!(read_request(&mut Cursor::new(bytes)).is_err());
    }
}

#[test]
fn failed_request_read_preserves_io_cause_without_returning_partial_secret() {
    struct FailedRead;
    impl Read for FailedRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "maintenance input was interrupted",
            ))
        }
    }
    let input = Cursor::new(frame(b"partial-secret-value")).chain(FailedRead);
    let error = read_request(&mut std::io::BufReader::new(input))
        .err()
        .expect("invariant: failing input cannot produce a request");
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert!(!error.to_string().contains("partial-secret-value"));
}

#[test]
fn initial_storage_keeps_private_causes_out_of_the_pipe_error() {
    #[derive(Debug)]
    struct Pending(std::io::Error);
    impl std::fmt::Display for Pending {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("original initialization remains pending")
        }
    }
    impl std::error::Error for Pending {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let error = call::<(), bool>(Operation::FirstStorageCreate, b"null", |_| {
        Err(Box::new(Pending(std::io::Error::other(
            "do-not-expose-credential-material",
        ))))
    })
    .unwrap_err();
    assert_eq!(error.message, "original initialization remains pending");
    assert!(!error.message.contains("do-not-expose"));
}
