//! Canonical rescue records can rebuild independent SQLite without SYSTEM,
//! seals or the original database. These bytes confer no installation authority.
use super::*;

pub const MAX_DISK_DATABASE_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const FORMAT: &str = "disk-local-database-rescue-material";
const DOMAIN: &str = "disk-database-rescue-material";

#[derive(Serialize)]
struct Content<'a> {
    scope: DiskDatabaseReadScope,
    identity: &'a DiskRegistrationIdentity,
    current: &'a Option<DiskCatalogSnapshot>,
    history: &'a Option<DiskHistorySnapshot>,
}
#[derive(Serialize)]
struct Output<'a> {
    format: &'static str,
    material: Content<'a>,
    records_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Input {
    format: String,
    material: MaterialInput,
    records_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MaterialInput {
    scope: DiskDatabaseReadScope,
    identity: DiskRegistrationIdentity,
    current: Option<DiskCatalogSnapshot>,
    history: Option<DiskHistorySnapshot>,
}

/// The caller selects exactly which representation to reconstruct. Never choose
/// a winner by count, filename, sequence, or a presumed current/history quorum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
pub enum DiskDatabaseRebuildSource {
    StoredCurrent,
    History,
    CurrentFromHistory,
}

/// Keep the selected semantic snapshot, not just a digest that could hide a
/// changed header or an image from a different reconstruction source.
enum ReconstructedSnapshot {
    Current(DiskCatalogSnapshot),
    History(DiskHistorySnapshot),
}

/// A standalone image and its declared coverage, not a disk binding capability.
pub struct DiskDatabaseReconstruction {
    source: DiskDatabaseRebuildSource,
    snapshot: ReconstructedSnapshot,
    records_hash: String,
    bytes: Vec<u8>,
}
impl DiskDatabaseReconstruction {
    pub fn identity(&self) -> &DiskRegistrationIdentity {
        match &self.snapshot {
            ReconstructedSnapshot::Current(row) => &row.identity,
            ReconstructedSnapshot::History(row) => &row.header.identity,
        }
    }
    pub fn source(&self) -> DiskDatabaseRebuildSource {
        self.source
    }
    pub fn source_sequence(&self) -> u64 {
        match &self.snapshot {
            ReconstructedSnapshot::Current(row) => row.source_sequence,
            ReconstructedSnapshot::History(row) => row.header.through_sequence,
        }
    }
    pub fn records(&self) -> usize {
        match &self.snapshot {
            ReconstructedSnapshot::Current(row) => {
                row.versions.len() + row.backup_directories.len()
            }
            ReconstructedSnapshot::History(row) => row.changes.len(),
        }
    }
    pub fn records_hash(&self) -> &str {
        &self.records_hash
    }
    pub fn database_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn maximum_database_bytes(&self) -> usize {
        MAX_DATABASE_BYTES
    }

    /// Decode a separately read image, checking the entire exact scope, source
    /// coverage and records. This is a rescue-copy check, not authority admission.
    pub fn verify_database_bytes(&self, bytes: &[u8]) -> Result<()> {
        let matches = match &self.snapshot {
            ReconstructedSnapshot::Current(expected) => {
                super::super::super::database::read(bytes, &expected.identity)? == *expected
            }
            ReconstructedSnapshot::History(expected) => {
                database::read(bytes, &expected.header.identity)? == *expected
            }
        };
        if !matches {
            return Err(invalid(
                "disk rescue image differs from the complete selected snapshot",
            ));
        }
        Ok(())
    }
}

impl DiskDatabaseMaterial {
    /// The original scope and absences are retained. Derived inventory is not
    /// serialized as though it were a surviving physical current database.
    pub fn archive_bytes(&self) -> Result<Vec<u8>> {
        let material = Content {
            scope: self.scope,
            identity: &self.identity,
            current: &self.current,
            history: &self.history,
        };
        let records_hash = storage_contract::object_hash(DOMAIN, &material)?;
        crate::disk::encode_metadata(
            &Output {
                format: FORMAT,
                material,
                records_hash,
            },
            MAX_DISK_DATABASE_ARCHIVE_BYTES,
        )
    }
}

/// Pure reconstruction. The expected binding is supplied separately from the
/// archive. The complete archive is checked even when only one image is wanted.
/// It cannot register a disk, revive a retired node, read bodies or open SYSTEM.
pub fn reconstruct_disk_database_material(
    bytes: &[u8],
    expected: &DiskRegistrationIdentity,
    source: DiskDatabaseRebuildSource,
) -> Result<DiskDatabaseReconstruction> {
    if bytes.len() > MAX_DISK_DATABASE_ARCHIVE_BYTES {
        return Err(budget());
    }
    let input: Input = storage_contract::decode_strict(bytes)?;
    if input.format != FORMAT
        || crate::disk::encode_metadata(&input, MAX_DISK_DATABASE_ARCHIVE_BYTES)? != bytes
        || storage_contract::object_hash(DOMAIN, &input.material)? != input.records_hash
    {
        return Err(invalid(
            "disk database rescue archive is not exact canonical material",
        ));
    }
    let mut material = input.material;
    validate_scope(&material, expected)?;
    let current = material.current.as_ref().map(current_image).transpose()?;
    let history = material.history.as_ref().map(history_image).transpose()?;
    let inventory = material
        .history
        .as_ref()
        .map(|history| fold(history, material.current.as_ref()))
        .transpose()?;
    let (bytes, records_hash, snapshot) = match source {
        DiskDatabaseRebuildSource::StoredCurrent => {
            let snapshot = material
                .current
                .take()
                .ok_or_else(|| invalid("archive has no stored current database"))?;
            let (bytes, hash) = current.expect("invariant: present current was encoded above");
            (bytes, hash, ReconstructedSnapshot::Current(snapshot))
        }
        DiskDatabaseRebuildSource::History => {
            let snapshot = material
                .history
                .take()
                .ok_or_else(|| invalid("archive has no retained history database"))?;
            let (bytes, hash) = history.expect("invariant: present history was encoded above");
            (bytes, hash, ReconstructedSnapshot::History(snapshot))
        }
        DiskDatabaseRebuildSource::CurrentFromHistory => {
            let snapshot = inventory.ok_or_else(|| {
                invalid("current-from-history requires complete retained history material")
            })?;
            let (bytes, hash) = current_image(&snapshot)?;
            (bytes, hash, ReconstructedSnapshot::Current(snapshot))
        }
    };
    Ok(DiskDatabaseReconstruction {
        source,
        snapshot,
        records_hash,
        bytes,
    })
}

fn validate_scope(material: &MaterialInput, expected: &DiskRegistrationIdentity) -> Result<()> {
    storage_contract::validate_id(&expected.pool_id, "pool")?;
    storage_contract::validate_id(&expected.disk_id, "disk")?;
    name_policy::validate_slot(&expected.slot_id)?;
    if expected.binding_generation == 0
        || material.identity != *expected
        || material
            .current
            .as_ref()
            .is_some_and(|current| current.identity != *expected)
        || material
            .history
            .as_ref()
            .is_some_and(|history| history.header.identity != *expected)
    {
        return Err(invalid(
            "rescue archive differs from the explicit disk registration identity",
        ));
    }
    if !material.scope.current() && material.current.is_some()
        || !material.scope.history() && material.history.is_some()
        || material.current.is_none() && material.history.is_none()
    {
        return Err(invalid(
            "disk database rescue scope contradicts its retained files",
        ));
    }
    Ok(())
}

fn charge<T: Serialize>(row: &T, total: &mut usize) -> Result<()> {
    let length = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?.len();
    if length > MAX_RECORD_BYTES - *total {
        return Err(budget());
    }
    *total += length;
    Ok(())
}

fn current_image(snapshot: &DiskCatalogSnapshot) -> Result<(Vec<u8>, String)> {
    if snapshot.source_sequence == 0 && !snapshot.is_empty() {
        return Err(invalid(
            "genesis current material cannot contain retained rows",
        ));
    }
    if snapshot.versions.len() > MAX_ROWS
        || snapshot.backup_directories.len() > MAX_ROWS - snapshot.versions.len()
    {
        return Err(budget());
    }
    let mut total = 0;
    for row in &snapshot.versions {
        charge(row, &mut total)?;
    }
    for row in &snapshot.backup_directories {
        charge(row, &mut total)?;
    }
    let hash = storage_contract::object_hash("disk-current-catalog", snapshot)?;
    let bytes = super::super::super::database::encode(snapshot, &hash)?;
    if super::super::super::database::read(&bytes, &snapshot.identity)? != *snapshot {
        return Err(invalid(
            "rebuilt current database differs from its complete rescue records",
        ));
    }
    Ok((bytes, hash))
}

fn history_image(snapshot: &DiskHistorySnapshot) -> Result<(Vec<u8>, String)> {
    if snapshot.changes.len() > MAX_CHANGES
        || snapshot.header.publications as usize > MAX_PUBLICATIONS
    {
        return Err(budget());
    }
    let mut total = 0;
    for row in &snapshot.changes {
        charge(row, &mut total)?;
    }
    let hash = digest(snapshot)?;
    let bytes = database::encode(snapshot, &hash)?;
    if database::read(&bytes, &snapshot.header.identity)? != *snapshot {
        return Err(invalid(
            "rebuilt history database differs from its complete rescue records",
        ));
    }
    Ok((bytes, hash))
}
