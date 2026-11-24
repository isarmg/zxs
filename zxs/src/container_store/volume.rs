//! An original data root and its owner lease, selected data and intake.
use super::{DataRoot, Id, State, hex, invalid, read_state};
use crate::fs::HeldDir;
use std::{
    ffi::{CString, c_int},
    fs::{self, File},
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

unsafe extern "C" {
    fn zxs_data_available(fd: c_int, bytes: *mut u64) -> c_int;
}

pub(super) struct DataVolume {
    pub(super) token: [u8; 32],
    root: HeldDir,
    data: HeldDir,
    staging: HeldDir,
    owner: File,
}

impl DataVolume {
    pub(super) fn open(DataRoot { root, token }: DataRoot, fresh: bool) -> io::Result<Self> {
        root.require_named()?;
        let owner = root.opaque_owner(c"owner")?;
        let data = root.opaque_subdirectory(c"data")?;
        if fresh {
            data.only(None)?;
        }
        let staging = root.opaque_subdirectory(c"staging")?;
        discard_interrupted_intake(&root, &staging, &owner)?;
        root.require_named()?;
        Ok(Self {
            token,
            root,
            data,
            staging,
            owner,
        })
    }

    pub(super) fn open_existing(DataRoot { root, token }: DataRoot) -> io::Result<Self> {
        root.require_named()?;
        let owner = root.existing_opaque_owner(c"owner")?;
        let data = root
            .child_directory(c"data")?
            .ok_or_else(|| invalid("the existing DATA container namespace is absent"))?;
        let staging = root
            .child_directory(c"staging")?
            .ok_or_else(|| invalid("the existing DATA staging namespace is absent"))?;
        root.require_named()?;
        Ok(Self {
            token,
            root,
            data,
            staging,
            owner,
        })
    }

    pub(super) fn root(&self) -> &HeldDir {
        &self.root
    }

    pub(super) fn contents(&self) -> &HeldDir {
        &self.data
    }

    pub(super) fn bound_owner(&self) -> &File {
        &self.owner
    }

    #[cfg(test)]
    pub(super) fn upload_owner(&self) -> &File {
        self.bound_owner()
    }

    pub(super) fn staging(&self) -> &HeldDir {
        &self.staging
    }

    #[cfg(test)]
    pub(super) fn container(&self, id: Id) -> std::path::PathBuf {
        self.data.path.join(hex(id))
    }

    pub(super) fn held_container(&self, id: Id) -> io::Result<Option<HeldDir>> {
        self.require_named()?;
        let found = self.data.child_directory(&leaf(id)?)?;
        self.data.require_named()?;
        Ok(found)
    }

    pub(super) fn require_vacant(&self, id: Id) -> io::Result<()> {
        self.require_named()?;
        if self.data.pin_record(&leaf(id)?)?.is_some() {
            return Err(invalid("unrouted DATA container name is occupied"));
        }
        self.data.require_named()
    }

    pub(super) fn create_container(&self, id: Id) -> io::Result<HeldDir> {
        self.require_named()?;
        self.data.new_child_directory(&leaf(id)?)
    }

    pub(super) fn create_container_when(
        &self,
        id: Id,
        before: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<HeldDir> {
        self.require_named()?;
        self.data.new_child_directory_when(&leaf(id)?, before)
    }

    pub(super) fn read(&self, id: Id) -> io::Result<State> {
        let Some(container) = self.held_container(id)? else {
            return Ok(State::default());
        };
        read_state(&container, id)
    }

    pub(super) fn require_named(&self) -> io::Result<()> {
        self.root.require_named()?;
        self.data.require_named()?;
        self.staging.require_named()
    }

    pub(super) fn available_bytes(&self) -> io::Result<u64> {
        self.require_named()?;
        let mut bytes = 0;
        // SAFETY: the qualified staging FD stays held; bytes is a live u64.
        if unsafe { zxs_data_available(self.staging.file.as_raw_fd(), &raw mut bytes) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.require_named()?;
        Ok(bytes)
    }

    pub(super) fn sync_data(&self) -> io::Result<()> {
        self.data.file.sync_all()
    }

    pub(super) fn sync_staging(&self) -> io::Result<()> {
        self.staging.file.sync_all()
    }

    pub(super) fn load<'a>(
        &self,
        id: Id,
        memory: &'a mut Option<State>,
    ) -> io::Result<&'a mut State> {
        self.require_named()?;
        if memory.is_none() {
            // Covers process cuts and same-process post-rename sync errors.
            // Only metadata is read; previous complete node bodies aren't scanned.
            self.sync_staging()?;
            *memory = Some(self.read(id)?);
        }
        memory
            .as_mut()
            .ok_or_else(|| invalid("container state unavailable"))
    }
}

fn leaf(id: Id) -> io::Result<CString> {
    CString::new(hex(id)).map_err(|_| invalid("invalid DATA container name"))
}

/// The ordinary owner lease excludes new intake during this cold cleanup.
/// Recognize only the flat staging format; selected data and recycle roots
/// are not traversed. Never follow an unexpected member or read file bytes.
fn discard_interrupted_intake(root: &HeldDir, staging: &HeldDir, owner: &File) -> io::Result<()> {
    let original_lease = owner.metadata()?;
    let require_base = || {
        root.require_named()?;
        staging.require_named()?;
        let named = root
            .opaque_pin(c"owner")?
            .ok_or_else(|| invalid("intake owner lease lost its name"))?;
        let seen = named.file.metadata()?;
        if (original_lease.dev(), original_lease.ino()) != (seen.dev(), seen.ino())
            || !root.shares_file_mount(owner)?
        {
            return Err(invalid("intake owner lease was replaced"));
        }
        Ok(())
    };
    require_base()?;
    // Enumerate the still-held staging directory on Linux, not its former
    // pathname. The borrowed File outlives the streamed enumeration.
    let held_path = format!("/proc/self/fd/{}", staging.file.as_raw_fd());
    let mut changed = false;
    for item in fs::read_dir(held_path)? {
        let name = item?.file_name();
        let bytes = name.as_encoded_bytes();
        if bytes.len() != 16 || !bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            continue;
        }
        let leaf = CString::new(bytes).map_err(|_| invalid("invalid stage name"))?;
        let candidate = || {
            let Some(stage) = staging.child_directory(&leaf)? else {
                return Ok::<_, io::Error>(None);
            };
            if stage
                .names()?
                .iter()
                .any(|name| !matches!(name.as_slice(), b"payload" | b"receipt"))
            {
                return Ok(None);
            }
            // Prove the whole flat shape before unlinking either member.
            let payload = stage.opaque_pin(c"payload")?;
            let receipt = stage.opaque_pin(c"receipt")?;
            Ok(Some((stage, payload, receipt)))
        };
        let qualified = candidate();
        // Candidate-local foreign layouts (including symlinks, unsupported
        // mounts and non-regular members) are left alone. A displaced base
        // or original owner record always invalidates the entire cleanup.
        require_base()?;
        let Ok(Some((stage, payload, receipt))) = qualified else {
            continue;
        };
        if let Some(pin) = payload.as_ref() {
            stage.unlink_opaque_original(c"payload", pin)?;
        }
        if let Some(pin) = receipt.as_ref() {
            stage.unlink_opaque_original(c"receipt", pin)?;
        }
        staging.unlink_empty_opaque_original(&leaf, &stage)?;
        changed = true;
    }
    if changed {
        staging.file.sync_all()?;
    }
    require_base()
}
