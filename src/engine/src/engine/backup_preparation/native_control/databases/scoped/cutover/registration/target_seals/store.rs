//! No-clobber task-local seal installation. The intent permits only matching
//! frozen reads before a selector exists; it cannot construct an I/O capability.
use super::*;
use std::os::unix::fs::MetadataExt;

const LOCAL_INTENT: &str = "backup-target-seal-intent.json";
const BINDING: &str = "backup-target-seal-binding.json";
const SEAL: &str = "disk.json";
const MAX_LOCAL: usize = 8 * 1024;

#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
enum IntentFormat {
    BackupFencedTargetSeal,
}
#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
enum BindingFormat {
    BackupTargetSealBinding,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    format: IntentFormat,
    plan_hash: String,
    native_installation_hash: String,
    admission_hash: String,
    seal: DiskSeal,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    format: BindingFormat,
    intent_hash: String,
    file: PhysicalBinding,
}

pub(super) struct PreparedSeal {
    intent: Intent,
    bytes: Vec<u8>,
    seal: Vec<u8>,
}

fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn local_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_LOCAL {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "Backup local seal record exceeds 8 KiB",
        ));
    }
    Ok(bytes)
}

fn require_digest(value: &str) -> Result<()> {
    value
        .parse::<storage_contract::ContentHash>()
        .map(|_| ())
        .map_err(|source| {
            Error::from(std::io::Error::other(source)).context(
                ErrorCode::ObjectCorrupt,
                "Backup seal intent contains an invalid native digest",
            )
        })
}

fn validate_intent(root: &DiskRoot, intent: &Intent, native: Option<&str>) -> Result<()> {
    for hash in [
        &intent.plan_hash,
        &intent.native_installation_hash,
        &intent.admission_hash,
    ] {
        require_digest(hash)?;
    }
    if native != Some(intent.native_installation_hash.as_str()) {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "Backup target seal lacks its exact unselected native metadata stage",
        ));
    }
    let seal = &intent.seal;
    let DiskSealFormat::MembershipGeneration { authority_hash } = &seal.format else {
        return Err(invalid(
            "Backup target seal cannot impersonate a Genesis disk",
        ));
    };
    require_digest(authority_hash)?;
    crate::validate_id(&seal.pool_id, "pool")?;
    crate::validate_id(&seal.disk_id, "disk")?;
    name_policy::validate_slot(&seal.slot_id)?;
    let physical = PhysicalBinding::of(root)?;
    if seal.binding_generation == 0
        || seal.device != physical.device
        || seal.inode != physical.inode
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "Backup target seal intent changes its held physical directory",
        ));
    }
    Ok(())
}

fn require_seal(root: &DiskRoot, expected: &[u8]) -> Result<PhysicalBinding> {
    // Keep bound I/O ownership through metadata, bytes and the error tail.
    // This opener also enforces single-link regular-file semantics.
    let file = root
        .open_optional_read_regular(&root.path().join(SEAL))?
        .ok_or_else(|| {
            Error::from(std::io::Error::from(std::io::ErrorKind::NotFound)).context(
                ErrorCode::DiskIdentityMismatch,
                "Backup target disk.json is missing",
            )
        })?;
    let metadata = file.metadata()?;
    let identity = PhysicalBinding {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    if crate::disk::read_file_bytes(file, MAX_LOCAL)? != expected {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "Backup target disk.json differs from the original native DiskWriter seal",
        ));
    }
    Ok(identity)
}

fn require_binding(root: &DiskRoot, intent: &[u8], seal: &[u8]) -> Result<PhysicalBinding> {
    let bytes = root.read_bytes(&root.path().join(BINDING), MAX_LOCAL)?;
    let binding: Binding = crate::journal::decode_proof(&bytes)?;
    if binding.intent_hash != digest(intent) || local_bytes(&binding)? != bytes {
        return Err(invalid(
            "Backup seal file binding changed its exact original intent",
        ));
    }
    if require_seal(root, seal)? != binding.file {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "Backup target disk.json was replaced after its durable file binding",
        ));
    }
    Ok(binding.file)
}

