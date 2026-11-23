//! Directed whole-container move from ordinary storage to recycle storage.
//! A small SYSTEM receipt selects exactly one qualified R installation.
use super::{
    Change, DataRoot, Error, FlatStage, Id, Mutation, State, entry_leaf, hex, invalid,
    recycle_material, volume::DataVolume,
};
use crate::{
    fs::{HeldDir, OPAQUE_STAMP_BYTES},
    hash::Sha256,
};
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString},
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const BYTES: usize = 108;
const RETIRED_BYTES: usize = 52;

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
    fn read(parent: &HeldDir, name: &CStr) -> io::Result<Option<Self>> {
        let Some(original) = parent.opaque_pin(name)? else {
            parent.require_named()?;
            return Ok(None);
        };
        if original.file.metadata()?.len() != BYTES as u64 {
            return Err(invalid("invalid recycle receipt size"));
        }
        let mut bytes = [0; BYTES];
        (&original.file).read_exact(&mut bytes)?;
        original.require(parent, name)?;
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
        Ok(Some(receipt))
    }
}

/// Holds the SYSTEM and recycle namespaces under Store's service leases.
/// Selected receipts never require opening the former DATA container.
pub(super) struct Recycle {
    system: HeldDir,
    pool: Id,
    archive_root: HeldDir,
    arrived: PathBuf,
    moves_directory: HeldDir,
    staging_directory: HeldDir,
    arrived_directory: HeldDir,
    _recycle_owner: File,
    receipt_fence: Mutex<()>,
    unsure: AtomicBool,
}

impl Recycle {
    pub(super) fn require_roles(
        system: &HeldDir,
        recycle: &HeldDir,
        data: &[DataRoot],
    ) -> io::Result<()> {
        let roles = [system, recycle]
            .into_iter()
            .chain(data.iter().map(|volume| &volume.root))
            .collect::<Vec<_>>();
        for (i, role) in roles.iter().enumerate() {
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
            if roles[..i].iter().any(|earlier| {
                role.path.starts_with(&earlier.path)
                    || earlier.path.starts_with(&role.path)
                    || role.entity == earlier.entity
            }) {
                return Err(invalid("storage role paths overlap"));
            }
        }
        let separate = |a: &HeldDir, b: &HeldDir| {
            Ok::<_, io::Error>(a.entity.device != b.entity.device && !a.same_mount(b)?)
        };
        if !separate(system, recycle)? {
            return Err(invalid("recycle requires a separate filesystem"));
        }
        for (i, volume) in data.iter().enumerate() {
            if !separate(recycle, &volume.root)? {
                return Err(invalid("recycle requires a separate filesystem"));
            }
            for earlier in &data[..i] {
                if !separate(&volume.root, &earlier.root)? {
                    return Err(invalid("enrolled DATA filesystems overlap"));
                }
            }
        }
        Ok(())
    }

    /// The caller already retains the original SYSTEM owner.
    pub(super) fn open(system: HeldDir, recycle: HeldDir, pool: Id) -> io::Result<Self> {
        let owner = recycle.opaque_owner(c"container-owner")?;
        let moves_directory = system.opaque_subdirectory(c"container-moves")?;
        let staging_directory = recycle.opaque_subdirectory(c"incoming-containers")?;
        let arrived_directory = recycle.opaque_subdirectory(c"containers")?;
        // A former service can stop after the selector rename but before
        // syncing its directory. Qualify that namespace before any cold
        // read or retirement; the previous process's unsure bit is gone.
        moves_directory.require_named()?;
        moves_directory.file.sync_all()?;
        moves_directory.require_named()?;
        let arrived = arrived_directory.path.clone();
        system.require_named()?;
        recycle.require_named()?;
        Ok(Self {
            system,
            pool,
            archive_root: recycle,
            arrived,
            _recycle_owner: owner,
            moves_directory,
            staging_directory,
            arrived_directory,
            receipt_fence: Mutex::new(()),
            unsure: AtomicBool::new(false),
        })
    }

