//! Directed whole-container move from ordinary storage to recycle storage.
//! A small SYSTEM receipt selects exactly one qualified R installation.
use super::{
    Change, Error, Id, Mutation, Stage, State, create_file, ensure_dir, entry_leaf, hex, invalid,
    open_file, private_dir, recycle_material, sync_dir, write_file,
};
use crate::{
    fs::{HeldDir, OPAQUE_STAMP_BYTES},
    hash::Sha256,
};
use std::{
    collections::BTreeSet,
    ffi::CString,
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const BYTES: usize = 108;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecycleReceipt {
    pub(crate) mutation: Mutation,
    pub(crate) original_sequence: u64,
    source_device: u64,
    source_inode: u64,
    recycle_device: u64,
    recycle_inode: u64,
    source_proof: [u8; 32],
}

impl RecycleReceipt {
    fn encode(self) -> [u8; BYTES] {
        let mut value = [0; BYTES];
        value[..4].copy_from_slice(b"RC1\0");
        value[4..20].copy_from_slice(&self.mutation.container);
        value[20..36].copy_from_slice(&self.mutation.request);
        for (i, field) in [
            self.original_sequence,
            self.source_device,
            self.source_inode,
            self.recycle_device,
            self.recycle_inode,
        ]
        .into_iter()
        .enumerate()
        {
            value[36 + i * 8..44 + i * 8].copy_from_slice(&field.to_be_bytes());
        }
        value[76..108].copy_from_slice(&self.source_proof);
        value
    }
    fn read(path: &Path) -> io::Result<Self> {
        let mut file = open_file(path)?;
        if file.metadata()?.len() != BYTES as u64 {
            return Err(invalid("invalid recycle receipt size"));
        }
        let mut bytes = [0; BYTES];
        file.read_exact(&mut bytes)?;
        if &bytes[..4] != b"RC1\0" {
            return Err(invalid("unknown recycle receipt"));
        }
        let id =
            |i| <[u8; 16]>::try_from(&bytes[i..i + 16]).map_err(|_| invalid("invalid recycle ID"));
        let word = |i| {
            <[u8; 8]>::try_from(&bytes[i..i + 8])
                .map(u64::from_be_bytes)
                .map_err(|_| invalid("invalid recycle coordinate"))
        };
        let receipt = Self {
            mutation: Mutation {
                container: id(4)?,
                request: id(20)?,
            },
            original_sequence: word(36)?,
            source_device: word(44)?,
            source_inode: word(52)?,
            recycle_device: word(60)?,
            recycle_inode: word(68)?,
            source_proof: bytes[76..108]
                .try_into()
                .map_err(|_| invalid("invalid source proof identity"))?,
        };
        if receipt.mutation.container == [0; 16]
            || receipt.mutation.request == [0; 16]
            || receipt.original_sequence == 0
            || receipt.source_device == receipt.recycle_device
        {
            return Err(invalid("invalid recycle transaction identity"));
        }
        Ok(receipt)
    }
}

/// Exclusive service custody of the two non-payload authorities used after
/// selection. Chosen receipts never require opening the former F container.
pub(super) struct Recycle {
    system: HeldDir,
    archive_root: HeldDir,
    moves: PathBuf,
    staging: PathBuf,
    arrived: PathBuf,
    moves_directory: HeldDir,
    staging_directory: HeldDir,
    arrived_directory: HeldDir,
    _owners: [File; 2],
    receipt_fence: Mutex<()>,
    unsure: AtomicBool,
}

impl Recycle {
    pub(super) fn require_roles(
        system: &HeldDir,
        recycle: &HeldDir,
        data: &HeldDir,
    ) -> io::Result<()> {
        for role in [system, recycle, data] {
            role.require_named()?;
            let meta = role.file.metadata()?;
            if !meta.is_dir()
                || meta.uid() != crate::fs::local_uid()
                || meta.mode() & 0o7777 != 0o700
            {
                return Err(invalid(
                    "enrolled role is not a held private service directory",
                ));
            }
        }
        for (a, b) in [(system, recycle), (system, data), (recycle, data)] {
            if a.path.starts_with(&b.path) || b.path.starts_with(&a.path) || a.entity == b.entity {
                return Err(invalid("storage role paths overlap"));
            }
        }
        if recycle.same_mount(data)? || recycle.entity.device == data.entity.device {
            return Err(invalid("recycle requires a separate filesystem"));
        }
        Ok(())
    }

    pub(super) fn open(system: HeldDir, recycle: HeldDir, data: &HeldDir) -> io::Result<Self> {
        Self::require_roles(&system, &recycle, data)?;
        let owners = [
            system.opaque_owner(c"container-owner")?,
            recycle.opaque_owner(c"container-owner")?,
        ];
        let moves_directory = system.opaque_subdirectory(c"container-moves")?;
        let staging_directory = recycle.opaque_subdirectory(c"incoming-containers")?;
        let arrived_directory = recycle.opaque_subdirectory(c"containers")?;
        // A former service can stop after the selector rename but before
        // syncing its directory. Qualify that namespace before any cold
        // read or retirement; the previous process's unsure bit is gone.
        moves_directory.require_named()?;
        moves_directory.file.sync_all()?;
        moves_directory.require_named()?;
        let moves = moves_directory.path.clone();
        let staging = staging_directory.path.clone();
        let arrived = arrived_directory.path.clone();
        system.require_named()?;
        recycle.require_named()?;
        Ok(Self {
            system,
            archive_root: recycle,
            moves,
            staging,
            arrived,
            _owners: owners,
            moves_directory,
            staging_directory,
            arrived_directory,
            receipt_fence: Mutex::new(()),
            unsure: AtomicBool::new(false),
        })
    }

    pub(super) fn staging(&self) -> io::Result<&Path> {
        self.held()?;
        Ok(&self.staging)
    }
    pub(super) fn moves(&self) -> io::Result<&Path> {
        self.system_held()?;
        Ok(&self.moves)
    }

    fn system_held(&self) -> io::Result<()> {
        self.system.require_named()?;
        self.moves_directory.require_named()
    }

    fn held(&self) -> io::Result<()> {
        self.system_held()?;
        self.archive_root.require_named()?;
        self.staging_directory.require_named()?;
        self.arrived_directory.require_named()
    }
    fn arrived_dir(&self, mutation: Mutation) -> PathBuf {
        // A ready but unselected copy is private to its original operation.
        // A later operation must not adopt or be blocked by that copy.
        self.arrived.join(format!(
            "{}-{}",
            hex(mutation.container),
            hex(mutation.request)
        ))
    }
    fn qualify(&self, ready: RecycleReceipt, directory: &Path) -> io::Result<PathBuf> {
        self.held()?;
        private_dir(directory)?;
        let selected = HeldDir::trusted_storage(directory)?;
        let known = selected.file.metadata()?;
        if !self.archive_root.same_mount(&selected)? {
            return Err(invalid("selected recycle data crosses its enrolled mount"));
        }
        if (known.dev(), known.ino()) != (ready.recycle_device, ready.recycle_inode)
            || RecycleReceipt::read(&directory.join("ready"))? != ready
        {
            return Err(invalid(
                "recycle location differs from its original ready receipt",
            ));
        }
        let payload = directory.join("container");
        let copy = HeldDir::trusted_storage(&payload)?;
        if !selected.same_mount(&copy)? {
            return Err(invalid("recycle body crosses its original parent mount"));
        }
        Ok(payload)
    }

    pub(super) fn selected(&self, id: Id) -> io::Result<Option<(RecycleReceipt, PathBuf)>> {
        self.system_held()?;
        let _cut = self
            .receipt_fence
            .lock()
            .map_err(|_| invalid("recycle receipt cut unavailable"))?;
        if self.unsure.load(Ordering::Acquire) {
            self.moves_directory.file.sync_all()?;
            self.unsure.store(false, Ordering::Release);
        }
        match RecycleReceipt::read(&self.moves.join(hex(id))) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
            Ok(ready) => {
                if ready.mutation.container != id {
                    return Err(invalid("SYSTEM selected a different recycle container"));
                }
                Ok(Some((
                    ready,
                    self.qualify(ready, &self.arrived_dir(ready.mutation))?,
                )))
            }
        }
    }

    pub(super) fn prepare(
        &self,
        mutation: Mutation,
        ordinary: &HeldDir,
        original: &Path,
        state: &State,
        candidate: &Stage,
        live: impl Fn() -> io::Result<()>,
    ) -> Result<RecycleReceipt, Error> {
        self.held()?;
        live()?;
        let held = HeldDir::trusted_storage(original)?;
        if !ordinary.same_mount(&held)? {
            return Err(invalid("original container crosses its enrolled F mount").into());
        }
        let before = held.file.metadata()?;
        let destination = self.arrived_dir(mutation);
        match RecycleReceipt::read(&destination.join("ready")) {
            Ok(old) => {
                if (
                    old.mutation,
                    old.original_sequence,
                    old.source_device,
                    old.source_inode,
                ) != (mutation, state.sequence, before.dev(), before.ino())
                {
                    return Err(super::Conflict::RequestReused.into());
                }
                self.qualify(old, &destination)?;
                self.arrived_directory.file.sync_all()?;
                self.staging_directory.file.sync_all()?;
                return Ok(old);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        if !matches!(fs::symlink_metadata(&destination),Err(e) if e.kind()==io::ErrorKind::NotFound)
        {
            return Err(
                invalid("recycle position is occupied without matching ready proof").into(),
            );
        }
        let data = candidate.path.join("container");
        if !ensure_dir(&data)? {
            return Err(invalid("recycle staging is not private and fresh").into());
        }
        let target = HeldDir::trusted_storage(&candidate.path)?;
        let target_data = HeldDir::trusted_storage(&data)?;
        if !self.archive_root.same_mount(&target)? || !target.same_mount(&target_data)? {
            return Err(invalid("recycle candidate crossed its enrolled R mount").into());
        }
        let mut evidence = create_file(&candidate.path.join("source-proof"))?;
        let source_proof =
            recycle_material::copy(&state.receipts, &held, &data, &mut evidence, || {
                self.held()?;
                ordinary.require_named()?;
                held.require_named()?;
                target.require_named()?;
                live()
            })?;
        let into = target.file.metadata()?;
        let proof = RecycleReceipt {
            mutation,
            original_sequence: state.sequence,
            source_device: before.dev(),
            source_inode: before.ino(),
            recycle_device: into.dev(),
            recycle_inode: into.ino(),
            source_proof,
        };
        write_file(&candidate.path.join("ready"), &proof.encode())?;
        sync_dir(&candidate.path)?;
        self.held()?;
        live()?;
        held.require_named()?;
        let after = held.file.metadata()?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(invalid("source container changed while recycling").into());
        }
        fs::rename(&candidate.path, &destination)?;
        self.arrived_directory.file.sync_all()?;
        self.staging_directory.file.sync_all()?;
        self.qualify(proof, &destination)?;
        Ok(proof)
    }

    pub(super) fn choose(
        &self,
        receipt: RecycleReceipt,
        candidate: &Stage,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), Error> {
        let _cut = self
            .receipt_fence
            .lock()
            .map_err(|_| invalid("recycle receipt cut unavailable"))?;
        self.qualify(receipt, &self.arrived_dir(receipt.mutation))?;
        let staged = candidate.path.join("selector");
        write_file(&staged, &receipt.encode())?;
        let selected = self.moves.join(hex(receipt.mutation.container));
        if !matches!(fs::symlink_metadata(&selected),Err(e) if e.kind()==io::ErrorKind::NotFound) {
            return Err(invalid("another recycle receipt is already selected").into());
        }
        authorize()?;
        self.held()?;
        fs::rename(staged, &selected)?;
        if let Err(e) = self.moves_directory.file.sync_all() {
            self.unsure.store(true, Ordering::Release);
            return Err(Error::Uncertain(e));
        }
        Ok(())
    }

    /// Validate the full original metadata proof before the first unlink.
    /// The per-entry retirement step re-pins its exact names under the same
    /// exclusive source lease. No node payload is opened for content.
    /// The store's service-owner lock and private tree admit no other
    /// cooperating writer between the final relative name check and unlink.
    pub(super) fn retire(
        &self,
        receipt: RecycleReceipt,
        ordinary: &HeldDir,
        source: &Path,
        known: &State,
    ) -> io::Result<()> {
        if self
            .selected(receipt.mutation.container)?
            .as_ref()
            .map(|(r, _)| *r)
            != Some(receipt)
            || known.sequence != receipt.original_sequence
        {
            return Err(invalid(
                "source cannot retire before exact recycle selection",
            ));
        }
        ordinary.require_named()?;
        let parent = HeldDir::trusted_storage(
            source
                .parent()
                .ok_or_else(|| invalid("missing source parent"))?,
        )?;
        let leaf = CString::new(hex(receipt.mutation.container))
            .map_err(|_| invalid("invalid original container name"))?;
        let Some(original) = parent.child_directory(&leaf)? else {
            return Ok(());
        };
        if !ordinary.same_mount(&parent)?
            || !parent.same_mount(&original)?
            || (original.entity.device, original.entity.inode)
                != (receipt.source_device, receipt.source_inode)
        {
            return Err(invalid(
                "source retirement no longer identifies the exact moved container",
            ));
        }
        let entries = known
            .receipts
            .values()
            .map(|r| entry_leaf(r.sequence, r.mutation.request))
            .collect::<BTreeSet<_>>();
        for item in fs::read_dir(&original.path)? {
            let name = item?.file_name();
            if !name.to_str().is_some_and(|s| entries.contains(s)) {
                return Err(invalid("unknown source descendant prevents retirement"));
            }
        }
        original.require_named()?;
        let proof_root = HeldDir::trusted_storage(&self.arrived_dir(receipt.mutation))?;
        if !self.archive_root.same_mount(&proof_root)? {
            return Err(invalid("retirement evidence crosses original R mount"));
        }
        let proof = proof_root
            .opaque_pin(c"source-proof")?
            .ok_or_else(|| invalid("retirement evidence is absent"))?;
        let mut input = &proof.file;
        let mut digest = Sha256::new();
        let mut rows = Vec::new();
        rows.try_reserve_exact(known.receipts.len())
            .map_err(|_| io::Error::other("cannot retain exact retirement metadata"))?;
        for saved in known.receipts.values() {
            let mut row = [0; recycle_material::PROOF_ROW];
            input.read_exact(&mut row)?;
            digest.update(&row)?;
            rows.push(row);
            retire_entry(&original, saved, &row, false)?;
        }
        let mut extra = [0; 1];
        if input.read(&mut extra)? != 0 || digest.finish() != receipt.source_proof {
            return Err(invalid(
                "source retirement evidence does not bind the exact moved copy",
            ));
        }
        proof.require(&proof_root, c"source-proof")?;
        // No unlink occurred before all rows and the exact source mount
        // were verified. Each old member is checked again at the syscall.
        for (saved, row) in known.receipts.values().zip(rows.iter()) {
            ordinary.require_named()?;
            retire_entry(&original, saved, row, true)?;
        }
        parent.unlink_empty_opaque_original(&leaf, &original)
    }
}
fn retire_entry(
    original: &HeldDir,
    receipt: &super::Receipt,
    row: &[u8; recycle_material::PROOF_ROW],
    remove: bool,
) -> io::Result<()> {
    let leaf = CString::new(entry_leaf(receipt.sequence, receipt.mutation.request))
        .map_err(|_| invalid("invalid source member label"))?;
    let Some(entry) = original.child_directory(&leaf)? else {
        return Ok(());
    };
    let dev = u64::from_be_bytes(
        row[..8]
            .try_into()
            .map_err(|_| invalid("invalid source member device"))?,
    );
    let ino = u64::from_be_bytes(
        row[8..16]
            .try_into()
            .map_err(|_| invalid("invalid source member inode"))?,
    );
    if (entry.entity.device, entry.entity.inode) != (dev, ino) || !original.same_mount(&entry)? {
        return Err(invalid(
            "source member is not the same copied original directory",
        ));
    }
    let body = matches!(receipt.change, Change::Save { .. });
    if entry
        .names()?
        .iter()
        .any(|n| n.as_slice() != b"receipt" && (!body || n.as_slice() != b"payload"))
    {
        return Err(invalid("unknown original member name prevents retirement"));
    }
    let record = entry.opaque_pin(c"receipt")?;
    let payload = if body {
        entry.opaque_pin(c"payload")?
    } else {
        None
    };
    if record
        .as_ref()
        .is_some_and(|f| f.stamp().as_slice() != &row[16..16 + OPAQUE_STAMP_BYTES])
        || payload
            .as_ref()
            .is_some_and(|f| f.stamp().as_slice() != &row[16 + OPAQUE_STAMP_BYTES..])
        || record.is_none() && payload.is_some()
    {
        return Err(invalid(
            "old source names no longer identify equal copied originals",
        ));
    }
    if remove {
        if let Some(body) = payload {
            entry.unlink_opaque_original(c"payload", &body)?;
        }
        // Payload unlink above is synchronized before receipt unlink, so
        // restart can distinguish an original partial cleanup safely.
        if let Some(record) = record {
            entry.unlink_opaque_original(c"receipt", &record)?;
        }
        original.unlink_empty_opaque_original(&leaf, &entry)?;
    }
    Ok(())
}
