//! Independently retained object-local metadata, never a pool publication or
//! installation capability. A reconstruction stays in memory until an explicit
//! rescue-file consumer publishes it outside the managed database names.
use super::{directory_database, file_database};
use crate::{
    Error, ErrorCode, Result,
    disk::{DiskRoot, HeldRegular},
};
use data_serialization::{Deserialize, Serialize};
use std::{fs::File, path::PathBuf};

pub const MAX_OBJECT_DATABASE_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RECORDS: usize = 65_536;
const MAX_RECORD_BYTES: usize = 12 * 1024 * 1024;
const MAX_DATABASE_BYTES: usize = 16 * 1024 * 1024;
const ARCHIVE_FORMAT: &str = "object-local-database-material";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectDatabaseScope {
    File {
        file_id: String,
        history_tree_id: String,
    },
    Directory {
        repository_id: String,
        directory_id: String,
    },
}
impl ObjectDatabaseScope {
    fn validate(&self) -> Result<()> {
        match self {
            Self::File {
                file_id,
                history_tree_id,
            } => {
                storage_contract::validate_id(file_id, "file")?;
                storage_contract::validate_id(history_tree_id, "history")?;
            }
            Self::Directory {
                repository_id,
                directory_id,
            } => {
                storage_contract::validate_id(repository_id, "repo")?;
                if directory_id != repository_id {
                    storage_contract::validate_id(directory_id, "entry")?;
                }
            }
        }
        Ok(())
    }
    fn leaf(&self) -> &'static str {
        match self {
            Self::File { .. } => file_database::NAME,
            Self::Directory { .. } => directory_database::NAME,
        }
    }
    fn read(&self, bytes: &[u8]) -> Result<ObjectDatabaseMaterial> {
        let material = match self {
            Self::File { .. } => file_database::read_material(bytes)?,
            Self::Directory { .. } => directory_database::read_material(bytes)?,
        };
        if material.scope != *self {
            return Err(invalid(
                "object database differs from the explicitly selected scope",
            ));
        }
        Ok(material)
    }
}

