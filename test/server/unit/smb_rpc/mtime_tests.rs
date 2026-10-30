use super::*;
use smb_protocol::protocol::{
    AuthorizedMetadata, DirectoryPage, FileMtime, SavedMtime, StatResult,
};

fn stat(active: &mut Session, path: &str) -> StatResult {
    let (value, descriptor) = active
        .dispatch(Request::Stat { path: path.into() })
        .unwrap();
    assert!(descriptor.is_none());
    json_codec::from_value(value).unwrap()
}

fn handle_time(active: &mut Session, handle: u64) -> FileMtime {
    let (value, descriptor) = active.dispatch(Request::AuthorizeRead { handle }).unwrap();
    assert!(descriptor.is_none());
    json_codec::from_value::<AuthorizedMetadata>(value)
        .unwrap()
        .mtime
}

fn directory(active: &mut Session) -> OpenResult {
    let (value, _) = active
        .open(String::new(), false, false, false, false, false)
        .unwrap();
    json_codec::from_value(value).unwrap()
}

fn page(active: &mut Session, handle: u64, restart: bool) -> DirectoryPage {
    let (value, descriptor) = active
        .dispatch(Request::ReadDirectory { handle, restart })
        .unwrap();
    assert!(descriptor.is_none());
    json_codec::from_value(value).unwrap()
}

