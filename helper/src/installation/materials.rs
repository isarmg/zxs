//! Exact public license/notice bytes. Never extracts an archive or mutates the
//! installed program, runtime state, configuration or credentials.
use crate::{Result, private_io};
use data_serialization::Serialize;
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};

#[derive(Clone, Copy)]
struct Embedded {
    archive: &'static [u8],
    manifest: &'static [u8],
    archive_sha256: &'static str,
    manifest_sha256: &'static str,
    version: &'static str,
    commit: &'static str,
    tree: &'static str,
    target: &'static str,
    payload_sha256: &'static str,
}
include!(concat!(env!("OUT_DIR"), "/install_materials.rs"));

#[derive(Serialize)]
pub(crate) struct Metadata {
    version: &'static str,
    commit: &'static str,
    tree: &'static str,
    target: &'static str,
    payload_sha256: &'static str,
    pub archive_sha256: &'static str,
    pub manifest_sha256: &'static str,
    archive_bytes: usize,
    manifest_bytes: usize,
    scope: &'static str,
    legal_completeness_attested: bool,
}
impl Embedded {
    fn metadata(self) -> Metadata {
        Metadata {
            version: self.version,
            commit: self.commit,
            tree: self.tree,
            target: self.target,
            payload_sha256: self.payload_sha256,
            archive_sha256: self.archive_sha256,
            manifest_sha256: self.manifest_sha256,
            archive_bytes: self.archive.len(),
            manifest_bytes: self.manifest.len(),
            scope: "central_licenses_notices_and_origin_inventory",
            legal_completeness_attested: false,
        }
    }
    fn verify(self, payload: super::payload::Payload) -> Result<Verified> {
        if self.version != payload.version
            || self.commit != payload.commit
            || self.tree != payload.tree
            || self.target != payload.target
            || self.payload_sha256 != payload.sha256
            || self.archive.is_empty()
            || self.archive.len() > 32 * 1024 * 1024
            || self.manifest.is_empty()
            || self.manifest.len() > 4 * 1024 * 1024
            || !self.archive.starts_with(b"\x1f\x8b\x08")
            || platform_io::lower_hex(&Sha256::digest(self.archive)) != self.archive_sha256
            || platform_io::lower_hex(&Sha256::digest(self.manifest)) != self.manifest_sha256
        {
            return Err("embedded legal material bytes or exact payload/source binding differ; no material was exported".into());
        }
        Ok(Verified {
            archive: self.archive,
            manifest: self.manifest,
            metadata: self.metadata(),
        })
    }
}

pub(crate) struct Verified {
    pub archive: &'static [u8],
    pub manifest: &'static [u8],
    pub metadata: Metadata,
}
pub(crate) fn metadata() -> Option<Metadata> {
    MATERIALS.map(Embedded::metadata)
}
pub(crate) fn verified() -> Result<Verified> {
    let payload = super::payload::required()?;
    MATERIALS
        .ok_or(
            "this helper has no embedded legal materials; use the matching complete release helper",
        )?
        .verify(payload)
}
pub(crate) fn export(path: &Path) -> Result<Metadata> {
    let materials = verified()?;
    private_io::write_new(path, materials.archive)?;
    Ok(materials.metadata)
}
pub(crate) fn print_manifest() -> Result<()> {
    let materials = verified()?;
    std::io::stdout().lock().write_all(materials.manifest)?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/helper/unit/installation/materials_tests.rs"]
mod tests;
