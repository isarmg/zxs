//! Separate, inert original-scope databases derived from each capsule's own
//! full native prefix. Never install these into active or new-identity roots.
use super::*;
use crate::engine::foreign_databases::{MAX_BYTES, PreparedForeignDatabases};
use transaction::ensure_exact;

const DIRECTORY: &str = "foreign-chain-databases";
const FORMAT: &str = "zxs-foreign-original-three-scope-databases";
const MAX_PLAN: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignOriginalDatabasesReport {
    pub original: ForeignOriginalStateReport,
    pub capsule_digest: String,
    pub chain_digest: String,
    pub target_level: u32,
    pub manifest_hash: String,
    pub original_disks: usize,
    pub file_databases: usize,
    pub directory_databases: usize,
    pub databases: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub database_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForeignDatabaseStatus {
    NotStarted,
    Preparing,
    Verified {
        report: Box<ForeignOriginalDatabasesReport>,
    },
}
#[derive(Serialize)]
struct DatabaseIntent<'a> {
    format: &'static str,
    operation: OperationId,
    parent: PhysicalBinding,
    report: &'a ForeignOriginalDatabasesReport,
    inventory: &'a Inventory,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    identity: PhysicalBinding,
    intent_digest: String,
}
struct Prepared {
    images: PreparedForeignDatabases,
    inventory: Inventory,
    report: ForeignOriginalDatabasesReport,
    intent: Vec<u8>,
}
fn budget() -> InspectionBudget {
    InspectionBudget {
        metadata_bytes: MAX_BYTES,
        journal_bytes: 1,
        versions: 100_000,
        payload_bytes: 0,
    }
}
impl ForeignMigrationTransaction {
    /// Derive every original scope from this exact target-local native prefix.
    /// No SQLite or self-reported marker elects its source State.
    pub fn preview_original_databases(
        &self,
    ) -> Result<(ForeignOriginalDatabasesReport, ForeignDatabaseStatus)> {
        let prepared = Prepared::capture(self)?;
        let status = prepared.status(self)?;
        Ok((prepared.report, status))
    }
    /// Read-only original status, including full independent image comparison
    /// after completion. No missing member, image or marker is repaired.
    pub fn original_database_status(&self) -> Result<ForeignDatabaseStatus> {
        let prepared = Prepared::capture(self)?;
        prepared.status(self)
    }
    /// Prepare a separate inert sibling, retaining every original identity.
    /// Completed missing/corrupt material is rejected rather than rebuilt.
    pub fn prepare_original_databases(&self) -> Result<ForeignOriginalDatabasesReport> {
        let prepared = Prepared::capture(self)?;
        let status = prepared.status(self)?;
        let reservation = reservation(self);
        let path = self.parent.path().join(DIRECTORY);
        if matches!(status, ForeignDatabaseStatus::Verified { .. }) {
            let root = self.parent.private_child(&path)?;
            prepared.complete(self, &root)?;
            return Ok(prepared.report);
        }
        if status == ForeignDatabaseStatus::NotStarted {
            ensure_exact(&self.parent, &reservation, &prepared.intent)?;
        } else {
            self.parent.open_regular(&reservation)?.sync_all()?;
            self.parent.sync_dir(self.parent.path())?;
        }
        self.parent.private_dir(&path)?;
        let root = self.parent.private_child(&path)?;
        let binding_path = path.join("binding.json");
        if root.optional_metadata(&binding_path)?.is_none() {
            unbound(&root)?;
            ensure_exact(&root, &binding_path, &prepared.binding(&root)?)?;
        }
        prepared.check_binding(&root)?;
        top_level(&root)?;
        if root
            .optional_metadata(&path.join("complete.json"))?
            .is_some()
        {
            prepared.complete(self, &root)?;
            return Ok(prepared.report);
        }
        ensure_exact(&root, &path.join("intent.json"), &prepared.intent)?;
        root.private_dir(&path.join("databases"))?;
        let data = root.private_child(&path.join("databases"))?;
        prepared
            .inventory
            .quarantine_residue(&data, &root, "databases", &budget())?;
        for directory in &prepared.inventory.directories {
            data.private_dir(&data.path().join(directory))?;
        }
        for (relative, bytes) in &prepared.images.files {
            ensure_exact(&data, &data.path().join(relative), bytes)?;
        }
        prepared.inventory.verify(&data, &budget())?;
        prepared.inventory.sync(&data)?;
        ensure_exact(
            &root,
            &path.join("complete.json"),
            &prepared.binding(&root)?,
        )?;
        prepared.complete(self, &root)?;
        Ok(prepared.report)
    }
}
fn reservation(transaction: &ForeignMigrationTransaction) -> PathBuf {
    transaction.parent.path().join(format!(
        ".zxs-foreign-databases-{}.json",
        transaction.intent.plan.operation_id
    ))
}
fn parent_completion(transaction: &ForeignMigrationTransaction) -> PathBuf {
    transaction.parent.path().join(format!(
        ".zxs-foreign-databases-{}-complete.json",
        transaction.intent.plan.operation_id
    ))
}
impl Prepared {
    fn capture(transaction: &ForeignMigrationTransaction) -> Result<Self> {
        transaction.check_names()?;
        let Placement::ChainMaterial {
            chain_digest,
            target_level,
        } = &transaction.intent.plan.placement
        else {
            return Err(invalid(
                "original databases require the complete-chain material lane",
            ));
        };
        if !transaction.published {
            return Err(invalid(
                "all original capsules must be published before database preparation",
            ));
        }
        let source = transaction.verify_replica()?;
        projection::Projection::from_replica(&transaction.intent.plan, &source)?
            .verify(&transaction.capsule, &transaction.intent.plan.budget)?;
        let images = PreparedForeignDatabases::capture(&source)?;
        let mut inventory = Inventory {
            directories: BTreeSet::new(),
            files: BTreeMap::new(),
        };
        for (path, bytes) in &images.files {
            for parent in path
                .ancestors()
                .skip(1)
                .filter(|path| !path.as_os_str().is_empty())
            {
                inventory.directories.insert(parent.into());
            }
            inventory.files.insert(
                path.clone(),
                FileCommitment {
                    bytes: bytes.len() as u64,
                    digest: blake3::hash(bytes).to_hex().to_string(),
                },
            );
        }
        inventory.validate(&budget())?;
        let report = ForeignOriginalDatabasesReport {
            original: images.state.clone(),
            capsule_digest: transaction.digest.to_string(),
            chain_digest: chain_digest.clone(),
            target_level: *target_level,
            manifest_hash: object_hash("foreign-original-database-inventory", &inventory)?,
            original_disks: images.original_disks,
            file_databases: images.object_files,
            directory_databases: images.object_directories,
            databases: images.files.len(),
            database_bytes: images.bytes() as u64,
        };
        let intent = canonical_json(&DatabaseIntent {
            format: FORMAT,
            operation: transaction.intent.plan.operation_id,
            parent: PhysicalBinding::of(&transaction.parent)?,
            report: &report,
            inventory: &inventory,
        })?;
        if intent.len() > MAX_PLAN {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "foreign database intent exceeds 2 MiB",
            ));
        }
        transaction.check_names()?;
        Ok(Self {
            images,
            inventory,
            report,
            intent,
        })
    }
    fn binding(&self, root: &DiskRoot) -> Result<Vec<u8>> {
        Ok(canonical_json(&Binding {
            identity: PhysicalBinding::of(root)?,
            intent_digest: blake3::hash(&self.intent).to_hex().to_string(),
        })?)
    }
    fn check_binding(&self, root: &DiskRoot) -> Result<()> {
        if root.read_bytes(&root.path().join("binding.json"), MAX_PLAN)? != self.binding(root)? {
            return Err(changed(
                "foreign database directory differs from its original physical binding",
            ));
        }
        Ok(())
    }
    fn status(&self, transaction: &ForeignMigrationTransaction) -> Result<ForeignDatabaseStatus> {
        let parent = &transaction.parent;
        let path = parent.path().join(DIRECTORY);
        let exists = parent.optional_metadata(&path)?.is_some();
        let completion = parent
            .open_optional_read_regular(&parent_completion(transaction))?
            .map(|file| read_file_bytes(file, MAX_PLAN))
            .transpose()?;
        let Some(file) = parent.open_optional_read_regular(&reservation(transaction))? else {
            if exists || completion.is_some() {
                return Err(changed(
                    "foreign database directory or completion lacks its original reservation",
                ));
            }
            return Ok(ForeignDatabaseStatus::NotStarted);
        };
        if read_file_bytes(file, MAX_PLAN)? != self.intent {
            return Err(changed(
                "foreign database reservation differs from the complete original State and scopes",
            ));
        }
        if !exists {
            if completion.is_some() {
                return Err(changed(
                    "completed original database root is missing; it was not recreated",
                ));
            }
            return Ok(ForeignDatabaseStatus::Preparing);
        }
        let root = parent.private_child(&path)?;
        if let Some(bytes) = &completion {
            if bytes != &self.binding(&root)? {
                return Err(changed(
                    "original database parent completion binds a different physical root",
                ));
            }
            self.verify(&root)?;
            transaction.check_names()?;
            return Ok(ForeignDatabaseStatus::Verified {
                report: Box::new(self.report.clone()),
            });
        }
        if root
            .optional_metadata(&path.join("binding.json"))?
            .is_none()
        {
            unbound(&root)?;
            return Ok(ForeignDatabaseStatus::Preparing);
        }
        self.check_binding(&root)?;
        top_level(&root)?;
        if root
            .optional_metadata(&path.join("complete.json"))?
            .is_none()
        {
            return Ok(ForeignDatabaseStatus::Preparing);
        }
        self.verify(&root)?;
        transaction.check_names()?;
        Ok(ForeignDatabaseStatus::Preparing)
    }
    fn complete(&self, transaction: &ForeignMigrationTransaction, root: &DiskRoot) -> Result<()> {
        self.verify(root)?;
        self.sync(root)?;
        ensure_exact(
            &transaction.parent,
            &parent_completion(transaction),
            &self.binding(root)?,
        )?;
        transaction.parent.sync_dir(transaction.parent.path())?;
        transaction.check_names()
    }
    fn verify(&self, root: &DiskRoot) -> Result<()> {
        self.check_binding(root)?;
        top_level(root)?;
        if root.read_bytes(&root.path().join("intent.json"), MAX_PLAN)? != self.intent
            || root.read_bytes(&root.path().join("complete.json"), MAX_PLAN)?
                != self.binding(root)?
        {
            return Err(changed(
                "foreign database completion differs from original native evidence",
            ));
        }
        let data = root.private_child(&root.path().join("databases"))?;
        self.inventory.verify(&data, &budget())?;
        self.check_binding(root)
    }
    fn sync(&self, root: &DiskRoot) -> Result<()> {
        self.inventory
            .sync(&root.private_child(&root.path().join("databases"))?)?;
        for name in ["binding.json", "intent.json", "complete.json"] {
            root.open_regular(&root.path().join(name))?.sync_all()?;
        }
        root.sync_dir(root.path())
    }
}
fn unbound(root: &DiskRoot) -> Result<()> {
    for (index, name) in root.entries(root.path())?.enumerate() {
        if index >= 64 {
            return Err(invalid(
                "unbound foreign database directory exceeds its entry bound",
            ));
        }
        let name = name?;
        if !platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
            return Err(changed(
                "unbound foreign database directory contains unrelated material",
            ));
        }
        root.open_read_regular(&root.path().join(name.as_path()))?;
    }
    Ok(())
}
fn top_level(root: &DiskRoot) -> Result<()> {
    for (index, name) in root.entries(root.path())?.enumerate() {
        if index >= 64 {
            return Err(invalid(
                "foreign database directory exceeds its entry bound",
            ));
        }
        let name = name?;
        match name.as_path().to_str() {
            Some("databases" | "residue") => {
                root.directory_identity(&root.path().join(name.as_path()))?;
            }
            Some("binding.json" | "intent.json" | "complete.json") => {
                root.open_read_regular(&root.path().join(name.as_path()))?;
            }
            _ if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) => {
                root.open_read_regular(&root.path().join(name.as_path()))?;
            }
            _ => return Err(changed("foreign database root contains unrelated material")),
        }
    }
    Ok(())
}
