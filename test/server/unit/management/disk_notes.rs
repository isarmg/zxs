use super::*;
use std::fs;

const DISK: &str = "disk_00000000000000000000000000000001";
const OTHER: &str = "disk_00000000000000000000000000000002";

fn note_path(root: &Path) -> std::path::PathBuf {
    root.join(DIRECTORY).join(format!("{DISK}.json"))
}

fn replace_fixture(root: &Path, bytes: &[u8]) {
    let parent = PrivateDirectory::open_trusted(root).unwrap();
    let directory = parent
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    AtomicFile::replace(&directory, &file_name(DISK).as_relative(), bytes).unwrap();
}

#[test]
fn absent_note_is_read_only_and_successful_updates_survive_reopening() {
    let root = crate::tests::private_tempdir();
    assert_eq!(read(root.path(), DISK).unwrap(), "");
    assert!(!root.path().join(DIRECTORY).exists());
    let text = format!(
        "  {}\n第二行\t<script>plain text</script>\0",
        "long note".repeat(100)
    );
    assert_eq!(update(root.path(), DISK, "", text.clone()).unwrap(), text);
    assert_eq!(read(root.path(), DISK).unwrap(), text);
    let on_disk = fs::read(note_path(root.path())).unwrap();
    assert_eq!(
        storage_contract::strict_json(&on_disk).unwrap()["note"],
        text
    );
    assert_eq!(read(root.path(), OTHER).unwrap(), "");
}

#[test]
fn stale_updates_conflict_same_value_retries_succeed_and_empty_explicitly_clears() {
    let root = crate::tests::private_tempdir();
    update(root.path(), DISK, "", "first".into()).unwrap();
    assert_eq!(
        update(root.path(), DISK, "", "first".into()).unwrap(),
        "first"
    );
    let error = update(root.path(), DISK, "", "lost change".into()).unwrap_err();
    assert_eq!(error.status, http::StatusCode::CONFLICT);
    assert_eq!(error.code, storage_contract::ErrorCode::GenerationMismatch);
    assert_eq!(read(root.path(), DISK).unwrap(), "first");
    update(root.path(), DISK, "first", String::new()).unwrap();
    assert_eq!(read(root.path(), DISK).unwrap(), "");
    assert!(note_path(root.path()).is_file());
}

#[test]
fn invalid_persisted_notes_are_not_silently_empty_or_overwritten() {
    let root = crate::tests::private_tempdir();
    for text in [
        "truncated".to_owned(),
        "[]".to_owned(),
        format!(r#"{{"format":"disk-note","disk_id":"{DISK}","note":null}}"#),
        format!(r#"{{"format":"disk-note","disk_id":"{DISK}","note":"a","note":"b"}}"#),
        format!(r#"{{"format":"old-format","disk_id":"{DISK}","note":"retain"}}"#),
        format!(r#"{{"format":"disk-note","disk_id":"{OTHER}","note":"retain"}}"#),
        format!(r#"{{"format":"disk-note","disk_id":"{DISK}","note":"retain","slot":"P01"}}"#),
    ] {
        replace_fixture(root.path(), text.as_bytes());
        assert_eq!(
            read(root.path(), DISK).unwrap_err().status,
            http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert!(update(root.path(), DISK, "", "replacement".into()).is_err());
        assert_eq!(fs::read(note_path(root.path())).unwrap(), text.as_bytes());
    }
}

#[test]
fn note_file_links_are_rejected_without_modifying_the_target() {
    use std::os::unix::fs::symlink;
    let root = crate::tests::private_tempdir();
    update(root.path(), DISK, "", "retained".into()).unwrap();
    let path = note_path(root.path());
    let original = root.path().join("original.json");
    fs::rename(&path, &original).unwrap();
    let before = fs::read(&original).unwrap();
    for hard_link in [false, true] {
        if hard_link {
            fs::hard_link(&original, &path).unwrap();
        } else {
            symlink(&original, &path).unwrap();
        }
        assert!(read(root.path(), DISK).is_err());
        assert!(update(root.path(), DISK, "retained", "replacement".into()).is_err());
        assert_eq!(fs::read(&original).unwrap(), before);
        fs::remove_file(&path).unwrap();
    }
}

#[test]
fn note_directory_links_are_rejected_and_request_budget_also_bounds_restart_reads() {
    use std::os::unix::fs::symlink;
    let root = crate::tests::private_tempdir();
    let other = crate::tests::private_tempdir();
    symlink(other.path(), root.path().join(DIRECTORY)).unwrap();
    assert!(read(root.path(), DISK).is_err());
    assert!(update(root.path(), DISK, "", "note".into()).is_err());
    assert_eq!(fs::read_dir(other.path()).unwrap().count(), 0);
    fs::remove_file(root.path().join(DIRECTORY)).unwrap();
    assert_eq!(
        update(root.path(), DISK, "", "x".repeat(crate::JSON_BODY_LIMIT))
            .unwrap_err()
            .status,
        http::StatusCode::PAYLOAD_TOO_LARGE,
    );
    assert!(!root.path().join(DIRECTORY).exists());
    replace_fixture(root.path(), &vec![b'x'; crate::JSON_BODY_LIMIT + 1]);
    assert!(read(root.path(), DISK).is_err());
    assert!(update(root.path(), DISK, "", "note".into()).is_err());
}

#[test]
fn every_body_admitted_note_record_is_within_the_same_restart_budget() {
    let disk = DISK;
    let request = storage_contract::disk_details::DiskNoteRequest {
        expected_disk_id: disk.into(),
        expected_note: String::new(),
        note: "x".repeat(crate::JSON_BODY_LIMIT - 128),
    };
    let body = json_codec::to_vec(&request).unwrap();
    assert!(body.len() <= crate::JSON_BODY_LIMIT);
    let record = Record {
        format: "disk-note".into(),
        disk_id: disk.into(),
        note: request.note.clone(),
    };
    assert!(json_codec::to_vec(&record).unwrap().len() < body.len());
    let root = crate::tests::private_tempdir();
    update(root.path(), disk, "", request.note.clone()).unwrap();
    assert_eq!(read(root.path(), disk).unwrap(), request.note);
}