#[test]
fn mtime_tracks_exact_versions_across_same_size_saves_old_handles_and_frozen_pages() {
    let (temp, shared, backend, branch) = fixture();
    let mut active = session(shared.clone(), &branch);
    let (old, _fd) = open_file(&mut active, false);
    let original = active
        .version_mtime(old.version_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(old.mtime, original);
    assert_eq!(stat(&mut active, "report.txt").mtime, original);
    let frozen = directory(&mut active);
    let (writer, file) = open_file(&mut active, true);
    assert_eq!(writer.mtime, original);
    file.write_all_at(b"new", 0).unwrap();
    active.dirty(writer.handle).unwrap();
    assert_eq!(handle_time(&mut active, writer.handle), original);
    active.flush(writer.handle).unwrap();
    let (new, _fd) = open_file(&mut active, false);
    assert_eq!(new.size, old.size);
    assert_ne!(new.version_id, old.version_id);
    let saved = active
        .version_mtime(new.version_id.as_deref().unwrap())
        .unwrap();
    // Stored times have whole-second precision. Distinct versions may have equal times.
    assert_eq!(new.mtime, saved);
    assert_eq!(stat(&mut active, "report.txt").mtime, saved);
    assert_eq!(handle_time(&mut active, writer.handle), saved);
    assert_eq!(handle_time(&mut active, old.handle), original);
    assert_eq!(read_session(&mut active, old.handle, 0, 3), b"old");
    let historical = page(&mut active, frozen.handle, false);
    assert_eq!(historical.entries.len(), 1);
    assert_eq!(
        historical.entries[0].entry.version_id,
        old.version_id.unwrap()
    );
    assert_eq!(historical.entries[0].mtime, original);
    let current = page(&mut active, frozen.handle, true);
    assert_eq!(current.entries[0].entry.version_id, new.version_id.unwrap());
    assert_eq!(current.entries[0].mtime, saved);
    file.write_all_at(b"end", 0).unwrap();
    active.dirty(writer.handle).unwrap();
    assert_eq!(handle_time(&mut active, writer.handle), saved);
    active.close(writer.handle, true).unwrap();
    for handle in [old.handle, new.handle, frozen.handle] {
        active.close(handle, false).unwrap();
    }
    drop(active);
    drop(shared);
    drop(backend);
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    let entry = &engine.list_tree(&branch).unwrap()[0];
    let persisted = &engine.state().versions[&entry.version_id];
    assert_eq!(
        crate::backend_read::version_mtime(persisted).unwrap(),
        saved
    );
}

#[test]
fn mtime_new_work_is_unpublished_until_flush_and_truncate_keeps_the_saved_baseline() {
    let (_temp, shared, _, branch) = fixture();
    let mut active = session(shared, &branch);
    let original = stat(&mut active, "report.txt").mtime;
    let (value, file) = active
        .open("fresh.txt".into(), true, true, false, true, false)
        .unwrap();
    let fresh: OpenResult = json_codec::from_value(value).unwrap();
    assert_eq!(fresh.mtime, FileMtime::Unpublished {});
    assert_eq!(
        handle_time(&mut active, fresh.handle),
        FileMtime::Unpublished {}
    );
    File::from(file.unwrap()).write_all_at(b"first", 0).unwrap();
    active.dirty(fresh.handle).unwrap();
    assert_eq!(
        handle_time(&mut active, fresh.handle),
        FileMtime::Unpublished {}
    );
    active.flush(fresh.handle).unwrap();
    assert_eq!(
        handle_time(&mut active, fresh.handle),
        stat(&mut active, "fresh.txt").mtime
    );
    assert!(matches!(
        handle_time(&mut active, fresh.handle),
        FileMtime::Saved { .. }
    ));
    active.close(fresh.handle, false).unwrap();
    let (value, _fd) = active
        .open("report.txt".into(), true, false, true, false, false)
        .unwrap();
    let truncated: OpenResult = json_codec::from_value(value).unwrap();
    assert_eq!(truncated.size, 0);
    assert_eq!(truncated.mtime, original);
    assert_eq!(handle_time(&mut active, truncated.handle), original);
    active.close(truncated.handle, true).unwrap();
}

#[test]
fn mtime_inherited_file_uses_its_visible_parent_version() {
    let (_temp, shared, _, branch) = fixture();
    let mut active = session(shared.clone(), &branch);
    let (parent, _fd) = open_file(&mut active, false);
    select_pending_file(&shared, &branch);
    let (inherited, _fd) = open_file(&mut active, false);
    assert_eq!(inherited.version_id, parent.version_id);
    assert_eq!(inherited.mtime, parent.mtime);
    assert_eq!(stat(&mut active, "report.txt").mtime, parent.mtime);
    let root = directory(&mut active);
    let listed = page(&mut active, root.handle, false);
    assert_eq!(
        listed.entries[0].entry.version_id,
        parent.version_id.unwrap()
    );
    assert_eq!(listed.entries[0].mtime, parent.mtime);
    let (writer, _fd) = open_file(&mut active, true);
    assert_eq!(handle_time(&mut active, writer.handle), parent.mtime);
    active.close(writer.handle, true).unwrap();
    for handle in [parent.handle, inherited.handle, root.handle] {
        active.close(handle, false).unwrap();
    }
}

#[test]
fn mtime_failed_flush_keeps_the_confirmed_head_until_original_operation_is_reconciled() {
    let (_temp, shared, backend, branch) = fixture();
    let mut active = session(shared, &branch);
    let (opened, file) = open_file(&mut active, true);
    let previous_head = match &active.handles[&opened.handle] {
        Handle::Write(writer) => writer.last_head.clone(),
        _ => unreachable!(),
    };
    file.write_all_at(b"new", 0).unwrap();
    active.dirty(opened.handle).unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        active.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::InjectedFailure
    );
    assert_eq!(handle_time(&mut active, opened.handle), opened.mtime);
    let operation = match &active.handles[&opened.handle] {
        Handle::Write(writer) => {
            assert_eq!(writer.last_head, previous_head);
            assert!(!writer.committed);
            writer.operation_id
        }
        _ => unreachable!(),
    };
    active.flush(opened.handle).unwrap();
    let published = active
        .shared
        .backend
        .operation_outcome(&active.identity, operation)
        .unwrap();
    match &active.handles[&opened.handle] {
        Handle::Write(writer) => {
            assert_eq!(Some(&writer.last_head), published.commit_id.as_ref());
            assert!(writer.committed);
        }
        _ => unreachable!(),
    }
    assert_eq!(
        handle_time(&mut active, opened.handle),
        stat(&mut active, "report.txt").mtime
    );
    active.close(opened.handle, false).unwrap();
}

