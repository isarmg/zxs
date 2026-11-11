//! Immutable same-operation SYSTEM selection. These bytes only restrict native
//! re-verification; neither a selected plan nor a completion is an authority.
use super::*;
use crate::persist::layout;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;

const DIRECTORY: &str = "backup-root-extension";
const PLAN: &str = "plan.json";
const SELECTED: &str = "selected.json";
const COMPLETE: &str = "complete.json";
const DOMAIN: &str = "backup-bootstrap-root-extension";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "backup-bootstrap-root-extension")]
    Backup,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Plan {
    format: Format,
    pub(super) authority: PortableAuthority,
    pub(super) extension: PreparedRootExtension,
    pub(super) candidate: String,
    pub(super) publication: String,
    pub(super) registry: crate::DiskRegistryCommitment,
    config: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    pool: String,
    operation: OperationId,
    authority: String,
    plan: String,
}

impl Plan {
    pub(super) fn capture(
        admitted: &BackupWriterIoConfig,
        first: &PreparedBackupFirstCandidate,
        publication: &PreparedBackupFirstPublication,
        extension: PreparedRootExtension,
    ) -> Result<Self> {
        let config = admitted.config();
        let generation = config
            .generation()
            .ok_or_else(|| invalid("Backup expansion has no native generation"))?;
        publication.require_namespace_candidate(first)?;
        let registry = generation
            .registry()
            .ok_or_else(|| invalid("Backup expansion has no verified registry continuation"))?
            .clone();
        let plan = Self {
            format: Format::Backup,
            authority: generation.portable.clone(),
            extension,
            candidate: first.hash().into(),
            publication: publication.report().publication_hash.clone(),
            registry,
            config: object_hash("backup-effective-config", config)?,
        };
        if plan.selection()?.authority != first.authority_hash()
            || generation.intent.operation_id != first.result().operation_id
        {
            return Err(invalid(
                "Backup expansion changes its native first operation",
            ));
        }
        Ok(plan)
    }
    fn selection(&self) -> Result<Selection> {
        if !self.authority.is_backup() {
            return Err(invalid(
                "Backup expansion cannot replay another operation kind",
            ));
        }
        let intent = self.authority.intent()?;
        for hash in [&self.candidate, &self.publication, &self.config] {
            BackupFirstPublicationHash::parse(hash)?;
        }
        if object_hash(
            "backup-effective-config",
            &self.authority.effective_config()?,
        )? != self.config
        {
            return Err(invalid("Backup expansion changes its native configuration"));
        }
        Ok(Selection {
            pool: intent.target.pool_id.clone(),
            operation: intent.operation_id,
            authority: self.authority.hash()?,
            plan: object_hash(DOMAIN, self)?,
        })
    }
    pub(super) fn operation(&self) -> Result<OperationId> {
        Ok(self.selection()?.operation)
    }
    pub(super) fn require_config(&self, config: &ValidatedConfig) -> Result<()> {
        let generation = config
            .generation()
            .ok_or_else(|| invalid("Backup startup lacks its native continuation"))?;
        if generation.hash != self.authority.hash()?
            || generation.registry() != Some(&self.registry)
            || object_hash("backup-effective-config", config)? != self.config
        {
            return Err(invalid(
                "Backup startup changes its exact native configuration or registry",
            ));
        }
        Ok(())
    }
}

