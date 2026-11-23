//! Public immutable Rust-family installation bytes. This type does not
//! create a host layout, grant access, open a vault, prove the active
//! credential generation, or bind a listener.
use super::first::{
    Folder, FolderIds, Id, RecycleName, decode_layout, emit_layout, fixed, folder_ids,
    sort_selection, take,
};
use sha2::{Digest, Sha256};

const TAG: &[u8; 16] = b"ZXS-RUST-SMB-I2\0";
const IDS: &[u8] = b"ZXS-RUST-SMB-I2/ID\0";
pub(crate) const MAX_WIRE: usize = super::first::MAX_WIRE + 1 + 253;
const _: () = assert!(MAX_WIRE == 17_596);
const BAD: &str = "not an exact Rust SMB installation descriptor";
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Origin {
    pub(crate) pool: Id,
    pub(crate) init: Id,
    pub(crate) instance: Id,
    pub(crate) bind: Id,
    pub(crate) setup: Id,
}
impl Origin {
    fn ids(self) -> [Id; 5] {
        [self.pool, self.init, self.instance, self.bind, self.setup]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustPlan {
    pub(crate) origin: Origin,
    host: String,
    folders: Vec<(Folder, FolderIds)>,
    recycle: RecycleName,
    raw: Vec<u8>,
    sha: [u8; 32],
}
pub(crate) fn valid_host(host: &str) -> bool {
    if !(1..=253).contains(&host.len()) {
        return false;
    }
    host.split('.').enumerate().all(|(i, label)| {
        let b = label.as_bytes();
        (1..=63).contains(&b.len())
            && (i != 0 || (b.len() <= 15 && b.iter().any(u8::is_ascii_lowercase)))
            && !label.starts_with('-')
            && !label.ends_with('-')
            && b.iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    })
}

impl RustPlan {
    pub(crate) fn selected(
        origin: Origin,
        host: String,
        mut folders: Vec<Folder>,
        recycle: RecycleName,
    ) -> Result<Self> {
        if !valid_host(&host) {
            return Err(BAD);
        }
        let done = sort_selection(origin.ids(), &mut folders, &recycle)?;
        let mut raw = TAG.to_vec();
        for id in origin.ids() {
            raw.extend(id);
        }
        raw.push(u8::try_from(host.len()).map_err(|_| BAD)?);
        raw.extend_from_slice(host.as_bytes());
        emit_layout(&mut raw, &folders, &recycle)?;
        if raw.len() > MAX_WIRE {
            return Err(BAD);
        }
        let sha = Sha256::digest(&raw).into();
        let folders = folder_ids(IDS, &sha, folders, done)?;
        Ok(Self {
            origin,
            host,
            folders,
            recycle,
            raw,
            sha,
        })
    }
    pub(crate) fn decode(raw: &[u8]) -> Result<Self> {
        let mut p = raw
            .strip_prefix(TAG)
            .filter(|_| raw.len() <= MAX_WIRE)
            .ok_or(BAD)?;
        let origin = Origin {
            pool: fixed(&mut p)?,
            init: fixed(&mut p)?,
            instance: fixed(&mut p)?,
            bind: fixed(&mut p)?,
            setup: fixed(&mut p)?,
        };
        let [n] = fixed(&mut p)?;
        let host = std::str::from_utf8(take(&mut p, n as usize)?)
            .map_err(|_| BAD)?
            .to_owned();
        let (folders, recycle) = decode_layout(&mut p)?;
        let plan = Self::selected(origin, host, folders, recycle)?;
        if !p.is_empty() || plan.wire() != raw {
            return Err(BAD);
        }
        Ok(plan)
    }
    pub(crate) fn wire(&self) -> &[u8] {
        &self.raw
    }
    pub(crate) fn sha(&self) -> [u8; 32] {
        self.sha
    }
    pub(crate) fn host(&self) -> &str {
        &self.host
    }
    pub(crate) fn folders(&self) -> &[(Folder, FolderIds)] {
        &self.folders
    }
    pub(crate) fn recycle(&self) -> &RecycleName {
        &self.recycle
    }
}

#[cfg(test)]
#[path = "../../tests/smb-host/rust_first.rs"]
mod tests;
