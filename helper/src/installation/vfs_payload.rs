use crate::Result;
use data_serialization::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) const HOST_VERSION: &str = "Version 4.22.11-Debian-4.22.11+dfsg-0+deb13u1";
pub(crate) const MODULE_DIRECTORY: &str = "/usr/lib/x86_64-linux-gnu/samba/vfs";
pub(crate) const MODULE_PATH: &str = "/usr/lib/x86_64-linux-gnu/samba/vfs/zxs.so";
pub(crate) const MODULE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Payload {
    bytes: &'static [u8],
    receipt: &'static [u8],
    version: &'static str,
    commit: &'static str,
    tree: &'static str,
    target: &'static str,
    sha256: &'static str,
    receipt_sha256: &'static str,
}
include!(concat!(env!("OUT_DIR"), "/install_vfs.rs"));

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Target {
    distribution: String,
    release: String,
    suite: String,
    architecture: String,
    package_version: String,
    upstream_version: String,
    vfs_interface: u32,
    source_files: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct BuildReceipt {
    distribution: String,
    release: String,
    suite: String,
    architecture: String,
    package_version: String,
    upstream_version: String,
    vfs_interface: u32,
    source_files: BTreeMap<String, String>,
    commit: String,
    tree: String,
    module_sha256: String,
    sdk_recipe_sha256: String,
    sdk_config_sha256: String,
    validation_mode: String,
    runtime_acceptance: String,
    host_version: String,
}
impl BuildReceipt {
    fn require_target(&self, payload: Payload) -> Result<()> {
        let target: Target =
            json_codec::from_str(include_str!("../../../.github/samba-debian-target.json"))?;
        if self.distribution != target.distribution
            || self.release != target.release
            || self.suite != target.suite
            || self.architecture != target.architecture
            || self.package_version != target.package_version
            || self.upstream_version != target.upstream_version
            || self.vfs_interface != target.vfs_interface
            || self.source_files != target.source_files
            || self.commit != payload.commit
            || self.tree != payload.tree
            || self.module_sha256 != payload.sha256
            || self.host_version != HOST_VERSION
            || !matches!(self.validation_mode.as_str(), "full" | "build-only")
            || self.runtime_acceptance != "not-run-by-build-script"
        {
            return Err("embedded VFS receipt differs from the exact helper source or supported Debian Samba build".into());
        }
        for digest in [&self.sdk_recipe_sha256, &self.sdk_config_sha256] {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err("embedded VFS SDK evidence has an invalid byte identity".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize)]
pub(crate) struct Metadata {
    version: &'static str,
    commit: &'static str,
    tree: &'static str,
    target: &'static str,
    sha256: &'static str,
    bytes: usize,
    receipt_sha256: &'static str,
    sdk: BuildReceipt,
}
pub(crate) struct Verified {
    pub bytes: &'static [u8],
    metadata: Metadata,
}
impl Verified {
    pub(crate) fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }
    pub(crate) fn sha256(&self) -> &'static str {
        self.metadata.sha256
    }
}
pub(crate) fn present() -> bool {
    EMBEDDED.is_some()
}
pub(crate) fn required() -> Result<Verified> {
    let payload = EMBEDDED.ok_or("this helper has no sealed matching VFS payload; use the matching offline release helper, never a runtime download or arbitrary module")?;
    let program = super::payload::required()?.metadata();
    if payload.version != program.version
        || payload.commit != program.commit
        || payload.tree != program.tree
        || payload.target != program.target
        || !(64..=MODULE_LIMIT as usize).contains(&payload.bytes.len())
        || payload.receipt.len() > 64 * 1024
        || platform_io::lower_hex(&Sha256::digest(payload.bytes)) != payload.sha256
        || platform_io::lower_hex(&Sha256::digest(payload.receipt)) != payload.receipt_sha256
    {
        return Err(
            "embedded VFS bytes or source binding differ from the exact ZXS payload".into(),
        );
    }
    let bytes = payload.bytes;
    if bytes[..7] != *b"\x7fELF\x02\x01\x01"
        || u16::from_le_bytes([bytes[16], bytes[17]]) != 3
        || u16::from_le_bytes([bytes[18], bytes[19]]) != 62
        || u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) != 1
        || u16::from_le_bytes([bytes[52], bytes[53]]) != 64
    {
        return Err("embedded VFS is not the supported ELF shared-object ABI".into());
    }
    let receipt: BuildReceipt = json_codec::from_slice(payload.receipt)?;
    receipt.require_target(payload)?;
    Ok(Verified {
        bytes,
        metadata: Metadata {
            version: payload.version,
            commit: payload.commit,
            tree: payload.tree,
            target: payload.target,
            sha256: payload.sha256,
            bytes: bytes.len(),
            receipt_sha256: payload.receipt_sha256,
            sdk: receipt,
        },
    })
}
