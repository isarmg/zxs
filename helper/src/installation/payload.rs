use crate::Result;
use data_serialization::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub(super) struct Payload {
    pub bytes: &'static [u8],
    pub version: &'static str,
    pub commit: &'static str,
    pub tree: &'static str,
    pub target: &'static str,
    pub sha256: &'static str,
}
include!(concat!(env!("OUT_DIR"), "/install_payload.rs"));

#[derive(Serialize)]
pub(crate) struct Metadata {
    pub version: &'static str,
    pub commit: &'static str,
    pub tree: &'static str,
    pub target: &'static str,
    pub sha256: &'static str,
    pub bytes: usize,
}
impl Payload {
    pub fn metadata(self) -> Metadata {
        Metadata {
            version: self.version,
            commit: self.commit,
            tree: self.tree,
            target: self.target,
            sha256: self.sha256,
            bytes: self.bytes.len(),
        }
    }
    pub fn verify(self) -> Result<Self> {
        if self.version != env!("CARGO_PKG_VERSION")
            || self.target != "x86_64-unknown-linux-gnu"
            || self.bytes.len() < 64
            || self.bytes.len() > 256 * 1024 * 1024
            || platform_io::lower_hex(&Sha256::digest(self.bytes)) != self.sha256
        {
            return Err("embedded ZXS payload identity, size or SHA-256 mismatch; no installation was attempted".into());
        }
        Ok(self)
    }
}
pub(super) fn required() -> Result<Payload> {
    EMBEDDED.ok_or("this development helper has no embedded ZXS payload; use a matching release helper, not a runtime download")?.verify()
}
pub(crate) fn metadata() -> Option<Metadata> {
    EMBEDDED.map(Payload::metadata)
}
