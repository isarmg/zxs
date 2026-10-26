//! Published SMB saves remain retryable while their byte comparison is incomplete.
use super::*;

#[test]
fn failed_publication_reconciliation_retries_without_restaging_or_second_version() {
    for failure in [RECONCILIATION_SNAPSHOT, RECONCILIATION_CONTENT] {
        for lost_reply in [false, true] {
            let (_temp, shared, backend, branch) = fixture();
            let mut one = session(shared, &branch);
            let (opened, file) = open_file(&mut one, true);
            file.write_all_at(b"new", 0).unwrap();
            one.dirty(opened.handle).unwrap();
            let operation_id = match &one.handles[&opened.handle] {
                Handle::Write(writer) => writer.operation_id,
                _ => unreachable!(),
            };
            backend
                .reconciliation_failure
                .store(failure, Ordering::SeqCst);
            if lost_reply {
                backend.fail.store(true, Ordering::SeqCst);
                assert_eq!(
                    one.flush(opened.handle).unwrap_err().code,
                    storage_contract::ErrorCode::InjectedFailure
                );
            }
            assert_eq!(
                one.flush(opened.handle).unwrap_err().code,
                storage_contract::ErrorCode::MetadataUnavailable
            );
            let published = backend
                .call(
                    crate::BackendAction::Operation,
                    &one.identity,
                    json!({"operation_id":operation_id}),
                )
                .unwrap();
            assert_eq!(published["commit_state"], "PUBLISHED");
            assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);

            let saved = one.flush(opened.handle).unwrap();
            assert_eq!(saved["commit_state"], "PUBLISHED");
            assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
            let entry = one
                .find(&name_policy::LookupPath::parse("report.txt").unwrap())
                .unwrap()
                .unwrap();
            let history = backend
                .call(
                    crate::BackendAction::History,
                    &one.identity,
                    json!({"file_id":entry.file_id}),
                )
                .unwrap();
            assert_eq!(history["versions"].as_array().unwrap().len(), 2);
            one.close(opened.handle, false).unwrap();
        }
    }
}

#[test]
fn failed_publication_reconciliation_preserves_later_bytes_for_authorized_rebase() {
    for failure in [RECONCILIATION_SNAPSHOT, RECONCILIATION_CONTENT] {
        let (_temp, shared, backend, branch) = fixture();
        let mut one = session(shared, &branch);
        let (opened, file) = open_file(&mut one, true);
        file.write_all_at(b"new", 0).unwrap();
        one.dirty(opened.handle).unwrap();
        backend
            .reconciliation_failure
            .store(failure, Ordering::SeqCst);
        assert_eq!(
            one.flush(opened.handle).unwrap_err().code,
            storage_contract::ErrorCode::MetadataUnavailable
        );
        file.write_all_at(b"end", 0).unwrap();
        assert_eq!(
            one.flush(opened.handle).unwrap_err().code,
            storage_contract::ErrorCode::WorkspaceRebaseRequired
        );
        let writer = match &one.handles[&opened.handle] {
            Handle::Write(writer) => writer,
            _ => unreachable!(),
        };
        assert!(writer.committed && writer.dirty);
        assert_eq!(fs::read(&writer.source).unwrap(), b"end");
        let record: Value =
            json_codec::from_slice(&fs::read(writer.source.with_extension("work.json")).unwrap())
                .unwrap();
        assert_eq!(record["published"], true);
        assert_eq!(record["dirty"], true);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
        let entry = one
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let content = backend.content(&one.identity, &entry.version_id).unwrap();
        let mut bytes = [0; 3];
        content.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"new");

        one.dirty(opened.handle).unwrap();
        one.flush(opened.handle).unwrap();
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 2);
        let entry = one
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let content = backend.content(&one.identity, &entry.version_id).unwrap();
        content.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"end");
        one.close(opened.handle, false).unwrap();
    }
}
