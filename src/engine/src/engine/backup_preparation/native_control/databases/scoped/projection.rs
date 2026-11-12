//! Bounded derived bytes from a complete target-local native prefix.
use super::*;
use crate::engine::disk_catalog::PreparedDiskScopes;
use crate::engine::index::preparation::SystemEncodingInput;
use crate::history_package::preparation_checkpoint;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_DATABASES: usize = 65;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Scope {
    SystemCurrent { pool_id: String },
    DiskCurrent { identity: DiskRegistrationIdentity },
    DiskHistory { identity: DiskRegistrationIdentity },
}
impl Scope {
    fn path(&self) -> PathBuf {
        match self {
            Self::SystemCurrent { .. } => PathBuf::from("system/index.sqlite"),
            Self::DiskCurrent { identity } => PathBuf::from("original-disks")
                .join(&identity.disk_id)
                .join(crate::database_install::DISK_DATABASE_NAME),
            Self::DiskHistory { identity } => PathBuf::from("original-disks")
                .join(&identity.disk_id)
                .join(crate::database_install::DISK_HISTORY_DATABASE_NAME),
        }
    }
}

#[derive(Serialize)]
struct Descriptor {
    scope: Scope,
    #[serialization(with = "storage_contract::u64_string")]
    bytes: u64,
    digest: String,
}

pub(super) struct Database {
    descriptor: Descriptor,
    bytes: Vec<u8>,
}
impl Database {
    pub(super) fn path(&self) -> PathBuf {
        self.descriptor.scope.path()
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub(super) struct PreparedScopes {
    databases: Vec<Database>,
    bytes: usize,
    disks: usize,
}
impl PreparedScopes {
    pub(super) fn capture(
        root: &DiskRoot,
        task: &ScopedTask,
        job: &Job,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let binding = |_: &AtomicBool| Ok(None);
        let mut encodings = encoding_memo::Transaction::new(None, &binding);
        Self::capture_with_encodings(root, task, job, stop, &mut encodings)
    }

    pub(super) fn capture_with_encodings(
        root: &DiskRoot,
        task: &ScopedTask,
        job: &Job,
        stop: &AtomicBool,
        encodings: &mut encoding_memo::Transaction<'_>,
    ) -> Result<Self> {
        let mut disks = None;
        let (control, state) = super::super::super::archive::read_observed_with_encodings(
            root,
            job,
            &task.objects.native,
            stop,
            &mut |config, candidate, hash, state| {
                preparation_checkpoint(stop)?;
                if disks.is_none() {
                    disks = Some(PreparedDiskScopes::new(config)?);
                }
                disks
                    .as_mut()
                    .expect("invariant: prefix initialized its disk scopes")
                    .observe(candidate, hash, state, stop)
            },
            encodings,
        )?;
        if control.state_hash != task.objects.state_hash {
            return Err(invalid(
                "scoped native prefix differs from original complete State",
            ));
        }
        let mut prepared = Self {
            databases: Vec::new(),
            bytes: 0,
            disks: 0,
        };
        preparation_checkpoint(stop)?;
        let mut session = encodings.session(&(task, job), stop)?;
        let checkpoint = || preparation_checkpoint(stop);
        let input = SystemEncodingInput::new(&state, &checkpoint)?;
        let system = session.system(input.input_hash(), || input.encode(&checkpoint))?;
        input.verify(&system, &checkpoint)?;
        prepared.add(
            Scope::SystemCurrent {
                pool_id: job.pool_id.clone(),
            },
            system,
        )?;
        disks
            .ok_or_else(|| invalid("scoped preparation has no admitted genesis prefix"))?
            .finish_with_encoder(&state, stop, |current, history| {
                let input = if session.enabled() {
                    encoding_memo::digest(&(current, history), stop)?
                } else {
                    None
                };
                session.disk_pair(input, || PreparedDiskScopes::encode_pair(current, history))
            }, |identity, current, history| {
                prepared.add(
                    Scope::DiskCurrent {
                        identity: identity.clone(),
                    },
                    current,
                )?;
                prepared.add(Scope::DiskHistory { identity }, history)?;
                prepared.disks += 1;
                Ok(())
            })?;
        encodings.retain(session);
        Ok(prepared)
    }
    fn add(&mut self, scope: Scope, bytes: Vec<u8>) -> Result<()> {
        let total = self.bytes.checked_add(bytes.len()).ok_or_else(budget)?;
        if total > MAX_BYTES || self.databases.len() == MAX_DATABASES {
            return Err(budget());
        }
        let descriptor = Descriptor {
            scope,
            bytes: bytes.len() as u64,
            digest: blake3::hash(&bytes).to_hex().to_string(),
        };
        self.bytes = total;
        self.databases.push(Database { descriptor, bytes });
        Ok(())
    }
    pub(super) fn databases(&self) -> &[Database] {
        &self.databases
    }
    pub(super) fn total_bytes(&self) -> u64 {
        self.bytes as u64
    }
    pub(super) fn disk_count(&self) -> usize {
        self.disks
    }
    pub(super) fn manifest(&self) -> Result<Vec<u8>> {
        let bytes = canonical_json(
            &self
                .databases
                .iter()
                .map(|database| &database.descriptor)
                .collect::<Vec<_>>(),
        )?;
        if bytes.len() > MAX_TASK {
            return Err(budget());
        }
        Ok(bytes)
    }
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "complete scoped database set exceeds 65 databases / 64 MiB",
    )
}
