//! Real SQLite encoders fed by each target's own authenticated native history.
use super::*;
use crate::engine::disk_catalog::PreparedDiskScopes;

const MAX_DATABASES: usize = 81; // SYSTEM plus current/history for 32 + 8 bindings.
pub(super) const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "kebab-case")]
enum Scope {
    SystemCurrent { pool_id: String },
    DiskCurrent { identity: DiskRegistrationIdentity },
    DiskHistory { identity: DiskRegistrationIdentity },
}
impl Scope {
    fn path(&self) -> PathBuf {
        match self {
            Self::SystemCurrent { .. } => PathBuf::from("system/index.sqlite"),
            Self::DiskCurrent { identity } => {
                disk_path(identity, crate::database_install::DISK_DATABASE_NAME)
            }
            Self::DiskHistory { identity } => disk_path(
                identity,
                crate::database_install::DISK_HISTORY_DATABASE_NAME,
            ),
        }
    }
}
fn disk_path(identity: &DiskRegistrationIdentity, name: &str) -> PathBuf {
    PathBuf::from("disks").join(&identity.disk_id).join(name)
}

#[derive(Serialize)]
struct Descriptor {
    scope: Scope,
    bytes: usize,
    digest: String,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    snapshot_hash: &'a str,
    registry: &'a DiskRegistryCommitment,
    target: &'a DiskRegistrationIdentity,
    databases: &'a [Descriptor],
}