/// Exact stored canonical JSON, not a newly synthesized State or global path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ObjectDatabaseRecord {
    kind: String,
    id: String,
    body: String,
}
impl ObjectDatabaseRecord {
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn body_json(&self) -> &str {
        &self.body
    }
    pub(super) fn new(kind: &str, id: String, body: String) -> Self {
        Self {
            kind: kind.into(),
            id,
            body,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct ObjectDatabaseMaterial {
    scope: ObjectDatabaseScope,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    records_hash: String,
    records: Vec<ObjectDatabaseRecord>,
}
impl ObjectDatabaseMaterial {
    pub fn scope(&self) -> &ObjectDatabaseScope {
        &self.scope
    }
    pub fn source_sequence(&self) -> u64 {
        self.source_sequence
    }
    pub fn records_hash(&self) -> &str {
        &self.records_hash
    }
    pub fn records(&self) -> &[ObjectDatabaseRecord] {
        &self.records
    }

    pub(super) fn new(
        scope: ObjectDatabaseScope,
        source_sequence: u64,
        records_hash: String,
        records: Vec<ObjectDatabaseRecord>,
    ) -> Self {
        Self {
            scope,
            source_sequence,
            records_hash,
            records,
        }
    }

    /// This durable rescue representation contains no selected host path,
    /// disk identity, original authority proof, or permission to install it.
    pub fn archive_bytes(&self) -> Result<Vec<u8>> {
        self.validate_envelope()?;
        crate::disk::encode_metadata(
            &ArchiveOutput {
                format: ARCHIVE_FORMAT,
                material: self,
            },
            MAX_OBJECT_DATABASE_ARCHIVE_BYTES,
        )
    }

    fn validate_envelope(&self) -> Result<()> {
        self.scope.validate()?;
        if self.source_sequence == 0 || self.records.is_empty() || self.records.len() > MAX_RECORDS
        {
            return Err(invalid(
                "object material has no bounded nonempty captured scope",
            ));
        }
        let mut bytes = 0usize;
        let mut previous = None;
        for record in &self.records {
            let length = record
                .id
                .len()
                .checked_add(record.body.len())
                .ok_or_else(budget)?;
            if record.kind.len() > 16 || length > MAX_RECORD_BYTES - bytes {
                return Err(budget());
            }
            let key = (record.kind.as_str(), record.id.as_str());
            if previous.is_some_and(|previous| previous >= key) {
                return Err(invalid("object material records are not uniquely ordered"));
            }
            previous = Some(key);
            bytes += length;
        }
        let domain = match &self.scope {
            ObjectDatabaseScope::File { .. } => "file-local-records",
            ObjectDatabaseScope::Directory { .. } => "directory-local-records",
        };
        if storage_contract::object_hash(domain, &self.records)? != self.records_hash {
            return Err(invalid(
                "object material records differ from their retained digest",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct ArchiveOutput<'a> {
    format: &'static str,
    material: &'a ObjectDatabaseMaterial,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Archive {
    format: String,
    material: MaterialInput,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct MaterialInput {
    scope: ObjectDatabaseScope,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    records_hash: String,
    records: Vec<ObjectDatabaseRecord>,
}

pub struct ObjectDatabaseReconstruction {
    material: ObjectDatabaseMaterial,
    bytes: Vec<u8>,
}
impl ObjectDatabaseReconstruction {
    pub fn material(&self) -> &ObjectDatabaseMaterial {
        &self.material
    }
    pub fn database_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn maximum_database_bytes(&self) -> usize {
        MAX_DATABASE_BYTES
    }

    /// Verify an independently read image against this exact rescue material.
    /// SQLite page identity is not required, but every typed record and header is.
    /// No file, system root or installation capability is opened or returned.
    pub fn verify_database_bytes(&self, bytes: &[u8]) -> Result<()> {
        if self.material.scope.read(bytes)? != self.material {
            return Err(invalid(
                "object rescue image differs from the complete selected material",
            ));
        }
        Ok(())
    }
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "object database material exceeds its bounded budget",
    )
}

fn no_sidecars(root: &DiskRoot, leaf: &str) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        if root
            .optional_metadata(&root.path().join(format!("{leaf}{suffix}")))?
            .is_some()
        {
            return Err(invalid(
                "object database has a live or unknown SQLite sidecar; preserve all files",
            ));
        }
    }
    Ok(())
}

/// Read one surviving entity/copy with all writers stopped. The caller chooses
/// the exact object scope; no SYSTEM, disk registry, locator, body or sibling
/// object is opened. Missing/unsupported bytes are never an empty object.
pub fn inspect_object_database_material(
    directory: File,
    path: PathBuf,
    expected: &ObjectDatabaseScope,
) -> Result<ObjectDatabaseMaterial> {
    expected.validate()?;
    let root = DiskRoot::from_directory(path, directory)?;
    let leaf = expected.leaf();
    no_sidecars(&root, leaf)?;
    let mut original = HeldRegular::capture(&root, leaf, MAX_DATABASE_BYTES)?.ok_or_else(|| {
        invalid("selected object database is absent; no metadata was reconstructed")
    })?;
    let material = expected.read(original.bytes())?;
    material.validate_envelope()?;
    original.release_bytes();
    let after = HeldRegular::capture(&root, leaf, MAX_DATABASE_BYTES)?
        .ok_or_else(|| invalid("selected object database disappeared during inspection"))?;
    if after.stamp() != original.stamp() {
        return Err(invalid(
            "selected object database changed during inspection",
        ));
    }
    no_sidecars(&root, leaf)?;
    // Revalidate all named ancestors against the still-held original directory.
    DiskRoot::from_directory(root.path().to_owned(), root.directory(root.path())?)?;
    original.check_running()?;
    Ok(material)
}

/// Rebuild a standalone SQLite image solely from an exact exported archive.
/// Reparse every supported row and compare the decoded output to the complete
/// input. This does not restore a global State, authenticate a publication,
/// admit content, revive a node or install into any running object directory.
pub fn reconstruct_object_database_material(
    archive: &[u8],
    expected: &ObjectDatabaseScope,
) -> Result<ObjectDatabaseReconstruction> {
    if archive.len() > MAX_OBJECT_DATABASE_ARCHIVE_BYTES {
        return Err(budget());
    }
    expected.validate()?;
    let decoded: Archive = storage_contract::decode_strict(archive)?;
    if decoded.format != ARCHIVE_FORMAT || &decoded.material.scope != expected {
        return Err(invalid("object archive format or explicit scope differs"));
    }
    let input = decoded.material;
    let material = ObjectDatabaseMaterial::new(
        input.scope,
        input.source_sequence,
        input.records_hash,
        input.records,
    );
    material.validate_envelope()?;
    let bytes = match expected {
        ObjectDatabaseScope::File {
            file_id,
            history_tree_id,
        } => file_database::reconstruct_material(&material, file_id, history_tree_id)?,
        ObjectDatabaseScope::Directory {
            repository_id,
            directory_id,
        } => directory_database::reconstruct_material(&material, repository_id, directory_id)?,
    };
    if expected.read(&bytes)? != material {
        return Err(invalid(
            "reconstructed object database does not reproduce its complete material",
        ));
    }
    Ok(ObjectDatabaseReconstruction { material, bytes })
}
