//! Local file identity is durable before a global completion. Cold inspection
//! only reacquires the exact existing name; it never creates a missing lock.
use super::*;
use crate::persist::layout;

const LOCAL_INTENT: &str = "backup-target-writer-intent.json";
const BINDING: &str = "backup-target-writer-binding.json";

#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
enum IntentFormat {
    BackupTargetWriterIntent,
}
#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
enum BindingFormat {
    BackupTargetWriterBinding,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    format: IntentFormat,
    plan_hash: String,
    identity: DiskRegistrationIdentity,
    directory: PhysicalBinding,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    format: BindingFormat,
    intent_hash: String,
    writer: PhysicalBinding,
}

fn intent(target: &Target, hash: &str, identity: &DiskRegistrationIdentity) -> Result<Vec<u8>> {
    bounded(&Intent {
        format: IntentFormat::BackupTargetWriterIntent,
        plan_hash: hash.into(),
        identity: identity.clone(),
        directory: PhysicalBinding::of(&target.root)?,
    })
}
fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn read_intent(target: &Target) -> Result<Vec<u8>> {
    let root = &target.root;
    let bytes = root.read_bytes(&root.path().join(LOCAL_INTENT), MAX_RECORD)?;
    let saved: Intent = crate::journal::decode_proof(&bytes)?;
    if saved.plan_hash.len() != 64
        || !saved
            .plan_hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || saved.directory != PhysicalBinding::of(root)?
        || saved.directory != PhysicalBinding::of(&DiskRoot::open(root.path())?)?
        || bounded(&saved)? != bytes
    {
        return Err(invalid(
            "Backup writer intent changed its physical root or bounded identity",
        ));
    }
    Ok(bytes)
}

fn read_binding(target: &Target, intent: &[u8]) -> Result<PhysicalBinding> {
    let root = &target.root;
    let bytes = root.read_bytes(&root.path().join(BINDING), MAX_RECORD)?;
    let saved: Binding = crate::journal::decode_proof(&bytes)?;
    if saved.intent_hash != digest(intent) || bounded(&saved)? != bytes {
        return Err(invalid(
            "Backup writer binding belongs to another exact target intent",
        ));
    }
    target.require_writer(&saved.writer)?;
    Ok(saved.writer)
}

/// This file is a resource reservation, not an authority. Its only permissible
/// read-side effect is locking the fixed existing file under an explicitly
/// selected root. No path or Config is obtained from these bytes.
pub(in super::super) fn resume_existing(targets: &[Target]) -> Result<()> {
    for target in targets {
        let root = &target.root;
        let selected = preparation_path_exists(root, &root.path().join(LOCAL_INTENT))?;
        let bound = preparation_path_exists(root, &root.path().join(BINDING))?;
        let writer = preparation_path_exists(
            root,
            &root.path().join(layout::DISK_WRITER_LOCK.as_path()),
        )?;
        if !selected {
            if writer || bound {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "unowned Backup target writer material retained without adoption",
                ));
            }
            continue;
        }
        let intent = read_intent(target)?;
        if writer {
            target.acquire_writer()?;
        }
        if bound {
            // Missing names and copied/replaced inodes are always fatal once
            // this local binding exists, even before global completion.
            read_binding(target, &intent)?;
        }
    }
    Ok(())
}

pub(super) fn preflight(
    target: &Target,
    hash: &str,
    identity: &DiskRegistrationIdentity,
) -> Result<()> {
    native_metadata::require_unselected_target(&target.root)?;
    let root = &target.root;
    if preparation_path_exists(root, &root.path().join(LOCAL_INTENT))? {
        let saved = read_intent(target)?;
        if saved != intent(target, hash, identity)? {
            return Err(invalid(
                "Backup writer reservation belongs to another original plan",
            ));
        }
        if preparation_path_exists(root, &root.path().join(BINDING))? {
            read_binding(target, &saved)?;
        }
    } else if preparation_path_exists(root, &root.path().join(layout::DISK_WRITER_LOCK.as_path()))?
        || preparation_path_exists(root, &root.path().join(BINDING))?
    {
        return Err(Error::new(
            ErrorCode::ObjectCollision,
            "Backup writer name has no original durable intent; preserved unchanged",
        ));
    }
    Ok(())
}

pub(super) fn install(
    target: &Target,
    hash: &str,
    identity: &DiskRegistrationIdentity,
) -> Result<()> {
    preflight(target, hash, identity)?;
    let root = &target.root;
    let bytes = intent(target, hash, identity)?;
    install_preparation_bytes(root, &root.path().join(LOCAL_INTENT), &bytes)?;
    if preparation_path_exists(root, &root.path().join(BINDING))? {
        read_binding(target, &bytes)?;
        return Ok(());
    }
    // Before a binding exists, an interrupted original empty-file publication
    // may be completed. Existing nonempty/unsafe files are never rewritten.
    install_preparation_bytes(root, &root.path().join(layout::DISK_WRITER_LOCK.as_path()), &[])?;
    let writer = target.acquire_writer()?;
    root.open_regular_entry(&layout::DISK_WRITER_LOCK)?.sync_all()?;
    root.sync_dir(root.path())?;
    target.require_writer(&writer)?;
    let binding = bounded(&Binding {
        format: BindingFormat::BackupTargetWriterBinding,
        intent_hash: digest(&bytes),
        writer,
    })?;
    install_preparation_bytes(root, &root.path().join(BINDING), &binding)?;
    read_binding(target, &bytes)?;
    Ok(())
}

pub(super) fn observe(
    target: &Target,
    hash: &str,
    identity: &DiskRegistrationIdentity,
) -> Result<PhysicalBinding> {
    let saved = read_intent(target)?;
    if saved != intent(target, hash, identity)? {
        return Err(invalid(
            "Backup writer inspection does not match its exact native plan",
        ));
    }
    native_metadata::require_unselected_target(&target.root)?;
    read_binding(target, &saved)
}

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_target_writer_tests.rs"]
mod tests;