pub(super) struct PlanStore {
    system: DiskRoot,
    directory: DiskRoot,
    selection: Selection,
}
impl PlanStore {
    pub(super) fn open(root: &Path) -> Result<Option<(Self, Plan)>> {
        let system = DiskRoot::open(root)?;
        let path = system.path().join(DIRECTORY);
        if system.optional_metadata(&path)?.is_none() {
            return Ok(None);
        }
        let directory = system.private_child(&path)?;
        inventory(&directory)?;
        let Some(bytes) = directory.open_optional_read_regular(&directory.path().join(SELECTED))?
        else {
            // Before the selection no caller is permitted to alter a selector.
            return Ok(None);
        };
        let selection: Selection =
            decode_strict(&crate::disk::read_file_bytes(bytes, MAX_METADATA)?)?;
        let plan: Plan =
            decode_strict(&directory.read_bytes(&directory.path().join(PLAN), MAX_METADATA)?)?;
        if plan.selection()? != selection {
            return Err(invalid(
                "Backup root plan differs from its protected selection",
            ));
        }
        Ok(Some((
            Self {
                system,
                directory,
                selection,
            },
            plan,
        )))
    }
    pub(super) fn select(engine: &Engine, plan: &Plan) -> Result<Self> {
        let system = DiskRoot::open(&engine.root)?;
        require_writer(&system, engine)?;
        let bytes = canonical_json(plan)?;
        if bytes.len() > MAX_METADATA {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup expansion plan exceeds 64 MiB",
            ));
        }
        let required = (bytes.len() as u64)
            .checked_mul(2)
            .and_then(|n| n.checked_add(engine.config.metadata_reserve_bytes))
            .and_then(|n| n.checked_add(512 * 1024))
            .ok_or_else(|| invalid("Backup expansion capacity arithmetic overflow"))?;
        if system.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "SYSTEM cannot retain the exact Backup expansion plan and completion",
            ));
        }
        system.private_dir(&system.path().join(DIRECTORY))?;
        let directory = system.private_child(&system.path().join(DIRECTORY))?;
        inventory(&directory)?;
        let selection = plan.selection()?;
        directory.write_json(&directory.path().join(PLAN), plan, false)?;
        directory.write_json(&directory.path().join(SELECTED), &selection, false)?;
        let saved = Self {
            system,
            directory,
            selection,
        };
        saved.verify()?;
        Ok(saved)
    }
    pub(super) fn verify(&self) -> Result<()> {
        inventory(&self.directory)?;
        let plan: Plan = decode_strict(
            &self
                .directory
                .read_bytes(&self.directory.path().join(PLAN), MAX_METADATA)?,
        )?;
        let selected: Selection = decode_strict(
            &self
                .directory
                .read_bytes(&self.directory.path().join(SELECTED), MAX_METADATA)?,
        )?;
        if selected != self.selection || plan.selection()? != selected {
            return Err(invalid("Backup selection changed during its transaction"));
        }
        Ok(())
    }
    pub(super) fn verify_writer(&self, engine: &Engine) -> Result<()> {
        require_writer(&self.system, engine)?;
        self.verify()
    }
    pub(super) fn completed(&self) -> Result<bool> {
        self.verify()?;
        match self
            .directory
            .open_optional_read_regular(&self.directory.path().join(COMPLETE))?
        {
            None => Ok(false),
            Some(file) => {
                let complete: Selection =
                    decode_strict(&crate::disk::read_file_bytes(file, MAX_METADATA)?)?;
                if complete != self.selection {
                    return Err(invalid("Backup root completion differs from its selection"));
                }
                Ok(true)
            }
        }
    }
    pub(super) fn complete(&self) -> Result<()> {
        self.verify()?;
        self.directory.write_json(
            &self.directory.path().join(COMPLETE),
            &self.selection,
            false,
        )
    }
}
fn require_writer(system: &DiskRoot, engine: &Engine) -> Result<()> {
    let named = system.open_regular_entry(&layout::CONTROL_WRITER_LOCK)?;
    if FileIdentity::from_fd(&named)? != engine._locks.system_identity()? {
        return Err(invalid("Backup expansion lost its original SYSTEM writer"));
    }
    Ok(())
}
fn inventory(root: &DiskRoot) -> Result<()> {
    for entry in root.entries(root.path())? {
        let entry = entry.map_err(crate::disk::fs_error)?;
        if !matches!(entry.as_os_str().to_str(), Some(PLAN | SELECTED | COMPLETE)) {
            return Err(invalid(
                "unknown or uncertain Backup expansion entry is retained without adoption",
            ));
        }
    }
    Ok(())
}
