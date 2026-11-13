//! Committed encoding lookup never grants or retains a Current admission.
use super::*;
use crate::engine::backup_preparation::snapshot;
use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo;
use std::{cell::Cell, fs, os::unix::fs::DirBuilderExt, path::Path};

fn namespaces(
    prepared: &PreparedBackupRegistry,
    plan: &Plan,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<Vec<Vec<u8>>> {
    encoding_memo::tests::unchanged_account(prepared.scope_encoding.as_ref(), || {
        namespaces_inner(prepared, plan, expected, stop)
    })
}

fn namespaces_inner(
    prepared: &PreparedBackupRegistry,
    plan: &Plan,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<Vec<Vec<u8>>> {
    let mut observations = Vec::new();
    for index in 0..prepared.targets.len() {
        observations.push(plan.with_namespaces(prepared, index, expected, stop, |admitted| {
            let mut branches = BTreeMap::new();
            for id in admitted.state().branches.keys() {
                let (head, entries) = admitted.branch(id)?;
                branches.insert(id.clone(), (head.to_owned(), entries.to_vec()));
            }
            assert!(admitted.state().directory_branches.is_empty());
            // Complete State includes all deletion and retirement facts, even
            // though this original fixture has no nonempty deletion history.
            Ok(canonical_json(&(
                admitted.publication_hash(), admitted.registry(), admitted.state(), branches,
            ))?)
        })?);
    }
    Ok(observations)
}

// This proves directory names and file bytes, not permissions or timestamps.
fn fixture_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if metadata.is_dir() {
            assert!(entries.insert(relative, None).is_none());
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        } else {
            assert!(metadata.is_file());
            assert!(entries.insert(relative, Some(fs::read(path).unwrap())).is_none());
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn error_chain(error: &Error) -> (ErrorCode, Vec<String>) {
    let mut chain = Vec::new();
    let mut next: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = next {
        chain.push(error.to_string());
        next = error.source();
    }
    (error.code(), chain)
}

fn compare_rejection(
    root: &Path,
    prepared: &mut PreparedBackupRegistry,
    action: impl Fn(&PreparedBackupRegistry) -> Result<()>,
) {
    let before = fixture_tree(root);
    let warm = encoding_memo::tests::unchanged_account(
        prepared.scope_encoding.as_ref(), || action(prepared),
    ).err().expect("warm read must reject");
    assert_eq!(fixture_tree(root), before);
    let memo = prepared.scope_encoding.take();
    let plain = encoding_memo::tests::unchanged_account(
        prepared.scope_encoding.as_ref(), || action(prepared),
    ).err().expect("plain read must reject");
    assert_eq!(fixture_tree(root), before);
    prepared.scope_encoding = memo;
    assert_eq!(error_chain(&warm), error_chain(&plain));
}

pub(in crate::engine::backup_preparation::native_control::databases::scoped::cutover::registration)
fn verify_committed_namespace_encodings(
    root: &Path,
    prepared: &mut PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<()> {
    let retained = prepared.scope_encoding.take();
    let before = fixture_tree(root);
    let plan = Plan::capture(prepared, decision, expected, stop)?;
    let baseline = namespaces(prepared, &plan, expected, stop)?;
    assert_eq!(baseline.len(), 2);
    assert_eq!(fixture_tree(root), before);
    prepared.scope_encoding = Some(encoding_memo::ScopeEncodingMemo::new());
    // This cold committed-only read must not change aggregate account usage.
    // Dedicated memo tests additionally compare complete entries and payloads.
    assert_eq!(namespaces(prepared, &plan, expected, stop)?, baseline);
    assert_eq!(fixture_tree(root), before);
    // Only the original full registry verifier may populate this request memo.
    prepared.verify(stop)?;
    assert_eq!(fixture_tree(root), before);
    assert_eq!(namespaces(prepared, &plan, expected, stop)?, baseline);
    assert_eq!(fixture_tree(root), before);

    for index in 0..prepared.targets.len() {
        let path = prepared.targets[index].root.path().join("native-control/objects/snapshot")
            .join(format!("{}.json", prepared.task.selection().jobs()[index].snapshot));
        let original = fs::read(&path).unwrap();
        for bad in [b"{}".as_slice(), br#"{"a":1,"a":1}"#.as_slice()] {
            fs::write(&path, bad).unwrap();
            compare_rejection(root, prepared, |prepared| {
                namespaces(prepared, &plan, expected, stop).map(|_| ())
            });
        }
        fs::write(&path, original).unwrap();
    }

    let stopped = AtomicBool::new(true);
    compare_rejection(root, prepared, |prepared| {
        namespaces(prepared, &plan, expected, &stopped).map(|_| ())
    });
    compare_rejection(root, prepared, |prepared| {
        let called = Cell::new(false);
        let result: Result<()> = plan.with_namespaces(prepared, 0, expected, stop, |_| {
            called.set(true);
            Err(Error::new(ErrorCode::InvalidRequest, "test callback rejected the view"))
        });
        assert!(called.get());
        assert_eq!(result.as_ref().err().unwrap().code(), ErrorCode::InvalidRequest);
        result
    });
    compare_rejection(root, prepared, |prepared| {
        let late_stop = AtomicBool::new(false);
        let called = Cell::new(false);
        let result = plan.with_namespaces(prepared, 0, expected, &late_stop, |_| {
            called.set(true);
            late_stop.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        });
        assert!(called.get());
        result
    });

    // A real admitted ORIGINAL snapshot is not the published Backup State.
    // Full replay still has to reject this mismatch at finish before callback.
    let target = &prepared.targets[0];
    let original = snapshot::read(&target.root, &prepared.task.selection().jobs()[0], stop)?;
    let graph = plan.verify(target, stop)?;
    let publication = first_publication::read_at(
        prepared, plan.candidate(), expected, target, stop,
    )?;
    compare_rejection(root, prepared, |prepared| {
        let called = Cell::new(false);
        let result = with_archive_namespaces(
            prepared,
            0,
            NamespaceInputs {
                candidate: plan.candidate(),
                registry: &graph,
                published: &original,
                publication: &publication,
            },
            stop,
            |_| {
                called.set(true);
                Ok(())
            },
        );
        assert!(!called.get());
        let error = result.as_ref().err().expect("finish must reject the original State");
        assert_eq!(error.code(), ErrorCode::PublicationInvalid);
        assert_eq!(error.message(),
            "saved first candidate does not preserve the exact prescribed complete State");
        result
    });

    // Replace one configured name while its original held directory stays alive.
    let named = prepared.targets[0].root.path().to_path_buf();
    let displaced = root.join("committed-encoding-held-target");
    assert!(!displaced.exists());
    fs::rename(&named, &displaced).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&named).unwrap();
    compare_rejection(root, prepared, |prepared| {
        namespaces(prepared, &plan, expected, stop).map(|_| ())
    });
    fs::remove_dir(&named).unwrap();
    fs::rename(&displaced, &named).unwrap();
    assert_eq!(namespaces(prepared, &plan, expected, stop)?, baseline);
    assert_eq!(fixture_tree(root), before);
    prepared.scope_encoding = retained;
    Ok(())
}
