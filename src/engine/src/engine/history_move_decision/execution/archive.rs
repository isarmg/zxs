//! Retire singleton preparation names only after actual publication and every
//! serving scope. No file is deleted: exact old material stays under its op ID.
use super::*;
use platform_io::fs_safety::linux::FileIdentity;
use std::path::Path;

struct Owner {
    root: DiskRoot,
    disk: Option<DiskConfig>,
    metadata: bool,
    target: bool,
    identity: FileIdentity,
}
impl Owner {
    fn check(&self, engine: &Engine) -> Result<()> {
        let named = DiskRoot::open(self.root.path())?;
        if named.directory_identity(named.path())? != self.identity {
            return Err(invalid("move completion owner root changed"));
        }
        if let Some(disk) = &self.disk {
            let handle = engine.disk_resources.get(disk)?;
            let lease = handle.root.admit_io()?;
            let seal =
                validate_seal_observed(&engine.config, disk, &mut |_| lease.check_running())?;
            validate_stored_seal(&handle.seal, &seal)?;
        } else {
            let writer = self
                .root
                .open_regular(&self.root.path().join("writer.lock"))?;
            if FileIdentity::from_fd(&writer)? != engine._locks.system_identity()? {
                return Err(invalid("move completion SYSTEM writer changed"));
            }
        }
        Ok(())
    }
}

fn marker(
    root: &DiskRoot,
    path: &Path,
    kind: &str,
    published: &PublishedMove,
    choice: &Choice,
) -> Result<()> {
    let directory = root.private_child(path)?;
    if kind == DIRECTORY {
        return choice.require_archive_choice(&directory);
    }
    let (name, key) = match kind {
        "file-history-move-recovery" => ("preparation.json", "operation_id"),
        "file-history-move-target" => ("target-intent.json", "operation_id"),
        "file-history-move-ordinary" => ("ordinary-inputs.json", "candidate_hash"),
        _ => unreachable!("invariant: fixed original move preparation directories"),
    };
    let held = HeldRegular::capture(&directory, name, MANIFEST_BYTES)?.ok_or_else(|| {
        invalid("move completion cannot retire an unidentified preparation directory")
    })?;
    let decoded: json_codec::Value = storage_contract::decode_strict(held.bytes())?;
    let object = if kind == "file-history-move-recovery" {
        decoded.get("record")
    } else {
        Some(&decoded)
    }
    .ok_or_else(|| invalid("move completion preparation has no original record"))?;
    let expected = if key == "candidate_hash" {
        json_codec::to_value(&published.hash)?
    } else {
        json_codec::to_value(published.candidate.operation_id)?
    };
    if object.get(key) != Some(&expected) {
        return Err(invalid(
            "move completion found another operation's preparation",
        ));
    }
    held.check_running()
}

fn retire_one(
    owner: &Owner,
    engine: &Engine,
    kind: &str,
    published: &PublishedMove,
    choice: &Choice,
) -> Result<()> {
    owner.check(engine)?;
    let root = &owner.root;
    let completed = root.path().join("file-history-move-completed");
    root.private_dir(&completed)?;
    let operation = completed.join(published.candidate.operation_id.to_string());
    root.private_dir(&operation)?;
    let source = root.path().join(kind);
    let destination = operation.join(kind);
    let old = root.optional_metadata(&source)?;
    let retained = root.optional_metadata(&destination)?;
    match (old, retained) {
        (Some(_), None) => {
            marker(root, &source, kind, published, choice)?;
            let identity = root.directory_identity(&source)?;
            let held = root.private_child_matching(&source, identity)?;
            root.rename_noreplace_destination_first_synced(&source, &destination)?;
            if root.directory_identity(&destination)? != identity
                || held.directory_identity(held.path())? != identity
            {
                return Err(invalid(
                    "move preparation changed during preserving retirement",
                ));
            }
            marker(root, &destination, kind, published, choice)?;
        }
        (None, Some(_)) => {
            marker(root, &destination, kind, published, choice)?;
            // A prior rename may have succeeded before its final directory sync.
            root.sync_dir(&operation)?;
            root.sync_dir(root.path())?;
        }
        (Some(_), Some(_)) => {
            return Err(invalid(
                "move completion has conflicting original and archived directories",
            ));
        }
        (None, None) => {
            return Err(invalid(
                "move completion lost its original preparation directory",
            ));
        }
    }
    owner.check(engine)
}

pub(super) fn retire(engine: &mut Engine, published: &PublishedMove) -> Result<()> {
    let choice = engine
        .history_move_choice
        .as_ref()
        .expect("invariant: selected move retains its fence until completion")
        .clone();
    let system = DiskRoot::open(&engine.root)?;
    let mut owners = vec![Owner {
        identity: system.directory_identity(system.path())?,
        root: system,
        disk: None,
        metadata: true,
        target: false,
    }];
    for identity in choice.participants() {
        let disk = engine.disk(&identity.slot_id)?;
        let root = engine
            .disk_resources
            .get(disk)?
            .root
            .private_child(disk.root.as_path())?;
        owners.push(Owner {
            identity: root.directory_identity(root.path())?,
            root,
            disk: Some(disk.clone()),
            metadata: disk.metadata_member,
            target: published
                .candidate
                .versions
                .iter()
                .any(|item| item.placement.route_slot_ids.contains(&disk.slot_id)),
        });
    }
    // Keep every choice name until all other material on every participant has
    // moved safely. A cold opener can still identify the one decided slot.
    for owner in &owners {
        if owner.metadata {
            retire_one(
                owner,
                engine,
                "file-history-move-recovery",
                published,
                &choice,
            )?;
        }
        if owner.target {
            retire_one(
                owner,
                engine,
                "file-history-move-target",
                published,
                &choice,
            )?;
            retire_one(
                owner,
                engine,
                "file-history-move-ordinary",
                published,
                &choice,
            )?;
        }
    }
    #[cfg(feature = "fault-injection")]
    engine.failpoint("move_before_choice_retirement")?;
    for (_index, owner) in owners.iter().enumerate() {
        retire_one(owner, engine, DIRECTORY, published, &choice)?;
        #[cfg(feature = "fault-injection")]
        engine.failpoint(&format!("move_after_choice_retirement_{}", _index + 1))?;
    }
    Ok(())
}
