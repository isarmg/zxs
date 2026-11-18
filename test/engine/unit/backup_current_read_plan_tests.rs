//! Differential readback inside the existing genuine native serving fixture.
//! No new source constructor, fake receipt or independent runtime test target.
use super::*;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

fn rebuilt(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupCurrentReadReport> {
    // Preserve the pre-change composition as an independent reference: two
    // full common captures and a separate namespace recipe in between.
    let before = common_databases::observe(context, prepared, decision, expected, stop)?;
    let plan = published_registry::Plan::capture(prepared, decision, expected, stop)?;
    let counts = namespace_counts(&plan, prepared, expected, stop)?;
    let common = common_databases::observe(context, prepared, decision, expected, stop)?;
    if common.common_manifest_hash != before.common_manifest_hash {
        return Err(invalid(
            "Backup common database binding changed during namespace admission",
        ));
    }
    Ok(BackupCurrentReadReport {
        common,
        targets: prepared.targets.len(),
        branch_scopes: counts.branches,
        readable_branch_scopes: counts.readable_branches,
        registered_folders: counts.registered_folders,
        entries_per_target: counts.entries,
        deleted_files: counts.deletions,
        unavailable_versions: counts.unavailable,
    })
}

type Media = BTreeMap<PathBuf, (u64, u64, Option<Vec<u8>>)>;
fn media(root: &Path) -> Media {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        let bytes = if metadata.is_dir() {
            result.extend(media(&path));
            None
        } else {
            Some(fs::read(&path).unwrap())
        };
        result.insert(path, (metadata.dev(), metadata.ino(), bytes));
    }
    result
}

fn late_failure(
    temporary: &Path,
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
    change: impl FnOnce() -> Result<()>,
) -> Result<ErrorCode> {
    let mut after_change = None;
    let result = common_databases::observe_with_namespaces(
        context, prepared, decision, expected, stop,
        |plan| {
            namespace_counts(plan, prepared, expected, stop)?;
            change()?;
            after_change = Some(media(temporary));
            Ok(())
        },
    );
    let retained = result.err().expect("late change must not reuse an earlier success");
    let after_change = after_change.expect("the actual namespace check must run first");
    assert_eq!(media(temporary), after_change);
    let reference = rebuilt(context, prepared, decision, expected, stop).unwrap_err();
    assert_eq!(media(temporary), after_change);
    assert_eq!(retained.code(), reference.code());
    Ok(retained.code())
}

pub(in super::super) fn verify_same_invocation_readback(
    temporary: &Path,
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    body: &Path,
    stop: &AtomicBool,
) -> Result<()> {
    let actual = observe(context, prepared, decision, expected, stop)?;
    let reference = rebuilt(context, prepared, decision, expected, stop)?;
    assert_eq!(format!("{actual:?}"), format!("{reference:?}"));
    assert_eq!(actual.targets, prepared.targets.len());
    assert!(actual.readable_branch_scopes > 0);
    assert_eq!(actual.registered_folders, 2);
    assert!(actual.entries_per_target > 0);

    // Change a real target SQLite and then a real held-source BODY only after
    // the initial full common and namespace checks have genuinely succeeded.
    let sqlite = prepared.targets[1].root.path()
        .join(crate::database_install::DISK_DATABASE_NAME);
    for path in [sqlite.as_path(), body] {
        let original = fs::read(path)?;
        let mut damaged = original.clone();
        damaged[0] ^= 1;
        late_failure(temporary, context, prepared, decision, expected, stop, || {
            fs::write(path, &damaged)?;
            Ok(())
        })?;
        assert_eq!(fs::read(path)?, damaged);
        fs::write(path, &original)?;
    }

    // Reusing the same pathname cannot supply a replacement target root.
    let target = &prepared.targets[1];
    let physical = PhysicalBinding::of(&target.root)?;
    let parked = temporary.join("current-read-retained-target");
    assert_eq!(
        late_failure(temporary, context, prepared, decision, expected, stop, || {
            fs::rename(target.root.path(), &parked)?;
            fs::DirBuilder::new().mode(0o700).create(target.root.path())?;
            Ok(())
        })?,
        ErrorCode::DiskIdentityMismatch,
    );
    assert!(fs::read_dir(target.root.path())?.next().is_none());
    fs::remove_dir(target.root.path())?;
    fs::rename(&parked, target.root.path())?;
    assert_eq!(PhysicalBinding::of(&target.root)?, physical);

    let cancelled = AtomicBool::new(false);
    assert_eq!(
        late_failure(temporary, context, prepared, decision, expected, &cancelled, || {
            cancelled.store(true, Ordering::Release);
            Ok(())
        })?,
        ErrorCode::OperationAborted,
    );
    let restored = observe(context, prepared, decision, expected, stop)?;
    assert_eq!(format!("{actual:?}"), format!("{restored:?}"));
    Ok(())
}
