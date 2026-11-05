//! Real tier-3 source and registered primary, owned by one private TempDir.
use super::*;
use std::{
    collections::BTreeMap,
    os::unix::fs::{DirBuilderExt, MetadataExt},
};
use storage_engine::{
    CommitRequest, Config, Engine, NamespaceOperation, NamespaceOperationKind, OperationId,
    Repository, RepositoryName,
};

#[cfg(test)]
use storage_engine::{ForeignImportLimits, ForeignImportPreview};

pub(super) const PRINCIPAL: &str = "smoke-admin";
pub(super) const PARENT: &str = "Reviewed";

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Entry {
    device: u64,
    inode: u64,
    mode: u32,
    body: Option<Vec<u8>>,
}
pub(super) type Snapshot = BTreeMap<PathBuf, Entry>;

/// Include directory entries and inode identity: unchanged file bytes alone
/// would miss a created empty directory or replacement of a foreign object.
pub(super) fn snapshot(root: &Path) -> Result<Snapshot> {
    fn collect(root: &Path, path: &Path, out: &mut Snapshot) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        let body = if metadata.is_dir() {
            None
        } else if metadata.is_file() {
            Some(fs::read(path)?)
        } else {
            return Err(format!(
                "unexpected non-regular private fixture entry: {}",
                path.display()
            )
            .into());
        };
        out.insert(
            path.strip_prefix(root)?.into(),
            Entry {
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
                body,
            },
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                collect(root, &entry?.path(), out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    collect(root, root, &mut out)?;
    Ok(out)
}

pub(super) struct Fixture {
    temporary: tempfile::TempDir,
    pub(super) pool: PathBuf,
    pub(super) discovery: PathBuf,
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
    pub(super) repository: Repository,
    source_before: Snapshot,
}
fn commit(engine: &mut Engine, workspace: storage_engine::Workspace) -> Result<()> {
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate()?,
        principal: PRINCIPAL.into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Disposable real foreign-review browser fixture".into(),
        request_digest: None,
    })?;
    Ok(())
}
fn save(engine: &mut Engine, name: &str, slot: &str, copies: u32) -> Result<()> {
    let repository = engine.create_repository(
        &RepositoryName::parse(name)?,
        slot,
        copies,
        PRINCIPAL,
        OperationId::generate()?,
    )?;
    let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
    engine.stage_bytes(&workspace.workspace_id, "document.txt", name.as_bytes())?;
    commit(engine, workspace)
}
impl Fixture {
    pub(super) fn new(parent: &Path, source_pool: &Path) -> Result<Self> {
        let temporary = tempfile::Builder::new()
            .prefix("foreign-review-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        let root = temporary.path();
        let discovery = root.join("disks");
        fs::DirBuilder::new().mode(0o700).create(&discovery)?;
        let pool = root.join("pool");
        let target = discovery.join("known");
        let mut engine = Engine::initialize(&pool, Config::development([target.clone()])?)?;
        let repository = engine.create_disk_root("P01", PRINCIPAL, OperationId::generate()?)?;
        let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
        engine.namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, PARENT, None)?,
        )?;
        engine.stage_bytes(
            &workspace.workspace_id,
            "untouched.txt",
            b"existing target data\n",
        )?;
        commit(&mut engine, workspace)?;
        drop(engine);

        // One actual tertiary replica sees two primary origins. Only the first
        // history has a local body here; the second must remain as unavailable.
        let source = discovery.join("foreign");
        let foreign_pool = root.join("foreign-system");
        let mut config = Config::development([
            root.join("origin-one"),
            root.join("backup"),
            source.clone(),
            root.join("origin-two"),
        ])?;
        config.disks[3].slot_id = "P02".into();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        let mut foreign = Engine::initialize(&foreign_pool, config)?;
        save(&mut foreign, "first-origin", "P01", 3)?;
        save(&mut foreign, "second-origin", "P02", 1)?;
        drop(foreign);
        drop(Engine::open(&foreign_pool)?);
        drop(Engine::open(&pool)?);

        // This source account belongs to the enclosing smoke's private fixture.
        // Copy management authority, never the pool recovery auth snapshot.
        let management = pool.join("management");
        fs::DirBuilder::new().mode(0o700).create(&management)?;
        let authority = fs::read(source_pool.join("management/management-auth.json"))?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(management.join("management-auth.json"))?;
        output.write_all(&authority)?;
        output.sync_all()?;
        File::open(&management)?.sync_all()?;
        storage_server::management::claim_authority(&pool)?;
        let source_before = snapshot(&source)?;
        Ok(Self {
            temporary,
            pool,
            discovery,
            source,
            target,
            repository,
            source_before,
        })
    }
    pub(super) fn root(&self) -> &Path {
        self.temporary.path()
    }
    #[cfg(test)]
    pub(super) fn inspect(&self) -> Result<ForeignImportPreview> {
        Ok(ForeignImportPreview::inspect(
            File::open(&self.source)?,
            self.source.clone(),
            ForeignImportLimits::default(),
        )?)
    }
    pub(super) fn source_unchanged(&self) -> Result<()> {
        if snapshot(&self.source)? != self.source_before {
            return Err("foreign-review smoke changed its original source".into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn names(&self, preview: &ForeignImportPreview) -> BTreeMap<String, String> {
        preview
            .histories()
            .iter()
            .enumerate()
            .map(|(index, history)| {
                (
                    history.group_id().to_owned(),
                    format!("origin-{}", index + 1),
                )
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../../unit/foreign_review_fixture_tests.rs"]
mod tests;
