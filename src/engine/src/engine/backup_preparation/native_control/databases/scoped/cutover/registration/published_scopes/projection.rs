//! Real SQLite encoders fed by each target's own authenticated native history.
use super::*;
use crate::engine::disk_catalog::PreparedDiskScopes;
use crate::engine::index::preparation::SystemEncodingInput;
use super::encoding_memo;
use crate::engine::backup_preparation::native_control::archive;
use crate::engine::backup_preparation::native_control::databases::materials::FileReference;
use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo::{
    Transaction,
};

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
struct Descriptor<'a> {
    scope: &'a Scope,
    bytes: usize,
    digest: &'a str,
}

/// A digest of these immutable expected SQLite bytes, not a read of any
/// installed file. This owner never hands out mutable access to the image.
pub(in super::super) struct ScopeFile {
    scope: Scope,
    data: Vec<u8>,
    reference: FileReference,
}

impl ScopeFile {
    fn new(scope: Scope, data: Vec<u8>) -> Self {
        let reference = FileReference::from_bytes(&data);
        Self { scope, data, reference }
    }

    pub(in super::super) fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub(in super::super) fn reference(&self) -> &FileReference {
        &self.reference
    }

    fn descriptor(&self) -> Descriptor<'_> {
        Descriptor {
            scope: &self.scope,
            bytes: self.reference.bytes(),
            digest: self.reference.digest(),
        }
    }
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
    databases: &'a [Descriptor<'a>],
}