pub(in super::super) struct Scopes {
    files: BTreeMap<PathBuf, Vec<u8>>,
    descriptors: Vec<Descriptor>,
    own_current: PathBuf,
    own_history: PathBuf,
    pub(super) bytes: usize,
    pub(super) disks: usize,
    pub(super) system_bytes: usize,
    pub(super) manifest: Vec<u8>,
}
impl Scopes {
    pub(in super::super) fn capture(
        prepared: &PreparedBackupRegistry,
        registry: &published_registry::Plan,
        index: usize,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let root = &prepared.targets[index].root;
        let job = &prepared.task.selection().jobs()[index];
        let own = &prepared.projection.targets[index].node.identity;
        named_target(root, job)?;
        if registry
            .verify(&prepared.targets[index], stop)?
            .registration(own)?
            != Some(crate::RegistrationMode::Enabled)
        {
            return Err(invalid(
                "published database target is not in its exact registry",
            ));
        }
        let published = registry.state_at(&prepared.targets[index], stop)?;
        let task = &prepared.task.installation.evidence.task.scoped;
        let mut disks = None;
        let (control, source) =
            crate::engine::backup_preparation::native_control::archive::read_observed(
                root,
                job,
                &task.objects.native.archive_hash,
                stop,
                &mut |config, candidate, hash, state| {
                    if disks.is_none() {
                        disks = Some(PreparedDiskScopes::new(config)?);
                    }
                    disks
                        .as_mut()
                        .expect("invariant: admitted prefix initialized disk scopes")
                        .observe(candidate, hash, state, stop)
                },
            )?;
        if control.state_hash != prepared.source_evidence().state_hash()
            || registry
                .candidate()
                .candidate()
                .previous_snapshot
                .as_deref()
                != Some(control.snapshot_hash.as_str())
        {
            return Err(invalid(
                "published scopes do not extend this target's exact frozen State",
            ));
        }
        let mut disks = disks
            .ok_or_else(|| invalid("published scopes have no authenticated genesis prefix"))?;
        disks.extend_backup(
            registry.candidate(),
            registry.effective_config(),
            &source,
            &published,
            stop,
        )?;
        let mut scopes = Self {
            files: BTreeMap::new(),
            descriptors: Vec::new(),
            own_current: disk_path(own, crate::database_install::DISK_DATABASE_NAME),
            own_history: disk_path(own, crate::database_install::DISK_HISTORY_DATABASE_NAME),
            bytes: 0,
            disks: 0,
            system_bytes: 0,
            manifest: Vec::new(),
        };
        let system = crate::engine::index::preparation::encode(&published, stop)?;
        scopes.system_bytes = system.len();
        scopes.add(
            Scope::SystemCurrent {
                pool_id: job.pool_id.clone(),
            },
            system,
        )?;
        disks.finish(&published, stop, |identity, current, history| {
            scopes.add(
                Scope::DiskCurrent {
                    identity: identity.clone(),
                },
                current,
            )?;
            scopes.add(Scope::DiskHistory { identity }, history)?;
            scopes.disks += 1;
            Ok(())
        })?;
        let configuration = registry.configuration();
        let publication = &configuration.publication.publication;
        scopes.manifest = canonical_json(&Manifest {
            format: "zxs-backup-published-scoped-databases",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: publication.decision_hash.as_str(),
            publication_hash: &publication.publication_hash,
            snapshot_hash: &publication.snapshot_hash,
            registry: &configuration.registry,
            target: own,
            databases: &scopes.descriptors,
        })?;
        if scopes.manifest.len() > MAX_COMPLETION {
            return Err(budget());
        }
        named_target(root, job)?;
        Ok(scopes)
    }
    fn add(&mut self, scope: Scope, bytes: Vec<u8>) -> Result<()> {
        let total = self.bytes.checked_add(bytes.len()).ok_or_else(budget)?;
        if total > MAX_BYTES || self.files.len() == MAX_DATABASES {
            return Err(budget());
        }
        let path = scope.path();
        let descriptor = Descriptor {
            scope,
            bytes: bytes.len(),
            digest: blake3::hash(&bytes).to_hex().to_string(),
        };
        if self.files.insert(path, bytes).is_some() {
            return Err(invalid("published database scopes repeat an identity"));
        }
        self.bytes = total;
        self.descriptors.push(descriptor);
        Ok(())
    }
    pub(super) fn prepared(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::PublishedScopes,
            self.manifest.as_slice(),
            self.files
                .iter()
                .map(|(path, bytes)| (path.clone(), bytes.as_slice())),
        )
    }
    pub(in super::super) fn system(&self) -> &[u8] {
        &self.files[&PathBuf::from("system/index.sqlite")]
    }
    pub(in super::super) fn disk(
        &self,
        identity: &DiskRegistrationIdentity,
    ) -> Result<[(&'static str, &[u8]); 2]> {
        let current = self
            .files
            .get(&disk_path(
                identity,
                crate::database_install::DISK_DATABASE_NAME,
            ))
            .ok_or_else(|| invalid("published scopes omit an original disk current database"))?;
        let history = self
            .files
            .get(&disk_path(
                identity,
                crate::database_install::DISK_HISTORY_DATABASE_NAME,
            ))
            .ok_or_else(|| invalid("published scopes omit an original disk history database"))?;
        Ok([
            (crate::database_install::DISK_DATABASE_NAME, current),
            (crate::database_install::DISK_HISTORY_DATABASE_NAME, history),
        ])
    }
    pub(super) fn own(&self) -> [(&'static str, &[u8]); 2] {
        [
            (
                crate::database_install::DISK_DATABASE_NAME,
                &self.files[&self.own_current],
            ),
            (
                crate::database_install::DISK_HISTORY_DATABASE_NAME,
                &self.files[&self.own_history],
            ),
        ]
    }
    pub(super) fn report(
        &self,
        identity: &DiskRegistrationIdentity,
    ) -> BackupPublishedScopeTargetReport {
        let [(_, current), (_, history)] = self.own();
        BackupPublishedScopeTargetReport {
            target: identity.clone(),
            scope_manifest_hash: blake3::hash(&self.manifest).to_hex().to_string(),
            system_bytes: self.system_bytes as u64,
            configured_disks: self.disks,
            databases: self.files.len(),
            scoped_bytes: self.bytes as u64,
            disk_current_hash: blake3::hash(current).to_hex().to_string(),
            disk_history_hash: blake3::hash(history).to_hex().to_string(),
        }
    }
}
pub(super) fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "published scopes exceed 81 SQLite databases / 128 MiB or their manifest bound",
    )
}
