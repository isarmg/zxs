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
        Self::reject_sidecars(root)?;
        let path = root.path().join(NAME);
        let metadata = root
            .optional_metadata(&path)?
            .ok_or_else(|| invalid("required FILE image is missing"))?;
        let before = crate::file_stamp::FileStamp::from_metadata(&metadata);
        let held = HeldRegular::capture(root, NAME, MAX_BYTES)?
            .ok_or_else(|| invalid("required FILE image is missing"))?;
        // Byte identity belongs to the captured material, not any later
        // re-encoding of equal logical rows. Always parse the actual image.
        if held.bytes() != expected.bytes() {
            return Err(invalid(
                "FILE image differs from its captured physical bytes",
            ));
        }
        expected.verify(held.bytes())?;
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
