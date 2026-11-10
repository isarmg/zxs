use super::*;
use json_codec::{Value, json};

fn page(repository: Option<(&str, &str)>) -> PageParts {
    let context = repository.map_or(Value::Null, |(id, path)| {
        json!({
            "repository_id": id,
            "repository_path": path,
            "branch_id": "main",
            "base_head": "head",
        })
    });
    Page::parse(
        json!({
            "slot_id": "P01", "path": "", "writable": true,
            "entries": [], "selected_entry": null, "total_entries": "0",
            "snapshot": "snapshot", "next_cursor": null, "previous_cursor": null,
            "context": context, "directory_file_count": "0", "directory_branches": [],
            "directory_branches_supported": false, "default_branch_scope": false,
        }),
        "P01",
        &DirectoryKey::navigation(""),
        1,
        None,
    )
    .unwrap()
    .into_parts()
}

#[test]
fn a_named_repository_blocks_its_root_display_name_with_or_without_a_disk_root() {
    for parent in [page(None), page(Some(("disk-root", "")))] {
        let target = page(Some(("named", "")));
        assert_eq!(
            check_root_target(parent.context.as_ref(), Ok(target))
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::NameCollision,
        );
    }
}

#[test]
fn an_ordinary_child_stays_under_the_original_move_admission() {
    let parent = page(Some(("disk-root", "")));
    assert!(
        check_root_target(
            parent.context.as_ref(),
            Ok(page(Some(("disk-root", "folder")))),
        )
        .is_ok()
    );
}

#[test]
fn missing_destinations_preserve_empty_disk_and_new_root_name_moves() {
    for parent in [page(None), page(Some(("disk-root", "")))] {
        let missing = api::error(storage_contract::ErrorCode::DirectoryNotFound, "missing");
        assert!(check_root_target(parent.context.as_ref(), Err(missing)).is_ok());
    }
}

#[test]
fn failed_root_reads_keep_their_original_error_and_correlation() {
    for code in [
        storage_contract::ErrorCode::TransportError,
        storage_contract::ErrorCode::InvalidDirectoryPage,
    ] {
        let mut error = api::error(code, "root read failed");
        error.status = 503;
        error.request_id = Some("request-42".into());
        let actual = check_root_target(None, Err(error)).unwrap_err();
        assert_eq!(actual.code, code);
        assert_eq!(actual.status, 503);
        assert_eq!(actual.message, "root read failed");
        assert_eq!(actual.request_id.as_deref(), Some("request-42"));
    }
}
