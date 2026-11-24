//! Directed whole-container move from ordinary storage to recycle storage.
//! A small SYSTEM receipt selects exactly one qualified R installation.
pub(crate) use super::recycle_receipt::RecycleReceipt;
use super::{
    Change, DataRoot, Error, FlatStage, Id, Mutation, State, entry_leaf, hex, invalid, read_state,
    recycle_material::{self, ProofFormat, SourceRow},
    recycle_receipt::{ENROLLED_BYTES, LEGACY_BYTES, RecycleRole},
    volume::DataVolume,
};
use crate::{
    fs::{HeldDir, ReadAccess},
    hash::Sha256,
};
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString},
    fs::File,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const RETIRED_BYTES: usize = 52;

fn read_ready(
    parent: &HeldDir,
    name: &CStr,
    access: ReadAccess,
) -> io::Result<Option<RecycleReceipt>> {
    let Some(original) = parent.opaque_pin_with(name, access)? else {
        parent.require_named()?;
        return Ok(None);
    };
    let len = usize::try_from(original.file.metadata()?.len())
        .map_err(|_| invalid("invalid recycle receipt size"))?;
    if !matches!(len, LEGACY_BYTES | ENROLLED_BYTES) {
        return Err(invalid("invalid recycle receipt size"));
    }
    let mut bytes = [0; ENROLLED_BYTES];
    let raw = &mut bytes[..len];
    let mut input = &original.file;
    input.read_exact(raw)?;
    if input.read(&mut [0; 1])? != 0 {
        return Err(invalid("recycle receipt grew while reading"));
    }
    original.require(parent, name)?;
    RecycleReceipt::decode(raw).map(Some)
}

pub(super) fn read_selector(
    moves: &HeldDir,
    id: Id,
    access: ReadAccess,
) -> io::Result<Option<RecycleReceipt>> {
    let name = CString::new(hex(id)).map_err(|_| invalid("invalid recycle selector name"))?;
    let Some(ready) = read_ready(moves, &name, access)? else {
        return Ok(None);
    };
    if ready.mutation.container != id {
        return Err(invalid("SYSTEM selected a different recycle container"));
    }
    Ok(Some(ready))
}

fn held_arrived_at(arrived: &HeldDir, mutation: Mutation) -> io::Result<Option<HeldDir>> {
    let name =
        CString::new(arrived_leaf(mutation)).map_err(|_| invalid("invalid recycle position"))?;
    arrived.require_named()?;
    let held = arrived.child_directory(&name)?;
    arrived.require_named()?;
    Ok(held)
}

pub(super) fn qualify_arrival(
    arrived: &HeldDir,
    ready: RecycleReceipt,
    role: RecycleRole,
    access: ReadAccess,
) -> io::Result<HeldDir> {
    qualified_envelope(arrived, ready, role, access)?
        .child_directory(c"container")?
        .ok_or_else(|| invalid("selected recycled container is absent"))
}

fn qualified_envelope(
    arrived: &HeldDir,
    ready: RecycleReceipt,
    role: RecycleRole,
    access: ReadAccess,
) -> io::Result<HeldDir> {
    let selected = held_arrived_at(arrived, ready.mutation)?
        .ok_or_else(|| invalid("selected recycle directory is absent"))?;
    ready.require_arrival(role, selected.entity)?;
    if read_ready(&selected, c"ready", access)? != Some(ready) {
        return Err(invalid(
            "recycle location differs from its original ready receipt",
        ));
    }
    selected.require_named()?;
    Ok(selected)
}

pub(super) fn read_retired_data(
    system: &HeldDir,
    pool: Id,
    token: [u8; 32],
    access: ReadAccess,
) -> io::Result<bool> {
    system.require_named()?;
    let Some(directory) = system.child_directory(c"retired-data")? else {
        return Ok(false);
    };
    let name = CString::new(hex(token)).map_err(|_| invalid("invalid retired DATA name"))?;
    let Some(pin) = directory.opaque_pin_with(&name, access)? else {
        return Ok(false);
    };
    if pin.file.metadata()?.len() != RETIRED_BYTES as u64 {
        return Err(invalid("invalid retired DATA record size"));
    }
    let mut raw = [0; RETIRED_BYTES];
    (&pin.file).read_exact(&mut raw)?;
    if &raw[..4] != b"DT1\0" || raw[4..20] != pool || raw[20..] != token {
        return Err(invalid(
            "retired DATA record differs from its original pool or disk",
        ));
    }
    if matches!(access, ReadAccess::Ordinary) {
        directory.file.sync_all()?;
        system.file.sync_all()?;
    }
    pin.require(&directory, &name)?;
    system.require_named()?;
    Ok(true)
}

