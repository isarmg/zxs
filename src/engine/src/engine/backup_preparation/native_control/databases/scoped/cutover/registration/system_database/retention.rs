//! Exact old files, retained without opening SQLite or checkpointing old WAL.
//! Every restart classifies actual names; it never blindly repeats a rename.
use super::*;
use crate::disk::{FileStamp, HeldRegular};
use crate::engine::index::INDEX_FILES;

const RETAINED: &str = "backup-preparation/system-index-displaced";
const FORMAT: &str = "zxs-backup-system-index-selection";

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    format: String,
    binding_hash: String,
    retained: PhysicalBinding,
    old: [Option<FileStamp>; 4],
}
pub(super) struct Selected {
    selection: Selection,
    bytes: Vec<u8>,
    hash: String,
}
impl Selected {
    pub(super) fn read(system: &DiskRoot, plan: &Plan) -> Result<Self> {
        let bytes = system.read_bytes(&system.path().join(SELECTION), MAX_DESCRIPTOR)?;
        let selection: Selection = storage_contract::decode_strict(&bytes)?;
        if selection.format != FORMAT
            || selection.binding_hash != plan.binding_hash
            || canonical_json(&selection)? != bytes
        {
            return Err(invalid(
                "SYSTEM selection is not this original native installation",
            ));
        }
        let device = system.directory_identity(system.path())?.device;
        for stamp in selection.old.iter().flatten() {
            if stamp.bytes > MAX_DATABASE as u64 || stamp.device != device {
                return Err(invalid(
                    "retained SYSTEM file exceeds the original device or byte bound",
                ));
            }
            blake3::Hash::from_hex(&stamp.digest)?;
        }
        let selected = Self {
            hash: blake3::hash(&bytes).to_hex().to_string(),
            selection,
            bytes,
        };
        selected.directory(system)?;
        Ok(selected)
    }
    pub(super) fn capture(system: &DiskRoot, plan: &Plan, stop: &AtomicBool) -> Result<Self> {
        if preparation_path_exists(system, &system.path().join(SELECTION))? {
            return Self::read(system, plan);
        }
        let path = system.path().join(RETAINED);
        system.private_dir(&path)?;
        let directory = system.private_child(&path)?;
        if let Some(entry) = directory.entries(directory.path())?.next() {
            entry?;
            return Err(invalid(
                "unselected SYSTEM retention directory is not empty; all material preserved",
            ));
        }
        let mut old: [Option<FileStamp>; 4] = std::array::from_fn(|_| None);
        for (index, name) in INDEX_FILES.iter().enumerate() {
            preparation_checkpoint(stop)?;
            if let Some(file) = HeldRegular::capture(system, name, MAX_DATABASE)? {
                file.check_running()?;
                old[index] = Some(file.stamp().clone());
            }
        }
        let selection = Selection {
            format: FORMAT.into(),
            binding_hash: plan.binding_hash.clone(),
            retained: PhysicalBinding::of(&directory)?,
            old,
        };
        let bytes = canonical_json(&selection)?;
        if bytes.len() > MAX_DESCRIPTOR {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "SYSTEM selection exceeds 128 KiB",
            ));
        }
        // This denial marker is durable BEFORE any old name moves. Cold Engine
        // admission must skip index rebuild, even after a partial sidecar move.
        install_preparation_bytes(system, &system.path().join(SELECTION), &bytes)?;
        Self::read(system, plan)
    }
    fn directory(&self, system: &DiskRoot) -> Result<DiskRoot> {
        let root = system.private_child(&system.path().join(RETAINED))?;
        if PhysicalBinding::of(&root)? != self.selection.retained {
            return Err(invalid(
                "original SYSTEM retention directory identity changed",
            ));
        }
        Ok(root)
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(super) fn hash(&self) -> &str {
        &self.hash
    }
    pub(super) fn retained_files(&self) -> usize {
        self.selection.old.iter().flatten().count()
    }
    fn require_selection(&self, system: &DiskRoot) -> Result<()> {
        if system.read_bytes(&system.path().join(SELECTION), MAX_DESCRIPTOR)? != self.bytes {
            return Err(invalid(
                "SYSTEM selection changed while retaining old SQLite",
            ));
        }
        Ok(())
    }
    fn require_live_after_move(name: &str, live: Option<&HeldRegular>, plan: &Plan) -> Result<()> {
        if let Some(live) = live {
            if name != "index.sqlite" || live.bytes() != plan.database {
                return Err(invalid(
                    "unexpected ordinary SYSTEM file retained; no overwrite or checkpoint",
                ));
            }
            live.check_running()?;
        }
        Ok(())
    }
    pub(super) fn preserve_old(
        &self,
        system: &DiskRoot,
        plan: &Plan,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_selection(system)?;
        let destination = self.directory(system)?;
        for (index, name) in INDEX_FILES.iter().enumerate() {
            preparation_checkpoint(stop)?;
            let live = HeldRegular::capture(system, name, MAX_DATABASE)?;
            let retained = HeldRegular::capture(&destination, name, MAX_DATABASE)?;
            let old = self.selection.old[index].as_ref();
            if retained.as_ref().map(HeldRegular::stamp) == old {
                Self::require_live_after_move(name, live.as_ref(), plan)?;
                // Also finish an interrupted rename's two durability tails.
                destination.sync_dir(destination.path())?;
                system.sync_dir(system.path())?;
                continue;
            }
            if retained.is_some() || old.is_none() || live.as_ref().map(HeldRegular::stamp) != old {
                return Err(invalid(
                    "SYSTEM old-file identity or bytes differ from the persisted original selection",
                ));
            }
            live.as_ref()
                .expect("invariant: matched old file exists")
                .check_running()?;
            system.rename_noreplace_destination_first_synced(
                &system.path().join(name),
                &destination.path().join(name),
            )?;
            let moved = HeldRegular::capture(&destination, name, MAX_DATABASE)?;
            if moved.as_ref().map(HeldRegular::stamp) != old
                || HeldRegular::capture(system, name, MAX_DATABASE)?.is_some()
            {
                return Err(invalid(
                    "SYSTEM preservation encountered changed names; displaced bytes retained",
                ));
            }
            self.directory(system)?;
        }
        self.require_selection(system)
    }
    pub(super) fn require_retained(
        &self,
        system: &DiskRoot,
        plan: &Plan,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_selection(system)?;
        let directory = self.directory(system)?;
        for (index, name) in INDEX_FILES.iter().enumerate() {
            preparation_checkpoint(stop)?;
            let file = HeldRegular::capture(&directory, name, MAX_DATABASE)?;
            if file.as_ref().map(HeldRegular::stamp) != self.selection.old[index].as_ref() {
                return Err(invalid(
                    "original SYSTEM main/WAL/SHM/journal retention is incomplete",
                ));
            }
            if let Some(file) = &file {
                file.check_running()?;
            }
            let live = HeldRegular::capture(system, name, MAX_DATABASE)?;
            Self::require_live_after_move(name, live.as_ref(), plan)?;
        }
        Ok(())
    }
    pub(super) fn require_installed(
        &self,
        system: &DiskRoot,
        plan: &Plan,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_retained(system, plan, stop)?;
        let file = HeldRegular::capture(system, "index.sqlite", MAX_DATABASE)?
            .ok_or_else(|| invalid("published ordinary SYSTEM database is absent"))?;
        if file.bytes() != plan.database {
            return Err(invalid(
                "ordinary SYSTEM database differs from actual publication",
            ));
        }
        file.check_running()
    }
    pub(super) fn require_complete(
        &self,
        system: &DiskRoot,
        plan: &Plan,
        stop: &AtomicBool,
    ) -> Result<()> {
        if system.read_bytes(&system.path().join(COMPLETE), MAX_DESCRIPTOR)? != self.bytes {
            return Err(invalid(
                "SYSTEM completion differs from original retained selection",
            ));
        }
        self.require_installed(system, plan, stop)
    }
}
