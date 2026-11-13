//! Real generation-control collection shares only committed canonical bytes.
use super::*;
use crate::engine::backup_preparation::BackupNativeObjects;
use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo;
use std::{fs, path::Path};

struct GenerationEncodingObservation {
    prefix: BackupNativeObjects,
    control: PreparedBackupControl,
    report: BackupGenerationControlReport,
}

fn observe_generation_encoding(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    generation: &BackupGenerationPreparationReport,
    index: usize,
    stop: &AtomicBool,
) -> Result<GenerationEncodingObservation> {
    let native = &prepared.task.installation.evidence.task.scoped.objects.native;
    let root = &prepared.targets[index].root;
    let job = &prepared.task.selection().jobs()[index];
    let prefix = match prepared.scope_encoding.as_ref() {
        Some(memo) => generation_objects::collect_with_committed_encodings(
            root, job, native, stop, memo,
        )?,
        None => generation_objects::collect(root, job, &native.archive_hash, stop)?,
    };
    let local = root.private_child(&root.path().join(DIRECTORY).join("archive"))?;
    let frozen = BackupFrozenControl::read_target(decision, &local, stop)?;
    let rebuilt = PreparedBackupControl::capture(
        &frozen,
        BackupNativeObjects {
            objects: prefix.objects.clone(),
            seals: prefix.seals.clone(),
        },
        stop,
    )?;
    // The actual production entry reconstructs again, then reads back all
    // installed control material and its completion. No result substitutes it.
    let (control, report) = read_authority_input(prepared, decision, generation, index, stop)?;
    assert_eq!(rebuilt.root_hash(), control.root_hash());
    assert_eq!(rebuilt.files().collect::<Vec<_>>(), control.files().collect::<Vec<_>>());
    Ok(GenerationEncodingObservation { prefix, control, report })
}

fn compare_generation_observations(
    left: &GenerationEncodingObservation,
    right: &GenerationEncodingObservation,
) -> Result<()> {
    assert_eq!(left.prefix.objects, right.prefix.objects);
    assert_eq!(canonical_json(&left.prefix.seals)?, canonical_json(&right.prefix.seals)?);
    assert_eq!(left.control.root_hash(), right.control.root_hash());
    assert_eq!(left.control.files().collect::<Vec<_>>(), right.control.files().collect::<Vec<_>>());
    let left = &left.report;
    let right = &right.report;
    assert_eq!(left.operation_id, right.operation_id);
    assert_eq!(left.decision_hash, right.decision_hash);
    assert_eq!(left.native_control_root, right.native_control_root);
    assert_eq!(left.targets, right.targets);
    assert_eq!(left.inputs_hash, right.inputs_hash);
    assert_eq!(left.original_members, right.original_members);
    assert_eq!(left.original_journal_frames, right.original_journal_frames);
    assert_eq!(left.original_journal_bytes_per_target, right.original_journal_bytes_per_target);
    Ok(())
}

// Names and file contents only, not a metadata, permissions or mtime assertion.
fn generation_fixture_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
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

fn generation_error(error: &Error) -> (ErrorCode, Vec<String>) {
    let mut chain = Vec::new();
    let mut next: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = next {
        chain.push(error.to_string());
        next = error.source();
    }
    (error.code(), chain)
}

fn compare_generation_rejection(
    root: &Path,
    prepared: &mut PreparedBackupRegistry,
    action: impl Fn(&PreparedBackupRegistry) -> Result<GenerationEncodingObservation>,
) {
    let before = generation_fixture_tree(root);
    let warm = action(prepared).err().expect("committed collection must reject");
    assert_eq!(generation_fixture_tree(root), before);
    let memo = prepared.scope_encoding.take();
    let plain = action(prepared).err().expect("uncached collection must reject");
    assert_eq!(generation_fixture_tree(root), before);
    prepared.scope_encoding = memo;
    assert_eq!(generation_error(&warm), generation_error(&plain));
}

pub(in crate::engine::backup_preparation::native_control::databases::scoped::cutover::registration)
fn verify_generation_encoding_reuse(
    root: &Path,
    prepared: &mut PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<()> {
    let retained = prepared.scope_encoding.take();
    let before = generation_fixture_tree(root);
    let generation = generation_inputs::observe(prepared, decision, stop)?;
    let mut baseline = Vec::new();
    for index in 0..prepared.targets.len() {
        baseline.push(observe_generation_encoding(prepared, decision, &generation, index, stop)?);
    }
    assert_eq!(baseline.len(), 2);
    assert_eq!(generation_fixture_tree(root), before);
    prepared.scope_encoding = Some(encoding_memo::ScopeEncodingMemo::new());
    for warm in [false, true] {
        if warm {
            // Only this existing complete verifier may fill the shared memo.
            prepared.verify(stop)?;
        }
        for (index, expected) in baseline.iter().enumerate() {
            let observed = observe_generation_encoding(
                prepared, decision, &generation, index, stop,
            )?;
            compare_generation_observations(&observed, expected)?;
            assert_eq!(generation_fixture_tree(root), before);
        }
    }
    // Account immutability itself is covered by the memo module's private-state
    // tests; this real fixture observes complete outputs and filesystem effects.
    for index in 0..prepared.targets.len() {
        let path = prepared.targets[index].root.path().join("native-control/objects/snapshot")
            .join(format!("{}.json", prepared.task.selection().jobs()[index].snapshot));
        let original = fs::read(&path).unwrap();
        for bad in [b"{}".as_slice(), br#"{"a":1,"a":1}"#.as_slice()] {
            fs::write(&path, bad).unwrap();
            compare_generation_rejection(root, prepared, |prepared| {
                observe_generation_encoding(prepared, decision, &generation, index, stop)
            });
        }
        fs::write(&path, original).unwrap();
        let displaced = root.join("generation-encoding-missing-snapshot");
        assert!(!displaced.exists());
        fs::rename(&path, &displaced).unwrap();
        compare_generation_rejection(root, prepared, |prepared| {
            observe_generation_encoding(prepared, decision, &generation, index, stop)
        });
        fs::rename(&displaced, &path).unwrap();
    }
    let stopped = AtomicBool::new(true);
    compare_generation_rejection(root, prepared, |prepared| {
        let error = observe_generation_encoding(
            prepared, decision, &generation, 0, &stopped,
        ).err().expect("canceled collection must reject");
        assert_eq!(error.code(), ErrorCode::OperationAborted);
        Err(error)
    });
    for (index, expected) in baseline.iter().enumerate() {
        let observed = observe_generation_encoding(prepared, decision, &generation, index, stop)?;
        compare_generation_observations(&observed, expected)?;
    }
    assert_eq!(generation_fixture_tree(root), before);
    prepared.scope_encoding = retained;
    Ok(())
}
