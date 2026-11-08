//! Identical immutable envelopes on held system and metadata-member roots.
use super::*;
use platform_io::fs_safety::linux::FileIdentity;
use std::{fs::File, os::unix::fs::FileExt, sync::Arc};

const DIRECTORY: &str = "file-history-move-recovery";
const NAME: &str = "preparation.json";

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    record: RecoveryRecord,
    digest: String,
}

enum Owner {
    System {
        root: DiskRoot,
        writer_identity: FileIdentity,
    },
    Member {
        config: ValidatedConfig,
        disk: DiskConfig,
        handle: Arc<DiskHandle>,
    },
}

pub(super) struct Store {
    owner: Owner,
    root_identity: FileIdentity,
}

pub(super) struct Observed {
    pub(super) record: RecoveryRecord,
    bytes: Vec<u8>,
    file: ReadFile,
    directory: File,
}

impl Store {
    pub(super) fn system(engine: &Engine) -> Result<Self> {
        let root = DiskRoot::open(&engine.root)?;
        root.private_directory(root.path())?;
        let store = Self {
            root_identity: root.directory_identity(root.path())?,
            owner: Owner::System {
                root,
                writer_identity: engine._locks.system_identity()?,
            },
        };
        store.check_owner()?;
        Ok(store)
    }

    pub(super) fn member(engine: &Engine, identity: &DiskRegistrationIdentity) -> Result<Self> {
        let disk = engine.disk(&identity.slot_id)?;
        if identity.pool_id != engine.config.pool_id
            || identity.disk_id != disk.disk_id
            || identity.binding_generation != disk.binding_generation
            || !disk.metadata_member
        {
            return Err(stale("move recovery member differs from its frozen binding"));
        }
        let handle = Arc::clone(engine.disk_resources.get(disk)?);
        let store = Self {
            root_identity: handle.root.directory_identity(handle.root.path())?,
            owner: Owner::Member {
                config: engine.config.clone(),
                disk: disk.clone(),
                handle,
            },
        };
        store.check_owner()?;
        Ok(store)
    }

    fn root(&self) -> &DiskRoot {
        match &self.owner {
            Owner::System { root, .. } => root,
            Owner::Member { handle, .. } => &handle.root,
        }
    }

    fn check_owner(&self) -> Result<()> {
        let root = self.root();
        let named = DiskRoot::open(root.path())?;
        if named.directory_identity(named.path())? != self.root_identity {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "move recovery root identity changed",
            ));
        }
        match &self.owner {
            Owner::System { writer_identity, .. } => {
                let writer = named.open_regular(&named.path().join("writer.lock"))?;
                if FileIdentity::from_fd(&writer)? != *writer_identity {
                    return Err(Error::new(
                        ErrorCode::DiskIdentityMismatch,
                        "move recovery system writer identity changed",
                    ));
                }
            }
            Owner::Member { config, disk, handle } => {
                let lease = handle.root.admit_io()?;
                let seal = validate_seal_observed(config, disk, &mut |_| lease.check_running())?;
                validate_stored_seal(&handle.seal, &seal)?;
                lease.check_running()?;
            }
        }
        Ok(())
    }

    pub(super) fn read(&self) -> Result<Option<Observed>> {
        self.check_owner()?;
        let path = self.root().path().join(DIRECTORY);
        if self.root().optional_metadata(&path)?.is_none() {
            return Ok(None);
        }
        self.root().private_directory(&path)?;
        let directory = self.root().directory(&path)?;
        let file = self
            .root()
            .open_optional_read_regular(&path.join(NAME))?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "move recovery envelope directory is incomplete; retain it and inspect the original preparation",
                )
            })?;
        let bytes = read_file_bytes(file.try_clone()?, MAX_BYTES)?;
        let envelope: Envelope = decode_strict(&bytes)?;
        envelope.record.validate()?;
        if envelope.record.digest()?.to_string() != envelope.digest {
            return Err(corrupt("move recovery envelope differs from its frozen record digest"));
        }
        let observed = Observed {
            record: envelope.record,
            bytes,
            file,
            directory,
        };
        observed.check_names(self)?;
        Ok(Some(observed))
    }

    pub(super) fn save(&self, record: &RecoveryRecord, reserve: u64) -> Result<()> {
        if let Some(existing) = self.read()? {
            if existing.record.operation_id != record.operation_id {
                return Err(Error::new(
                    ErrorCode::WorkspaceBusy,
                    "another move recovery envelope occupies the retained slot; it was not replaced",
                ));
            }
            if existing.record != *record {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "the original move recovery envelope retains a different request, actor or source baseline",
                ));
            }
            return existing.synchronize(self);
        }
        let bytes = encode_metadata(
            &Envelope {
                record: record.clone(),
                digest: record.digest()?.to_string(),
            },
            MAX_BYTES,
        )?;
        let required = reserve
            .checked_add(bytes.len() as u64 + 4096)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "move recovery envelope metadata reserve overflow",
                )
            })?;
        if self.root().available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "move recovery envelope would consume the root metadata reserve",
            ));
        }
        self.check_owner()?;
        let create = || -> Result<()> {
            // An existing incomplete directory is never adopted or overwritten.
            // The first durable directory also bounds interrupted temporary files.
            self.root()
                .create_private_directory_synced(&self.root().path().join(DIRECTORY))?;
            self.root().write_atomic(
                &self.root().path().join(DIRECTORY).join(NAME),
                &bytes,
                false,
            )?;
            let observed = self.read()?.ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "new move recovery envelope disappeared before its durability barrier",
                )
            })?;
            if observed.record != *record {
                return Err(corrupt("new move recovery envelope differs from the captured source plan"));
            }
            observed.synchronize(self)?;
            if self.root().available()? < reserve {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "root metadata reserve changed while saving move recovery envelope",
                ));
            }
            Ok(())
        };
        create().map_err(|error| {
            error.context(
                ErrorCode::RecoveryRequired,
                "move recovery envelope may be retained; inspect the same original operation before retrying",
            )
        })
    }
}

impl Observed {
    pub(super) fn check_names(&self, store: &Store) -> Result<()> {
        store.check_owner()?;
        let path = store.root().path().join(DIRECTORY);
        let named = store.root().open_read_regular(&path.join(NAME))?;
        if store.root().directory_identity(&path)? != FileIdentity::from_fd(&self.directory)?
            || FileIdentity::from_metadata(&named.metadata()?)
                != FileIdentity::from_metadata(&self.file.metadata()?)
        {
            return Err(stale("move recovery envelope file or directory name changed"));
        }
        verify_bytes(&named, &self.bytes)?;
        store.check_owner()
    }

    fn synchronize(&self, store: &Store) -> Result<()> {
        self.check_names(store)?;
        self.file.sync_all()?;
        self.check_names(store)?;
        self.directory.sync_all()?;
        store.root().sync_dir(store.root().path())?;
        self.check_names(store)
    }
}

fn verify_bytes(file: &ReadFile, bytes: &[u8]) -> Result<()> {
    if file.metadata()?.len() != bytes.len() as u64 {
        return Err(corrupt("retained move recovery envelope size changed"));
    }
    let mut buffer = [0u8; 64 * 1024];
    let mut offset = 0;
    for chunk in bytes.chunks(buffer.len()) {
        file.read_exact_at(&mut buffer[..chunk.len()], offset)?;
        if &buffer[..chunk.len()] != chunk {
            return Err(corrupt("retained move recovery envelope bytes changed"));
        }
        offset += chunk.len() as u64;
    }
    if file.metadata()?.len() != offset {
        return Err(corrupt("retained move recovery envelope changed during verification"));
    }
    file.check_running()
}
