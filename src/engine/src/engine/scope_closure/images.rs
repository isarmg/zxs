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

    pub(in crate::engine) fn read(
        self,
        root: &DiskRoot,
        required: Schema,
        maximum: usize,
    ) -> Result<Vec<u8>> {
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
        let file = root
            .open_optional_read_relative(&path)?
            .ok_or(ScopeClosureError::ImageMismatch)?;
        let bytes = read_file_bytes(file, length)?;
        if Self::new(schema, &bytes) != self {
            return Err(ScopeClosureError::ImageMismatch.into());
        }
        Ok(bytes)
    }
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
        root.write_relative_atomic(&layout::scope_image(image_hash), &self.bytes, false)?;
        let actual = self.representation.read(root, schema, self.bytes.len())?;
        inspect_logical(&actual)
    }
}
