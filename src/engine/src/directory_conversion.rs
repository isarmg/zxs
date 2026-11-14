//! Explicit, non-destructive directory conversion used by the disk wizard.
//! Discovery never calls these operations. A reviewed plan copies contents into
//! a new destination; it neither admits foreign storage nor changes disk format.
use crate::{ContentHash, Engine, Error, ErrorCode, Result};
use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};
mod import;
mod name_budget;
pub use crate::name_budget::NameBudgetError;
mod output;
mod source;
mod stop;
pub use import::{
    DirectoryImportFailure, DirectoryImportProgress, OrdinaryDirectoryPreview,
    PreparedDirectoryImport, PreparedOrdinaryDirectory,
};
pub(crate) use stop::require_import_not_stopped;
pub use stop::{DirectoryImportStopOutcome, DirectoryImportStopRequest};
mod export;
pub use export::{DirectoryExportOutcome, PreparedDirectoryExport};

/// A review commitment, separate from content hashes and publication identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryConversionDigest([u8; 32]);
#[derive(Debug, error_definition::Error)]
#[error("conversion digest must be exactly 64 lowercase hexadecimal characters")]
pub struct ParseDirectoryConversionDigest;
impl fmt::Display for DirectoryConversionDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}
impl FromStr for DirectoryConversionDigest {
    type Err = ParseDirectoryConversionDigest;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let parsed = value
            .parse::<ContentHash>()
            .map_err(|_| ParseDirectoryConversionDigest)?;
        Ok(Self(*parsed.digest().as_bytes()))
    }
}

/// Display before confirmation. Ordinary directories preserve names, file bytes
/// and empty directories, not permissions, timestamps, ACLs, xattrs, or history.
#[derive(Clone, Debug)]
pub struct DirectoryConversionPreview {
    pub digest: DirectoryConversionDigest,
    pub source: PathBuf,
    pub destination: String,
    pub files: usize,
    pub directories: usize,
    pub content_bytes: u64,
    /// Additional full workspace bytes during import; metadata and any configured
    /// backup copies are extra. This is not a free-space admission certificate.
    pub temporary_bytes: u64,
}

/// Only construct after the administrator has reviewed the exact preview and
/// acknowledged its contents-only scope. A plan is not confirmation by itself.
pub struct DirectoryConversionConfirmation {
    digest: DirectoryConversionDigest,
}
impl DirectoryConversionConfirmation {
    pub fn contents_only(digest: DirectoryConversionDigest) -> Self {
        Self { digest }
    }
    fn verify(&self, preview: &DirectoryConversionPreview) -> Result<()> {
        if self.digest != preview.digest {
            return Err(Error::new(
                ErrorCode::PlanInvalid,
                "conversion confirmation differs from the reviewed plan",
            ));
        }
        Ok(())
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::PlanInvalid, message)
}
fn io(action: &'static str, path: impl AsRef<Path>, source: std::io::Error) -> Error {
    Error::from(source).context(ErrorCode::IoError, format!("{action}: {:?}", path.as_ref()))
}
fn disjoint(left: &Path, right: &Path) -> Result<()> {
    if left.starts_with(right) || right.starts_with(left) {
        return Err(invalid(format!(
            "conversion source and target roots overlap: {left:?}, {right:?}"
        )));
    }
    Ok(())
}
fn external_root(engine: &Engine, path: &Path) -> Result<PathBuf> {
    let path = crate::disk::absolute_path(path)?;
    if path.to_str().is_none() {
        return Err(invalid("conversion root must be valid UTF-8"));
    }
    disjoint(&path, engine.root())?;
    for disk in &engine.config().disks {
        disjoint(&path, &disk.root)?;
    }
    Ok(path)
}
fn digest_field(digest: &mut blake3::Hasher, value: &[u8]) {
    digest.update(&(value.len() as u64).to_le_bytes());
    digest.update(value);
}

#[cfg(test)]
#[path = "../../../test/engine/unit/directory_conversion/tests.rs"]
mod tests;