#[test]
fn mtime_write_through_dirty_uses_the_confirmed_publication_even_if_later_projection_fails() {
    let (_temp, shared, backend, branch) = fixture();
    let mut active = session(shared, &branch);
    let (value, descriptor) = active
        .open("report.txt".into(), true, false, false, false, true)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(value).unwrap();
    let file = File::from(descriptor.unwrap());
    file.write_all_at(b"new", 0).unwrap();
    active.dirty(opened.handle).unwrap();
    let first_head = match &active.handles[&opened.handle] {
        Handle::Write(writer) => {
            assert!(writer.committed);
            writer.last_head.clone()
        }
        _ => unreachable!(),
    };
    assert_eq!(
        handle_time(&mut active, opened.handle),
        stat(&mut active, "report.txt").mtime
    );
    file.write_all_at(b"end", 0).unwrap();
    backend
        .reconciliation_failure
        .store(RECONCILIATION_SNAPSHOT, Ordering::SeqCst);
    assert_eq!(
        active.dirty(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::MetadataUnavailable
    );
    let confirmed_head = match &active.handles[&opened.handle] {
        Handle::Write(writer) => {
            assert_ne!(writer.last_head, first_head);
            assert!(!writer.committed);
            let outcome = backend
                .operation_outcome(&active.identity, writer.operation_id)
                .unwrap();
            assert_eq!(Some(&writer.last_head), outcome.commit_id.as_ref());
            writer.last_head.clone()
        }
        _ => unreachable!(),
    };
    // Native completion preceded the injected metadata failure. Reporting this
    // published version time does not claim that arbitrary later FD bytes saved.
    assert_eq!(
        handle_time(&mut active, opened.handle),
        stat(&mut active, "report.txt").mtime
    );
    active.flush(opened.handle).unwrap();
    match &active.handles[&opened.handle] {
        Handle::Write(writer) => {
            assert_eq!(writer.last_head, confirmed_head);
            assert!(writer.committed);
        }
        _ => unreachable!(),
    }
    active.close(opened.handle, false).unwrap();
}

#[test]
fn mtime_metadata_never_reads_payload_bytes() {
    let (temp, shared, backend, branch) = fixture();
    for path in immutable_fixture_payloads(temp.path()) {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .write_all_at(b"bad", 0)
            .unwrap();
    }
    backend
        .reconciliation_failure
        .store(RECONCILIATION_CONTENT, Ordering::SeqCst);
    let mut active = session(shared, &branch);
    let expected = stat(&mut active, "report.txt").mtime;
    let root = directory(&mut active);
    assert_eq!(
        page(&mut active, root.handle, false).entries[0].mtime,
        expected
    );
    let (opened, _fd) = open_file(&mut active, false);
    assert_eq!(opened.mtime, expected);
    assert_eq!(handle_time(&mut active, opened.handle), expected);
    assert_eq!(
        backend.reconciliation_failure.load(Ordering::SeqCst),
        RECONCILIATION_CONTENT
    );
    assert_eq!(
        active.read(opened.handle, 0, 3).unwrap_err().code,
        storage_contract::ErrorCode::ContentUnavailable
    );
    active.close(opened.handle, true).unwrap();
    active.close(root.handle, true).unwrap();
}

#[test]
fn mtime_parsing_rejects_noncanonical_or_out_of_range_saved_timestamps() {
    let (_temp, shared, _, branch) = fixture();
    let mut active = session(shared, &branch);
    let (opened, _fd) = open_file(&mut active, false);
    let mut version = active
        .shared
        .backend
        .version_snapshot(&active.identity, opened.version_id.as_deref().unwrap())
        .unwrap()
        .version;
    for (utc, seconds) in [
        ("1970-01-01T00:00:00Z", 0),
        ("2038-01-19T03:14:08Z", 2_147_483_648),
        ("9999-12-31T23:59:59Z", SavedMtime::MAX_UNIX_SECONDS),
    ] {
        version.created_at = utc.into();
        assert_eq!(
            crate::backend_read::version_mtime(&version).unwrap(),
            FileMtime::Saved {
                unix_seconds: SavedMtime::from_unix_seconds(seconds).unwrap()
            }
        );
    }
    for invalid in [
        "",
        "now",
        "1969-12-31T23:59:59Z",
        "2026-02-30T00:00:00Z",
        "2026-10-30T13:00:00+00:00",
        "2026-10-30T13:00:00.000Z",
        "2026-10-30t13:00:00z",
        "2016-12-31T23:59:60Z",
        "10000-01-01T00:00:00Z",
    ] {
        version.created_at = invalid.into();
        let error = crate::backend_read::version_mtime(&version).unwrap_err();
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::MetadataUnavailable,
            "{invalid}"
        );
        assert_eq!(error.status, ::http::StatusCode::INTERNAL_SERVER_ERROR);
    }
    active.close(opened.handle, true).unwrap();
}
