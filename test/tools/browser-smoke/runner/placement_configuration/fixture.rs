//! Isolated five-directory pool. Never read or mutate a supplied user pool.
use super::*;
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt};
use storage_engine::{CommitRequest, Config, DiskConfig, Engine, OperationId, Repository};

pub(super) const PRINCIPAL: &str = "smoke-admin";
pub(super) const REPOSITORY: &str = "placement-e2e";
const FUTURE: &str = "future/note.txt";
const PROTECTED: &str = "protected/note.txt";
const NEW_BODY: &[u8] = b"version created after the real browser placement update\n";

struct SavedVersion {
    id: String,
    record: Value,
    placement: Value,
    body: Vec<u8>,
}
pub(super) struct Fixture {
    temporary: tempfile::TempDir,
    pub(super) pool: PathBuf,
    pub(super) repository: Repository,
    pub(super) future_directory: String,
    pub(super) protected_directory: String,
    pub(super) future_file: String,
    pub(super) target_root: String,
    originals: Vec<SavedVersion>,
    policies: Vec<u8>,
}
fn save(engine: &mut Engine, repository: &Repository, path: &str, bytes: &[u8]) -> Result<()> {
    let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
    engine.stage_bytes(&workspace.workspace_id, path, bytes)?;
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate()?,
        principal: PRINCIPAL.into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Isolated real browser placement fixture".into(),
        request_digest: None,
    })?;
    Ok(())
}
impl Fixture {
    pub(super) fn new(parent: &Path, source_pool: &Path) -> Result<Self> {
        let temporary = tempfile::Builder::new()
            .prefix("placement-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        let root = temporary.path();
        let pool = root.join("pool");
        let mut config =
            Config::development([root.join("P01"), root.join("B01"), root.join("C01")])?;
        for (slot, level, upstream) in [("P02", 1, None), ("B02", 2, Some("P02"))] {
            config.disks.push(DiskConfig {
                slot_id: slot.into(),
                disk_id: storage_engine::new_id("disk")?,
                root: root.join(slot),
                level,
                upstream_slot_id: upstream.map(str::to_owned),
                metadata_member: false,
                binding_generation: 1,
                capacity_bytes: None,
            });
        }
        let mut engine = Engine::initialize(&pool, config)?;
        let repository = engine.create_repository(
            &storage_engine::RepositoryName::parse(REPOSITORY)?,
            "P01",
            1,
            PRINCIPAL,
            OperationId::generate()?,
        )?;
        save(
            &mut engine,
            &repository,
            FUTURE,
            b"first preserved version\n",
        )?;
        save(
            &mut engine,
            &repository,
            FUTURE,
            b"second preserved version\n",
        )?;
        save(&mut engine, &repository, PROTECTED, b"protected original\n")?;
        engine.configure_backups(storage_engine::BackupConfigurationRequest {
            repository_id: &repository.repository_id,
            path: "protected",
            target_copies: 2,
            backup_prefixes: &BTreeMap::from([("B01".into(), "protected-backup".into())]),
            expected_generation: 0,
            principal: PRINCIPAL,
            operation_id: OperationId::generate()?,
        })?;
        let entries = engine.list_tree(&repository.main_branch_id)?;
        let entry = |path: &str| {
            entries
                .iter()
                .find(|entry| entry.path() == path)
                .ok_or("placement fixture entry missing")
        };
        let future_directory = entry("future")?.entry_id.clone();
        let protected_directory = entry("protected")?.entry_id.clone();
        let future_file = entry(FUTURE)?.file_id.clone();
        let mut originals = Vec::new();
        for version in engine.state().versions.values() {
            let mut content = engine.open_content(&version.version_id)?;
            let mut body = Vec::new();
            content.file.read_to_end(&mut body)?;
            originals.push(SavedVersion {
                id: version.version_id.clone(),
                record: json_codec::to_value(version)?,
                placement: json_codec::to_value(&engine.state().placements[&version.version_id])?,
                body,
            });
        }
        let policies = storage_engine::canonical_json(&engine.state().protection_policies)?;
        drop(engine);
        // Copy only the independent authority of the existing test-owned daemon,
        // never install a pool disaster-recovery snapshot as login authority.
        let management = pool.join("management");
        fs::DirBuilder::new().mode(0o700).create(&management)?;
        let authority = fs::read(source_pool.join("management/management-auth.json"))?;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(management.join("management-auth.json"))?
            .write_all(&authority)?;
        storage_server::management::claim_authority(&pool)?;
        let target_root = root
            .join("P02")
            .to_str()
            .ok_or("non-UTF8 fixture root")?
            .to_owned();
        let fixture = Self {
            temporary,
            pool,
            repository,
            future_directory,
            protected_directory,
            future_file,
            target_root,
            originals,
            policies,
        };
        let engine = Engine::open(&fixture.pool)?;
        fixture.preserved(&engine)?;
        Ok(fixture)
    }
    pub(super) fn root(&self) -> &Path {
        self.temporary.path()
    }
    pub(super) fn endpoint(&self) -> String {
        format!(
            "/api/repositories/{}/placement-configuration",
            self.repository.repository_id
        )
    }
    pub(super) fn originals(&self) -> Vec<String> {
        self.originals
            .iter()
            .filter(|entry| entry.record["file_id"] == self.future_file)
            .map(|entry| entry.id.clone())
            .collect()
    }
    fn preserved(&self, engine: &Engine) -> Result<()> {
        if storage_engine::canonical_json(&engine.state().protection_policies)? != self.policies {
            return Err("browser placement modified independent backup policies".into());
        }
        for original in &self.originals {
            if json_codec::to_value(
                engine
                    .state()
                    .versions
                    .get(&original.id)
                    .ok_or("old version disappeared")?,
            )? != original.record
                || json_codec::to_value(
                    engine
                        .state()
                        .placements
                        .get(&original.id)
                        .ok_or("old placement disappeared")?,
                )? != original.placement
            {
                return Err(
                    "browser placement changed an existing immutable version or its placement"
                        .into(),
                );
            }
            let mut content = engine.open_content(&original.id)?;
            let mut bytes = Vec::new();
            content.file.read_to_end(&mut bytes)?;
            if bytes != original.body {
                return Err("an old version's verified content changed".into());
            }
        }
        Ok(())
    }
    pub(super) fn publish_future_version(&self) -> Result<String> {
        let mut engine = wait_for(
            "placement fixture releases the stopped daemon's locks",
            || Ok(Engine::open(&self.pool)?),
        )?;
        self.preserved(&engine)?;
        let placement = engine.placement_configuration(&self.repository.repository_id, "future")?;
        if placement.primary_slot_id != "P02"
            || placement.generation != 2
            || placement.scope.id != self.future_directory
        {
            return Err("real browser save did not persist the expected scoped P02 rule".into());
        }
        let protected =
            engine.placement_configuration(&self.repository.repository_id, "protected")?;
        if protected.primary_slot_id != "P01" || protected.rule_id.is_some() {
            return Err("rejected protected-directory switch changed its rule".into());
        }
        if engine.state().versions.len() != self.originals.len() {
            return Err("placement UI unexpectedly created file content".into());
        }
        save(&mut engine, &self.repository, FUTURE, NEW_BODY)?;
        self.preserved(&engine)?;
        let current = engine
            .list_tree(&self.repository.main_branch_id)?
            .into_iter()
            .find(|entry| entry.path() == FUTURE)
            .ok_or("future file missing")?;
        let placement = &engine.state().placements[&current.version_id];
        if placement.primary_slot_id != "P02"
            || placement.route_slot_ids != ["P02"]
            || engine.file_history(&self.future_file)?.len() != 3
        {
            return Err("post-change content did not use only the newly selected primary".into());
        }
        let mut content = engine.open_content(&current.version_id)?;
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes)?;
        if bytes != NEW_BODY {
            return Err("post-change content bytes differ".into());
        }
        Ok(current.version_id)
    }
}
