//! Original-identity three-scope images from a full foreign native prefix.
//! No active installation or replacement identity is provided by this module.
use super::disk_catalog::PreparedDiskScopes;
use crate::{
    container_storage::backup_databases::PreparedDatabases, history_package::AdmittedState, *,
};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

pub(crate) const MAX_BYTES: usize = 192 * 1024 * 1024;
const MAX_DATABASES: usize = 4161;
pub(crate) struct PreparedForeignDatabases {
    pub(crate) state: ForeignOriginalStateReport,
    pub(crate) files: BTreeMap<PathBuf, Vec<u8>>,
    pub(crate) original_disks: usize,
    pub(crate) object_files: usize,
    pub(crate) object_directories: usize,
    bytes: usize,
}
impl PreparedForeignDatabases {
    pub(crate) fn capture(source: &ForeignImportPreview) -> Result<(Self, AdmittedState)> {
        let stop = AtomicBool::new(false);
        let mut disks = None;
        let (state, admitted) =
            source.visit_original_states(|config, candidate, hash, state| {
                if disks.is_none() {
                    disks = Some(PreparedDiskScopes::new(config)?);
                }
                disks
                    .as_mut()
                    .expect("invariant: first publication initialized scopes")
                    .observe(candidate, hash, state, &stop)
            })?;
        let objects = PreparedDatabases::capture(&admitted, &stop)?;
        let mut result = Self {
            state,
            files: BTreeMap::new(),
            original_disks: 0,
            object_files: objects.file_count(),
            object_directories: objects.directory_count(),
            bytes: 0,
        };
        result.add(
            "system/index.sqlite".into(),
            super::index::preparation::encode(&admitted, &stop)?,
        )?;
        disks
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "foreign source has no original disk prefix",
                )
            })?
            .finish(&admitted, &stop, |identity, current, history| {
                let root = PathBuf::from("original-disks").join(identity.disk_id);
                result.add(
                    root.join(crate::database_install::DISK_DATABASE_NAME),
                    current,
                )?;
                result.add(
                    root.join(crate::database_install::DISK_HISTORY_DATABASE_NAME),
                    history,
                )?;
                result.original_disks += 1;
                Ok(())
            })?;
        for database in objects.databases() {
            database.verify(database.bytes())?;
            result.add(database.path(), database.bytes().to_vec())?;
        }
        Ok((result, admitted))
    }
    fn add(&mut self, path: PathBuf, bytes: Vec<u8>) -> Result<()> {
        let total = self.bytes.checked_add(bytes.len()).ok_or_else(budget)?;
        if total > MAX_BYTES || self.files.len() == MAX_DATABASES {
            return Err(budget());
        }
        if self.files.insert(path, bytes).is_some() {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "foreign original database scopes overlap",
            ));
        }
        self.bytes = total;
        Ok(())
    }
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "complete foreign three-scope set exceeds 4161 databases / 192 MiB",
    )
}