/// Used by frozen staging inventory, before rebuilding the full native proof.
/// It admits no writer and reads no paths supplied by the record. A missing
/// post-binding seal fails here, including on a cold original-task retry.
pub(in super::super) fn require_staging(root: &DiskRoot, native: Option<&str>) -> Result<()> {
    read_staging(root, native).map(|_| ())
}

fn read_staging(root: &DiskRoot, native: Option<&str>) -> Result<Option<Vec<u8>>> {
    let selected = preparation_path_exists(root, &root.path().join(LOCAL_INTENT))?;
    let bound = preparation_path_exists(root, &root.path().join(BINDING))?;
    let sealed = preparation_path_exists(root, &root.path().join(SEAL))?;
    if !selected {
        if sealed || bound {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "frozen Backup reader cannot adopt an unowned runtime seal; files retained",
            ));
        }
        return Ok(None);
    }
    let bytes = root.read_bytes(&root.path().join(LOCAL_INTENT), MAX_LOCAL)?;
    let intent: Intent = crate::journal::decode_proof(&bytes)?;
    validate_intent(root, &intent, native)?;
    if local_bytes(&intent)? != bytes {
        return Err(invalid(
            "Backup target seal intent is not its exact canonical record",
        ));
    }
    let seal = local_bytes(&intent.seal)?;
    if bound {
        require_binding(root, &bytes, &seal)?;
    } else if sealed {
        // This is the sole creation-interruption window: the original intent
        // exists, its exact seal exists, and no inode has yet been committed.
        require_seal(root, &seal)?;
    }
    Ok(Some(bytes))
}

impl PreparedSeal {
    pub(super) fn new(plan: &str, native: &str, admission: &str, seal: DiskSeal) -> Result<Self> {
        let intent = Intent {
            format: IntentFormat::BackupFencedTargetSeal,
            plan_hash: plan.into(),
            native_installation_hash: native.into(),
            admission_hash: admission.into(),
            seal,
        };
        Ok(Self {
            bytes: local_bytes(&intent)?,
            seal: local_bytes(&intent.seal)?,
            intent,
        })
    }

    pub(super) fn seal_hash(&self) -> String {
        digest(&self.seal)
    }

    pub(super) fn preflight(&self, root: &DiskRoot) -> Result<()> {
        validate_intent(
            root,
            &self.intent,
            Some(&self.intent.native_installation_hash),
        )?;
        if let Some(bytes) = read_staging(root, Some(&self.intent.native_installation_hash))?
            && bytes != self.bytes
        {
            return Err(invalid(
                "Backup target seal belongs to another original native plan",
            ));
        }
        Ok(())
    }

    pub(super) fn install(&self, root: &DiskRoot) -> Result<()> {
        self.preflight(root)?;
        install_preparation_bytes(root, &root.path().join(LOCAL_INTENT), &self.bytes)?;
        if preparation_path_exists(root, &root.path().join(BINDING))? {
            self.observe(root)?;
            return Ok(());
        }
        // No replacement and no Genesis fallback. The actual writer gate lease
        // is held by the caller through this complete write/sync/error tail.
        install_preparation_bytes(root, &root.path().join(SEAL), &self.seal)?;
        let file = require_seal(root, &self.seal)?;
        let binding = local_bytes(&Binding {
            format: BindingFormat::BackupTargetSealBinding,
            intent_hash: digest(&self.bytes),
            file,
        })?;
        install_preparation_bytes(root, &root.path().join(BINDING), &binding)?;
        self.observe(root)?;
        Ok(())
    }

    pub(super) fn observe(&self, root: &DiskRoot) -> Result<PhysicalBinding> {
        self.preflight(root)?;
        if root.read_bytes(&root.path().join(LOCAL_INTENT), MAX_LOCAL)? != self.bytes {
            return Err(invalid("Backup target seal lost its exact original intent"));
        }
        require_binding(root, &self.bytes, &self.seal)
    }
}

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_target_seal_tests.rs"]
mod tests;