    pub(super) fn staging(&self) -> io::Result<&HeldDir> {
        self.held()?;
        Ok(&self.staging_directory)
    }
    pub(super) fn moves(&self) -> io::Result<&HeldDir> {
        self.system_held()?;
        Ok(&self.moves_directory)
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
        self.arrived.join(arrived_leaf(mutation))
    }

    fn held_arrived(&self, mutation: Mutation) -> io::Result<Option<HeldDir>> {
        let name = CString::new(arrived_leaf(mutation))
            .map_err(|_| invalid("invalid recycle position"))?;
        self.arrived_directory.require_named()?;
        let held = self.arrived_directory.child_directory(&name)?;
        self.arrived_directory.require_named()?;
        Ok(held)
    }

    fn qualify(&self, ready: RecycleReceipt) -> io::Result<HeldDir> {
        self.held()?;
        let selected = self
            .held_arrived(ready.mutation)?
            .ok_or_else(|| invalid("selected recycle directory is absent"))?;
        let known = selected.file.metadata()?;
        if (known.dev(), known.ino()) != (ready.recycle_device, ready.recycle_inode)
            || RecycleReceipt::read(&selected, c"ready")? != Some(ready)
        {
            return Err(invalid(
                "recycle location differs from its original ready receipt",
            ));
        }
        selected
            .child_directory(c"container")?
            .ok_or_else(|| invalid("selected recycled container is absent"))
    }

    pub(super) fn selected(&self, id: Id) -> io::Result<Option<(RecycleReceipt, HeldDir)>> {
        self.system_held()?;
        let _cut = self
            .receipt_fence
            .lock()
            .map_err(|_| invalid("recycle receipt cut unavailable"))?;
        if self.unsure.load(Ordering::Acquire) {
            self.moves_directory.file.sync_all()?;
            self.unsure.store(false, Ordering::Release);
        }
        let name = CString::new(hex(id)).map_err(|_| invalid("invalid recycle selector name"))?;
        let Some(ready) = RecycleReceipt::read(&self.moves_directory, &name)? else {
            return Ok(None);
        };
        if ready.mutation.container != id {
            return Err(invalid("SYSTEM selected a different recycle container"));
        }
        Ok(Some((ready, self.qualify(ready)?)))
    }