pub(in super::super) struct Scopes {
    addresses: BTreeMap<PathBuf, usize>,
    // The manifest's insertion order is part of its canonical bytes.
    files: Vec<ScopeFile>,
    own_identity: DiskRegistrationIdentity,
    pub(super) bytes: usize,
    pub(super) disks: usize,
    pub(super) manifest: Vec<u8>,
}
impl Scopes {
    fn for_target(identity: &DiskRegistrationIdentity) -> Self {
        Self {
            addresses: BTreeMap::new(),
            files: Vec::new(),
            own_identity: identity.clone(),
            bytes: 0,
            disks: 0,
            manifest: Vec::new(),
        }
    }

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
        let unbound = |_: &AtomicBool| Ok(None);
        let mut canonical = Transaction::new(
            prepared.scope_encoding.as_ref(), &unbound,
        );
        let (control, source) =
            archive::read_observed_with_encodings(
                root,
                job,
                &task.objects.native,
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
                &mut canonical,
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
        let mut scopes = Self::for_target(own);
        // Every call above still verifies this target's registry, archive and
        // authenticated disk history. The shared memo stores only pure encodings.
        let configuration = registry.configuration();
        let publication = &configuration.publication.publication;
        let memo_context = if prepared.scope_encoding.is_some() {
            encoding_memo::Context {
                target: own,
                device: job.target.device,
                inode: job.target.inode,
                operation: job.request.operation_id,
                source_fence: prepared.source_fence_hash(),
                archive: &task.objects.native.archive_hash,
                candidate: registry.candidate().hash(),
                decision: publication.decision_hash.as_str(),
                publication: &publication.publication_hash,
                authority: &publication.authority_hash,
                snapshot: &publication.snapshot_hash,
                first_sequence: publication.first_sequence,
                registry: &configuration.registry,
                configuration: &configuration.config_hash,
            }.digest(stop)?
        } else {
            None
        };
        let mut encodings = encoding_memo::Session::new(
            prepared.scope_encoding.as_ref().zip(memo_context),
        );
        let checkpoint = || preparation_checkpoint(stop);
        let input = SystemEncodingInput::new(&published, &checkpoint)?;
        let system = encodings.system(input.input_hash(), || input.encode(&checkpoint))?;
        input.verify(&system, &checkpoint)?;
        scopes.add(
            Scope::SystemCurrent {
                pool_id: job.pool_id.clone(),
            },
            system,
        )?;
        disks.finish_with_encoder(&published, stop, |current, history| {
            let input = if encodings.enabled() {
                // These complete snapshots include identity, header and every
                // historical change, not just the final publication State.
                encoding_memo::digest(&(current, history), stop)?
            } else {
                None
            };
            encodings.disk_pair(input, || PreparedDiskScopes::encode_pair(current, history))
        }, |identity, current, history| {
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
        let descriptors: Vec<_> = scopes.files.iter().map(ScopeFile::descriptor).collect();
        scopes.manifest = canonical_json(&Manifest {
            format: "zxs-backup-published-scoped-databases",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: publication.decision_hash.as_str(),
            publication_hash: &publication.publication_hash,
            snapshot_hash: &publication.snapshot_hash,
            registry: &configuration.registry,
            target: own,
            databases: &descriptors,
        })?;
        if scopes.manifest.len() > MAX_COMPLETION {
            return Err(budget());
        }
        named_target(root, job)?;
        preparation_checkpoint(stop)?;
        encodings.commit();
        canonical.commit();
        Ok(scopes)
    }
    fn add(&mut self, scope: Scope, bytes: Vec<u8>) -> Result<()> {
        let total = self.bytes.checked_add(bytes.len()).ok_or_else(budget)?;
        if total > MAX_BYTES || self.files.len() == MAX_DATABASES {
            return Err(budget());
        }
        let path = scope.path();
        match self.addresses.entry(path) {
            std::collections::btree_map::Entry::Vacant(address) => {
                address.insert(self.files.len());
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(invalid("published scopes repeat an identity"));
            }
        }
        self.files.push(ScopeFile::new(scope, bytes));
        self.bytes = total;
        Ok(())
    }
    pub(super) fn prepared(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::PublishedScopes,
            self.manifest.as_slice(),
            self.addresses
                .iter()
                .map(|(path, &index)| (path.clone(), self.files[index].bytes())),
        )
    }
    pub(in super::super) fn system(&self) -> Result<&ScopeFile> {
        self.files
            .first()
            .filter(|file| match &file.scope {
                Scope::SystemCurrent { pool_id } => pool_id == &self.own_identity.pool_id,
                Scope::DiskCurrent { .. } | Scope::DiskHistory { .. } => false,
            })
            .ok_or_else(|| invalid("published scopes omit the expected SYSTEM database"))
    }

    pub(in super::super) fn disk(
        &self,
        identity: &DiskRegistrationIdentity,
    ) -> Result<[(&'static str, &ScopeFile); 2]> {
        let lookup = |name: &str| {
            self.addresses
                .get(&disk_path(identity, name))
                .map(|&index| &self.files[index])
        };
        let current_name = crate::database_install::DISK_DATABASE_NAME;
        let history_name = crate::database_install::DISK_HISTORY_DATABASE_NAME;
        let current = lookup(current_name)
            .filter(|file| {
                matches!(&file.scope, Scope::DiskCurrent { identity: actual } if actual == identity)
            })
            .ok_or_else(|| invalid("published scopes omit the exact disk current database"))?;
        let history = lookup(history_name)
            .filter(|file| {
                matches!(&file.scope, Scope::DiskHistory { identity: actual } if actual == identity)
            })
            .ok_or_else(|| invalid("published scopes omit the exact disk history database"))?;
        Ok([(current_name, current), (history_name, history)])
    }

    pub(super) fn own(&self) -> Result<[(&'static str, &ScopeFile); 2]> {
        self.disk(&self.own_identity)
    }

    pub(super) fn report(&self) -> Result<BackupPublishedScopeTargetReport> {
        let [(_, current), (_, history)] = self.own()?;
        Ok(BackupPublishedScopeTargetReport {
            target: self.own_identity.clone(),
            scope_manifest_hash: blake3::hash(&self.manifest).to_hex().to_string(),
            system_bytes: self.system()?.reference.bytes() as u64,
            configured_disks: self.disks,
            databases: self.files.len(),
            scoped_bytes: self.bytes as u64,
            disk_current_hash: current.reference.digest().to_owned(),
            disk_history_hash: history.reference.digest().to_owned(),
        })
    }
}
pub(super) fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "published scopes exceed 81 SQLite databases / 128 MiB or their manifest bound",
    )
}

#[cfg(test)]
#[path =
    "../../../../../../../../../../../test/engine/unit/backup_published_encoding_probe.rs"
]
pub(in super::super) mod encoding_probe;

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_scope_file_refs.rs"]
mod file_reference_tests;
