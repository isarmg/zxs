//! An image commitment describes original physical bytes, not a recipe for
//! regenerating byte-identical SQLite pages from equivalent logical rows.
use crate::Result;
use crate::disk::{DiskRoot, read_file_bytes};
use crate::engine::scope_closure::ScopeClosureError;
use crate::persist::layout;
use data_serialization::{Deserialize, Serialize};
use storage_contract::ContentHash;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::engine) enum Schema {
    #[serialization(rename = "disk-current-local-catalog")]
    DiskCurrent,
    #[serialization(rename = "disk-local-inventory-changes")]
    DiskHistory,
    #[serialization(rename = "system-business-index")]
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::engine) enum Representation {
    Sqlite {
        schema: Schema,
        image_hash: ContentHash,
        #[serialization(with = "storage_contract::u64_string")]
        image_bytes: u64,
    },
}
impl Representation {
    fn new(schema: Schema, bytes: &[u8]) -> Self {
        Self::Sqlite {
            schema,
            image_hash: ContentHash::from_digest(blake3::hash(bytes)),
            image_bytes: bytes.len() as u64,
        }
    }

    pub(in crate::engine) fn verify<T>(
        self,
        root: &DiskRoot,
        required: Schema,
        maximum: usize,
        inspect: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        let Self::Sqlite {
            schema,
            image_hash,
            image_bytes,
        } = self;
        if schema != required || image_bytes == 0 {
            return Err(ScopeClosureError::ImageMismatch.into());
        }
        let length = usize::try_from(image_bytes).map_err(|_| ScopeClosureError::Budget)?;
        if length > maximum {
            return Err(ScopeClosureError::Budget.into());
        }
        let path = layout::scope_image(image_hash);
        reject_companions(root, &path)?;
        let file = root
            .open_optional_read_relative(&path)?
            .ok_or(ScopeClosureError::ImageMismatch)?;
        let stamp = crate::file_stamp::FileStamp::from_metadata(&file.metadata()?);
        let bytes = read_file_bytes(file.try_clone()?, length)?;
        if Self::new(schema, &bytes) != self {
            return Err(ScopeClosureError::ImageMismatch.into());
        }
        // Keep the admitted FD through the SQLite schema and row read, then
        // recheck that neither the selected image nor its actual name moved.
        let result = inspect(&bytes)?;
        let named = root
            .open_optional_read_relative(&path)?
            .ok_or(ScopeClosureError::ImageMismatch)?;
        if crate::file_stamp::FileStamp::from_metadata(&file.metadata()?) != stamp
            || crate::file_stamp::FileStamp::from_metadata(&named.metadata()?) != stamp
        {
            return Err(ScopeClosureError::ImageMismatch.into());
        }
        reject_companions(root, &path)?;
        file.check_running()?;
        Ok(result)
    }
}

// A frozen standalone image has no WAL, SHM or rollback journal. Never open,
// checkpoint, truncate or delete unexpected siblings to make it readable.
fn preflight_destination(
    root: &DiskRoot,
    path: &platform_io::fs_safety::RelativePath,
) -> Result<()> {
    // On first installation the hash-prefix directory need not exist yet.
    // Only an absent child under an already verified parent proves absence.
    let mut parent = root.path().to_owned();
    for component in path
        .as_path()
        .parent()
        .expect("scope image has a fixed private parent")
    {
        parent.push(component);
        if root.optional_metadata(&parent)?.is_none() {
            return Ok(());
        }
        root.directory_identity(&parent)?;
    }
    reject_companions(root, path)
}
fn reject_companions(root: &DiskRoot, path: &platform_io::fs_safety::RelativePath) -> Result<()> {
    let image = root.path().join(path.as_path());
    for extension in ["sqlite-wal", "sqlite-shm", "sqlite-journal"] {
        if root
            .optional_metadata(&image.with_extension(extension))?
            .is_some()
        {
            return Err(ScopeClosureError::ImageMismatch.into());
        }
    }
    Ok(())
}

pub(in crate::engine) struct PreparedImage {
    bytes: Vec<u8>,
    representation: Representation,
}
impl PreparedImage {
    pub(in crate::engine) fn new(schema: Schema, bytes: Vec<u8>) -> Self {
        Self {
            representation: Representation::new(schema, &bytes),
            bytes,
        }
    }

    pub(in crate::engine) fn bytes(&self) -> usize {
        self.bytes.len()
    }

    pub(in crate::engine) fn representation(&self) -> Representation {
        self.representation
    }

    pub(in crate::engine) fn install_verified(
        &self,
        root: &DiskRoot,
        inspect_logical: impl FnOnce(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let Representation::Sqlite {
            schema, image_hash, ..
        } = self.representation;
        // Existing immutable material is compared, never exchanged; both a new
        // and a previously completed install synchronize file and parent.
        let path = layout::scope_image(image_hash);
        preflight_destination(root, &path)?;
        root.write_relative_atomic(&path, &self.bytes, false)?;
        self.representation
            .verify(root, schema, self.bytes.len(), inspect_logical)
    }
}
