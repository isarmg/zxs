//! Actual SYSTEM/root read interval inside the existing serving-cost fixture.
//! All source tokens, targets and installations come from that real decision.
use super::*;
use std::{
    collections::BTreeMap,
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
) -> Result<BackupSystemDatabaseReport> {
    // The old composition: independently capture the complete root interval
    // before each of the two actual SYSTEM observations.
    let before = Plan::capture(context, prepared, decision, expected, stop)?;
    let selected = Selected::read(context.system, &before)?;
    selected.require_complete(context.system, &before, stop)?;
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if before.binding_hash != after.binding_hash {
        return Err(invalid(
            "SYSTEM native installation proof changed during observation",
        ));
    }
    selected.require_complete(context.system, &after, stop)?;
    Ok(after.report(&selected))
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
    let result = (|| {
        // The production interval's same private root kernel. Injection follows
        // the real first SYSTEM read, not a synthesized successful report.
        let (roots, (before, selected)) = root_databases::observe_around(
            context, prepared, decision, expected, stop,
            |roots| {
                let before = Plan::capture_with_roots(
                    context, prepared, decision, expected, roots.clone(), stop,
                )?;
                let selected = Selected::read(context.system, &before)?;
                selected.require_complete(context.system, &before, stop)?;
                change()?;
                after_change = Some(media(temporary));
                Ok((before, selected))
            },
        )?;
        let after = Plan::capture_with_roots(
            context, prepared, decision, expected, roots, stop,
        )?;
        if before.binding_hash != after.binding_hash {
            return Err(invalid(
                "SYSTEM native installation proof changed during observation",
            ));
        }
        selected.require_complete(context.system, &after, stop)?;
        Ok(after.report(&selected))
    })();
    let actual = result.unwrap_err();
    let after_change = after_change.expect("real first root/SYSTEM checks must succeed");
    assert_eq!(media(temporary), after_change);
    let reference = rebuilt(context, prepared, decision, expected, stop).unwrap_err();
    assert_eq!(media(temporary), after_change);
    assert_eq!(actual.code(), reference.code());
    Ok(actual.code())
}

pub(in super::super) fn verify_read_interval(
    temporary: &Path,
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    body: &Path,
    quiescent: &QuiescentIndex,
    stop: &AtomicBool,
) -> Result<()> {
    // Actual production installs under the fixture's retained lock/quiescence
    // tokens. No manually written selector, completion or expected database.
    database_aliases::install(context, prepared, decision, expected, stop)?;
    root_databases::install(context, prepared, decision, expected, stop)?;
    install(context, prepared, decision, expected, quiescent, stop)?;
    let unchanged = media(temporary);
    let actual = observe(context, prepared, decision, expected, stop)?;
    let reference = rebuilt(context, prepared, decision, expected, stop)?;
    assert_eq!(format!("{actual:?}"), format!("{reference:?}"));
    assert_eq!(media(temporary), unchanged);
    assert_eq!(actual.roots.selected_targets, prepared.targets.len());

    let mut ordinary_disk = None;
    context.sources.for_each_disk(
        stop,
        |root, _| {
            ordinary_disk.get_or_insert_with(|| {
                root.path().join(crate::database_install::DISK_DATABASE_NAME)
            });
            Ok(())
        },
        &prepared.task.selection().first().pool_id,
    )?;
    let ordinary_disk = ordinary_disk.expect("actual original members exist");
    let system_index = context.system.path().join("index.sqlite");
    // A BODY and ordinary DISK pair require the second full root read; the
    // actual SYSTEM index requires the final selected/retained-set read.
    for path in [body, ordinary_disk.as_path(), system_index.as_path()] {
        let original = fs::read(path)?;
        assert!(!original.is_empty());
        let mut damaged = original.clone();
        damaged[0] ^= 1;
        late_failure(temporary, context, prepared, decision, expected, stop, || {
            fs::write(path, &damaged)?;
            Ok(())
        })?;
        assert_eq!(fs::read(path)?, damaged);
        fs::write(path, &original)?;
    }

    let target = &prepared.targets[1];
    let physical = PhysicalBinding::of(&target.root)?;
    let parked = temporary.join("system-read-retained-target");
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
    assert_eq!(media(temporary), unchanged);
    quiescent.require_root(context.system)?;
    Ok(())
}
