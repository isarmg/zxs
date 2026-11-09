use super::*;
use std::io::Cursor;
fn reply(header: ReplyHeader, body: &[u8]) -> Vec<u8> {
    let mut bytes = json_codec::to_vec(&header).unwrap();
    bytes.push(b'\n');
    bytes.extend(body);
    bytes
}
#[test]
fn exact_json_response_is_relayed_without_domain_round_trip() {
    let bytes = b"{\"sequence\":\"9007199254740993\",\"result\":null}";
    assert_eq!(
        read_reply(
            Operation::DiskAdditionPreview,
            &mut Cursor::new(reply(
                ReplyHeader::Success {
                    body_bytes: bytes.len()
                },
                bytes
            ))
        )
        .unwrap(),
        bytes
    );
}
#[test]
fn incomplete_oversized_unknown_and_failed_with_body_replies_are_not_success() {
    for bytes in [
        Vec::new(),
        reply(ReplyHeader::Success { body_bytes: 4 }, b"{}"),
        reply(ReplyHeader::Success { body_bytes: 1 }, b"{}"),
        b"{\"status\":\"success\"}".to_vec(),
        vec![b'x'; wire::MAX_HEADER_BYTES + 1],
        b"{\"status\":\"success\",\"body_bytes\":2,\"unknown\":true}\n{}".to_vec(),
        reply(
            ReplyHeader::Success {
                body_bytes: 64 * 1024 + 1,
            },
            &vec![b'x'; 64 * 1024 + 1],
        ),
        reply(
            ReplyHeader::Failed {
                kind: FailureKind::Conflict,
                message: "retained original".into(),
            },
            b"{}",
        ),
    ] {
        assert!(matches!(
            read_reply(Operation::DiskAdditionPreview, &mut Cursor::new(bytes)),
            Err(Error::Transport { .. })
        ));
    }
}
#[test]
fn remote_rejection_keeps_its_kind_and_original_operation_diagnostic() {
    let error = read_reply(
        Operation::FirstStorageCreate,
        &mut Cursor::new(reply(
            ReplyHeader::Failed {
                kind: FailureKind::Conflict,
                message: "original request aabb remains pending".into(),
            },
            b"",
        )),
    )
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(
        error.code(Operation::FirstStorageCreate),
        "first_storage_initialization"
    );
    assert!(error.to_string().contains("aabb"));
}
#[test]
fn current_protocol_does_not_admit_unknown_commands_or_formats() {
    for value in [
        r#"{"format":"old","operation":"disk_addition_preview"}"#,
        r#"{"format":"zxs-offline-maintenance-pipe-1","operation":"run_shell"}"#,
        r#"{"format":"zxs-offline-maintenance-pipe-1","operation":"disk_addition_preview","program":"/tmp/other"}"#,
    ] {
        assert!(json_codec::from_str::<wire::RequestHeader>(value).is_err());
    }
}