/// Holds the SYSTEM and recycle namespaces under Store's service leases.
/// Selected receipts never require opening the former DATA container.
pub(super) struct Recycle {
    system: HeldDir,
    role: RecycleRole,
    archive_root: HeldDir,
    moves_directory: HeldDir,
    staging_directory: HeldDir,
    arrived_directory: HeldDir,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "retains the native RECYCLE lease")
    )]
    archive_owner: File,
    receipt_fence: Mutex<()>,
    unsure: AtomicBool,
}

#[cfg(test)]
pub(super) struct IndexRoles<'a> {
    pub(super) system: &'a HeldDir,
    pub(super) root: &'a HeldDir,
    pub(super) owner: &'a File,
    pub(super) role: RecycleRole,
    pub(super) moves: &'a HeldDir,
    pub(super) staging: &'a HeldDir,
    pub(super) arrived: &'a HeldDir,
}

/// The chosen receipt and the exact R child that was copied or verified.
pub(super) struct PreparedRecycle {
    pub(super) receipt: RecycleReceipt,
    pub(super) at: HeldDir,
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
    pub(super) fn open(system: HeldDir, recycle: HeldDir, role: RecycleRole) -> io::Result<Self> {
        Self::select_namespaces(system, recycle, role, true)
    }

    pub(super) fn open_existing(
        system: HeldDir,
        recycle: HeldDir,
        role: RecycleRole,
    ) -> io::Result<Self> {
        Self::select_namespaces(system, recycle, role, false)
    }

