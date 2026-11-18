//! Read the original physical FILE image while its actual holder is pinned.
//! A matching FILE projection is not alone a publication or a body inventory.
use super::{MAX_BYTES, NAME, PreparedFileMaterial, invalid};
use crate::{
    Result,
    disk::{DiskRoot, HeldRegular},
};
use platform_io::fs_safety::linux::FileIdentity;

pub(in crate::container_storage) struct HeldFileImage {
    held: HeldRegular,
    before: crate::file_stamp::FileStamp,
}
impl HeldFileImage {
    pub(in crate::container_storage) fn read(
        root: &DiskRoot,
        expected: &PreparedFileMaterial,
    ) -> Result<Self> {
        let image = Self::open(root)?;
        if image.held.bytes() != expected.bytes() {
            return Err(invalid("FILE image differs from its captured physical bytes"));
        }
        expected.verify(image.held.bytes())?;
        image.require_named(root)?;
        Ok(image)
    }

    fn open(root: &DiskRoot) -> Result<Self> {
        Self::reject_sidecars(root)?;
        let path = root.path().join(NAME);
        let metadata = root.optional_metadata(&path)?
            .ok_or_else(|| invalid("required FILE image is missing"))?;
        let before = crate::file_stamp::FileStamp::from_metadata(&metadata);
        let held = HeldRegular::capture(root, NAME, MAX_BYTES)?
            .ok_or_else(|| invalid("required FILE image is missing"))?;
        let image = Self { held, before };
        image.require_named(root)?;
        Ok(image)
    }

    pub(in crate::container_storage) fn require_named(&self, root: &DiskRoot) -> Result<()> {
        Self::reject_sidecars(root)?;
        self.held.check_running()?;
        let file = root.open_read_regular(&root.path().join(NAME))?;
        let metadata = file.metadata()?;
        let identity = FileIdentity::from_metadata(&metadata);
        let stamp = self.held.stamp();
        if crate::file_stamp::FileStamp::from_metadata(&metadata) != self.before
            || stamp.inode != identity.inode
            || stamp.device != identity.device
        {
            return Err(invalid("FILE image changed during physical readback"));
        }
        self.held.check_running()
    }

    fn reject_sidecars(root: &DiskRoot) -> Result<()> {
        for suffix in ["-wal", "-shm", "-journal"] {
            if root
                .optional_metadata(&root.path().join(format!("{NAME}{suffix}")))?
                .is_some()
            {
                return Err(invalid("FILE image has an unadmitted SQLite sidecar"));
            }
        }
        Ok(())
    }
}

/// Facts of one original physical SQLite image. The file/storage names, a
/// valid local header and this type alone cannot establish a native frontier.
pub(crate) struct ResidentFileImage {
    handle: HeldFileImage,
    pub(crate) file: storage_contract::FileId,
    pub(crate) history: storage_contract::HistoryTreeId,
    pub(crate) sequence: u64,
    pub(crate) records: storage_contract::ContentHash,
}

impl ResidentFileImage {
    pub(crate) fn read(
        holder: &crate::container_storage::ObservedContainerReplica,
        file: storage_contract::FileId, history: storage_contract::HistoryTreeId,
    ) -> Result<Self> {
        let root = holder.container()?;
        let handle = HeldFileImage::open(root)?;
        // Read the actual SQLite pages and run the full native FILE owner,
        // record, closure, integrity and schema checks. Do not re-encode a
        // current State and guess that identical rows imply identical bytes.
        let rows = super::verify_snapshot(handle.held.bytes())?;
        if rows.file.parse::<storage_contract::FileId>()? != file
            || rows.history.parse::<storage_contract::HistoryTreeId>()? != history
        {
            return Err(invalid("resident FILE is not the selected file/history"));
        }
        handle.require_named(holder.container()?)?;
        let records = rows.records_hash.parse().map_err(|_| invalid("invalid verified FILE record commitment"))?;
        Ok(Self { handle, file, history, sequence: rows.sequence, records })
    }

    pub(crate) fn bytes(&self) -> &[u8] { self.handle.held.bytes() }

    pub(crate) fn require_named(&self, holder: &crate::container_storage::ObservedContainerReplica) -> Result<()> {
        self.handle.require_named(holder.container()?)
    }
}