    pub(super) fn prepare(
        &self,
        mutation: Mutation,
        ordinary: &DataVolume,
        state: &State,
        candidate: &mut FlatStage<'_>,
        live: impl Fn() -> io::Result<()>,
    ) -> Result<RecycleReceipt, Error> {
        self.held()?;
        live()?;
        let held = ordinary
            .held_container(mutation.container)?
            .ok_or_else(|| invalid("original container is absent from its routed DATA"))?;
        let before = held.file.metadata()?;
        let destination = CString::new(arrived_leaf(mutation))
            .map_err(|_| invalid("invalid recycle arrival position"))?;
        if let Some(arrived) = self.held_arrived(mutation)? {
            let old = RecycleReceipt::read(&arrived, c"ready")?.ok_or_else(|| {
                invalid("recycle position is occupied without matching ready proof")
            })?;
            if (
                old.mutation,
                old.original_sequence,
                old.source_device,
                old.source_inode,
            ) != (mutation, state.sequence, before.dev(), before.ino())
            {
                return Err(super::Conflict::RequestReused.into());
            }
            self.qualify(old)?;
            self.arrived_directory.file.sync_all()?;
            self.staging_directory.file.sync_all()?;
            return Ok(old);
        }
        let mut tree = recycle_material::CopyTree::new(&candidate.directory)?;
        let evidence = candidate.create(c"source-proof")?;
        let source_proof =
            recycle_material::copy(&state.receipts, &held, &mut tree, evidence, || {
                self.held()?;
                ordinary.require_named()?;
                held.require_named()?;
                live()
            })?;
        let into = candidate.directory.file.metadata()?;
        let proof = RecycleReceipt {
            mutation,
            original_sequence: state.sequence,
            source_device: before.dev(),
            source_inode: before.ino(),
            recycle_device: into.dev(),
            recycle_inode: into.ino(),
            source_proof,
        };
        candidate.write(c"ready", &proof.encode())?;
        candidate.directory.file.sync_all()?;
        self.held()?;
        live()?;
        held.require_named()?;
        let after = held.file.metadata()?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(invalid("source container changed while recycling").into());
        }
        tree.require_named()?;
        candidate.publish_directory(&self.arrived_directory, &destination, live)?;
        self.arrived_directory.file.sync_all()?;
        self.staging_directory.file.sync_all()?;
        self.qualify(proof)?;
        Ok(proof)
    }

    pub(super) fn choose(
        &self,
        receipt: RecycleReceipt,
        candidate: &mut FlatStage<'_>,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), Error> {
        let _cut = self
            .receipt_fence
            .lock()
            .map_err(|_| invalid("recycle receipt cut unavailable"))?;
        self.qualify(receipt)?;
        candidate.write(c"selector", &receipt.encode())?;
        let selected = CString::new(hex(receipt.mutation.container))
            .map_err(|_| invalid("invalid recycle selector name"))?;
        self.held()?;
        candidate.publish_record(c"selector", &self.moves_directory, &selected, authorize)?;
        if let Err(e) = self.moves_directory.file.sync_all() {
            self.unsure.store(true, Ordering::Release);
            return Err(Error::Uncertain(e));
        }
        Ok(())
    }

    pub(super) fn require_selected(
        &self,
        receipt: RecycleReceipt,
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
        Ok(())
    }

    /// The offline owner publishes this record before removing the disk
    /// from the roster. The caller checks that the original token is absent.
    pub(super) fn retired_data(&self, token: [u8; 32]) -> io::Result<bool> {
        self.system.require_named()?;
        let Some(directory) = self.system.child_directory(c"retired-data")? else {
            return Ok(false);
        };
        let name = CString::new(hex(token)).map_err(|_| invalid("invalid retired DATA name"))?;
        let Some(pin) = directory.opaque_pin(&name)? else {
            return Ok(false);
        };
        if pin.file.metadata()?.len() != RETIRED_BYTES as u64 {
            return Err(invalid("invalid retired DATA record size"));
        }
        let mut raw = [0; RETIRED_BYTES];
        (&pin.file).read_exact(&mut raw)?;
        if &raw[..4] != b"DT1\0" || raw[4..20] != self.pool || raw[20..] != token {
            return Err(invalid(
                "retired DATA record differs from its original pool or disk",
            ));
        }
        directory.file.sync_all()?;
        self.system.file.sync_all()?;
        pin.require(&directory, &name)?;
        self.system.require_named()?;
        Ok(true)
    }

    /// Validate the full original metadata proof before the first unlink.
    /// The per-entry retirement step re-pins its exact names under the same
    /// exclusive source lease. No node payload is opened for content.
    /// The store's service-owner lock and private tree admit no other
    /// cooperating writer between the final relative name check and unlink.
    pub(super) fn retire(
        &self,
        receipt: RecycleReceipt,
        ordinary: &DataVolume,
        known: &State,
    ) -> io::Result<()> {
        self.require_selected(receipt, known)?;
        let parent = ordinary.contents();
        let leaf = CString::new(hex(receipt.mutation.container))
            .map_err(|_| invalid("invalid original container name"))?;
        let Some(original) = ordinary.held_container(receipt.mutation.container)? else {
            return parent.confirm_absence(&leaf);
        };
        if (original.entity.device, original.entity.inode)
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
fn arrived_leaf(mutation: Mutation) -> String {
    format!("{}-{}", hex(mutation.container), hex(mutation.request))
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