    fn select_namespaces(
        system: HeldDir,
        recycle: HeldDir,
        role: RecycleRole,
        may_create: bool,
    ) -> io::Result<Self> {
        role.require()?;
        let owner = if may_create {
            recycle.opaque_owner(c"container-owner")?
        } else {
            recycle.existing_opaque_owner(c"container-owner")?
        };
        let directory = |parent: &HeldDir, name: &CStr| {
            if may_create {
                parent.opaque_subdirectory(name)
            } else {
                parent
                    .child_directory(name)?
                    .ok_or_else(|| invalid("the existing recycle namespace is absent"))
            }
        };
        let moves_directory = directory(&system, c"container-moves")?;
        let staging_directory = directory(&recycle, c"incoming-containers")?;
        let arrived_directory = directory(&recycle, c"containers")?;
        // A former service can stop after the selector rename but before
        // syncing its directory. Qualify that namespace before any cold
        // read or retirement; the previous process's unsure bit is gone.
        moves_directory.require_named()?;
        moves_directory.file.sync_all()?;
        moves_directory.require_named()?;
        system.require_named()?;
        recycle.require_named()?;
        Ok(Self {
            system,
            role,
            archive_root: recycle,
            archive_owner: owner,
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
    fn held_arrived(&self, mutation: Mutation) -> io::Result<Option<HeldDir>> {
        held_arrived_at(&self.arrived_directory, mutation)
    }

    fn qualify(&self, ready: RecycleReceipt) -> io::Result<HeldDir> {
        self.held()?;
        qualify_arrival(
            &self.arrived_directory,
            ready,
            self.role,
            ReadAccess::Ordinary,
        )
    }

    #[cfg(test)]
    pub(super) fn index_roles(&self) -> io::Result<IndexRoles<'_>> {
        let _cut = self
            .receipt_fence
            .lock()
            .map_err(|_| invalid("recycle receipt cut unavailable"))?;
        if self.unsure.load(Ordering::Acquire) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.held()?;
        Ok(IndexRoles {
            system: &self.system,
            root: &self.archive_root,
            owner: &self.archive_owner,
            role: self.role,
            moves: &self.moves_directory,
            staging: &self.staging_directory,
            arrived: &self.arrived_directory,
        })
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
        let Some(ready) = read_selector(&self.moves_directory, id, ReadAccess::Ordinary)? else {
            return Ok(None);
        };
        Ok(Some((ready, self.qualify(ready)?)))
    }

    pub(super) fn prepare(
        &self,
        mutation: Mutation,
        ordinary: &DataVolume,
        state: &State,
        candidate: &mut FlatStage<'_>,
        live: impl Fn() -> io::Result<()>,
    ) -> Result<PreparedRecycle, Error> {
        self.held()?;
        live()?;
        let held = ordinary
            .held_container(mutation.container)?
            .ok_or_else(|| invalid("original container is absent from its routed DATA"))?;
        let before = held.file.metadata()?;
        let destination = CString::new(arrived_leaf(mutation))
            .map_err(|_| invalid("invalid recycle arrival position"))?;
        if let Some(arrived) = self.held_arrived(mutation)? {
            let old = read_ready(&arrived, c"ready", ReadAccess::Ordinary)?.ok_or_else(|| {
                invalid("recycle position is occupied without matching ready proof")
            })?;
            if (old.mutation, old.original_sequence) != (mutation, state.sequence) {
                return Err(super::Conflict::RequestReused.into());
            }
            if !old.is_enrolled() {
                return Err(invalid(
                    "unselected legacy recycle material is preserved, not adopted",
                )
                .into());
            }
            old.require_source(ordinary.token, held.entity)?;
            let envelope = qualified_envelope(
                &self.arrived_directory,
                old,
                self.role,
                ReadAccess::Ordinary,
            )?;
            if envelope.entity != arrived.entity || !envelope.same_mount(&arrived)? {
                return Err(invalid("original recycle arrival was replaced").into());
            }
            let archive = envelope
                .child_directory(c"container")?
                .ok_or_else(|| invalid("selected recycled container is absent"))?;
            let exact = read_state(&archive, mutation.container)?;
            if exact.sequence != state.sequence || exact.receipts != state.receipts {
                return Err(invalid(
                    "prepared archive does not match the current original history",
                )
                .into());
            }
            recycle_material::verify_recovered(&exact.receipts, &archive, &live)?;
            prove_source(old, state, &held, &envelope, true)?;
            self.arrived_directory.file.sync_all()?;
            self.staging_directory.file.sync_all()?;
            archive.require_named()?;
            held.require_named()?;
            live()?;
            return Ok(PreparedRecycle {
                receipt: old,
                at: archive,
            });
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
        let proof = RecycleReceipt::enrolled(
            mutation,
            state.sequence,
            self.role,
            ordinary.token,
            (before.ino(), into.ino()),
            source_proof,
        )?;
        candidate.write(c"ready", &proof.encode())?;
        candidate.directory.file.sync_all()?;
        self.held()?;
        live()?;
        held.require_named()?;
        let after = held.file.metadata()?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(invalid("source container changed while recycling").into());
        }
        candidate.publish_directory(&self.arrived_directory, &destination, || {
            tree.require_named()?;
            live()
        })?;
        self.arrived_directory.file.sync_all()?;
        self.staging_directory.file.sync_all()?;
        let at = self.qualify(proof)?;
        tree.require_arrived(&at)?;
        Ok(PreparedRecycle { receipt: proof, at })
    }

    pub(super) fn choose(
        &self,
        prepared: &PreparedRecycle,
        candidate: &mut FlatStage<'_>,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), Error> {
        let receipt = prepared.receipt;
        let _cut = self
            .receipt_fence
            .lock()
            .map_err(|_| invalid("recycle receipt cut unavailable"))?;
        if !receipt.is_enrolled() {
            return Err(invalid("new recycle selection requires an enrolled-role receipt").into());
        }
        candidate.write(c"selector", &receipt.encode())?;
        let selected = CString::new(hex(receipt.mutation.container))
            .map_err(|_| invalid("invalid recycle selector name"))?;
        self.held()?;
        candidate.publish_record(c"selector", &self.moves_directory, &selected, || {
            let named = self.qualify(receipt)?;
            if named.entity != prepared.at.entity || !named.same_mount(&prepared.at)? {
                return Err(invalid("prepared recycle child changed before selection"));
            }
            prepared.at.require_named()?;
            authorize()
        })?;
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
        read_retired_data(&self.system, self.role.pool, token, ReadAccess::Ordinary)
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
        verify_copy: impl FnOnce() -> io::Result<()>,
        authorize: impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        self.require_selected(receipt, known)?;
        receipt.require_data(ordinary.token)?;
        let parent = ordinary.contents();
        let leaf = CString::new(hex(receipt.mutation.container))
            .map_err(|_| invalid("invalid original container name"))?;
        let Some(original) = ordinary.held_container(receipt.mutation.container)? else {
            authorize()?;
            return parent.confirm_absence(&leaf);
        };
        receipt.require_source(ordinary.token, original.entity)?;
        let proof_root = qualified_envelope(
            &self.arrived_directory,
            receipt,
            self.role,
            ReadAccess::Ordinary,
        )?;
        // A recovery verifies the archived bodies only when this exact source
        // is still present. The same held source is used through retirement.
        verify_copy()?;
        let rows = prove_source(receipt, known, &original, &proof_root, false)?;
        let before_unlink = || {
            ordinary.require_named()?;
            original.require_named()?;
            proof_root.require_named()?;
            authorize()
        };
        // No unlink before the entire proof and final live authority succeed.
        // Each source member is re-pinned and authorized again at its unlink.
        before_unlink()?;
        for (saved, row) in known.receipts.values().zip(rows.iter()) {
            retire_entry(&original, saved, row, false, Some(&before_unlink))?;
        }
        before_unlink()?;
        parent.unlink_empty_opaque_original(&leaf, &original)
    }
}

fn prove_source(
    receipt: RecycleReceipt,
    known: &State,
    original: &HeldDir,
    proof_root: &HeldDir,
    require_complete: bool,
) -> io::Result<Vec<SourceRow>> {
    let entries = known
        .receipts
        .values()
        .map(|r| entry_leaf(r.sequence, r.mutation.request))
        .collect::<BTreeSet<_>>();
    original.each_name(ReadAccess::Ordinary, |name| {
        if std::str::from_utf8(name).is_ok_and(|s| entries.contains(s)) {
            Ok(())
        } else {
            Err(invalid("unknown source descendant prevents retirement"))
        }
    })?;
    original.require_named()?;
    let proof = proof_root
        .opaque_pin(c"source-proof")?
        .ok_or_else(|| invalid("retirement evidence is absent"))?;
    let mut input = &proof.file;
    let mut digest = Sha256::new();
    let mut rows = Vec::new();
    let format = if receipt.is_enrolled() {
        ProofFormat::Enrolled
    } else {
        ProofFormat::Legacy
    };
    rows.try_reserve_exact(known.receipts.len())
        .map_err(|_| io::Error::other("cannot retain exact retirement metadata"))?;
    for saved in known.receipts.values() {
        let row = SourceRow::read(
            &mut input,
            format,
            matches!(saved.change, Change::Save { .. }),
        )?;
        digest.update(row.bytes())?;
        retire_entry(original, saved, &row, require_complete, None)?;
        rows.push(row);
    }
    if input.read(&mut [0; 1])? != 0 || digest.finish() != receipt.source_proof {
        return Err(invalid(
            "source retirement evidence does not bind the exact moved copy",
        ));
    }
    proof.require(proof_root, c"source-proof")?;
    Ok(rows)
}

fn arrived_leaf(mutation: Mutation) -> String {
    format!("{}-{}", hex(mutation.container), hex(mutation.request))
}

fn retire_entry(
    original: &HeldDir,
    receipt: &super::Receipt,
    row: &SourceRow,
    require_complete: bool,
    before_unlink: Option<&dyn Fn() -> io::Result<()>>,
) -> io::Result<()> {
    let leaf = CString::new(entry_leaf(receipt.sequence, receipt.mutation.request))
        .map_err(|_| invalid("invalid source member label"))?;
    let Some(entry) = original.child_directory(&leaf)? else {
        if require_complete {
            return Err(invalid(
                "a prepared unselected archive lost an original source entry",
            ));
        }
        return Ok(());
    };
    row.require_directory(entry.entity, original.same_mount(&entry)?)?;
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
    if require_complete && (record.is_none() || body && payload.is_none()) {
        return Err(invalid(
            "a prepared unselected archive lost an original source member",
        ));
    }
    row.require_members(record.as_ref(), payload.as_ref())?;
    if let Some(check) = before_unlink {
        if let Some(body) = payload {
            check()?;
            entry.unlink_opaque_original(c"payload", &body)?;
        }
        // Payload unlink above is synchronized before receipt unlink, so
        // restart can distinguish an original partial cleanup safely.
        if let Some(record) = record {
            check()?;
            entry.unlink_opaque_original(c"receipt", &record)?;
        }
        check()?;
        original.unlink_empty_opaque_original(&leaf, &entry)?;
    }
    Ok(())
}
