//! Recover only the original cooperative writer's incomplete private copy.
use super::super::super::{
    ACCESS, native_name,
    proof::{Proof, Side, Stamp},
    source::{self, Member},
};
use super::super::publish;
use super::{
    Armed, CANDIDATE, CStr, HeldDir, OpaquePin, R, Ready, TARGET, Tree, absent, birth, digest,
    encode_route, invalid, read_raw, tree::check_body,
};
use crate::container_store::{Change, Receipt, codec, entry_leaf};
use std::{collections::BTreeSet, ffi::CString, io};

const COPY: &CStr = c"COPYING";
const COPY_LIMIT: usize = 152;

pub(super) struct Work<'h, 'held, 'scope> {
    armed: &'h Armed<'held, 'scope>,
    stage: HeldDir,
    name: CString,
    witness: OpaquePin,
    raw: [u8; COPY_LIMIT],
}

impl<'h, 'held, 'scope> Work<'h, 'held, 'scope> {
    pub(super) fn open(armed: &'h Armed<'held, 'scope>) -> io::Result<Self> {
        Self::require_before_ready(armed)?;
        let plan = &armed.plan;
        let j = &plan.journal;
        let at = &plan.selected.target_staging;
        let leaf = &plan.selected.names.target_stage;
        let source = &plan.source.original;
        if source.entity.device == at.entity.device || source.same_mount(at)? {
            return Err(invalid("DATA move staging is not on a distinct filesystem"));
        }
        let name = j.scoped_leaf(COPY, false)?;
        let existing = j.parent.opaque_pin_with(&name, ACCESS)?;
        let stage = match at.child_directory(leaf)? {
            Some(known) => known,
            None if existing.is_none() => {
                at.new_child_directory_when(leaf, || Self::require_before_ready(armed))?
            }
            None => return Err(invalid("the recorded original copy stage is absent")),
        };
        let raw = Self::bytes(armed, &stage)?;
        let witness = if let Some(original) = existing {
            j.draft_absent(COPY)?;
            original
        } else {
            // Before an effective COPYING no body can have been written. An
            // exact, private empty name may have survived this operation's mkdir.
            let pristine = || {
                Self::require_before_ready(armed)?;
                stage.require_named()?;
                stage.only(None)
            };
            pristine()?;
            stage.file.sync_all()?;
            at.file.sync_all()?;
            pristine()?;
            let candidate = j.scoped_leaf(COPY, true)?;
            publish::publish((&j.parent, &candidate), (&j.parent, &name), &raw, pristine)?
        };
        let work = Self {
            armed,
            stage,
            name,
            witness,
            raw,
        };
        work.require()?;
        // A visible canonical witness after a killed writer is not durable by itself.
        work.witness.file.sync_all()?;
        j.parent.file.sync_all()?;
        work.require()?;
        Ok(work)
    }

    fn bytes(armed: &Armed<'_, '_>, stage: &HeldDir) -> io::Result<[u8; COPY_LIMIT]> {
        let selected = armed.plan.selected;
        let i = &armed.plan.journal.original;
        let mut bytes = [0; COPY_LIMIT];
        bytes[..8].copy_from_slice(b"ZXDMC1\0\0");
        bytes[8..40].copy_from_slice(&i.id());
        bytes[40..72].copy_from_slice(&armed.guard.checked.id());
        bytes[72..104].copy_from_slice(&i.value().target);
        bytes[104..112].copy_from_slice(&selected.target_staging.entity.inode.to_be_bytes());
        bytes[112..120].copy_from_slice(&stage.entity.inode.to_be_bytes());
        let checked = digest(&bytes[..120])?;
        bytes[120..].copy_from_slice(&checked);
        Ok(bytes)
    }

    fn require_before_ready(armed: &Armed<'_, '_>) -> io::Result<()> {
        let plan = &armed.plan;
        (plan.selected.outer)()?;
        plan.require_held()?;
        plan.journal.drafts_absent(&[c"SWITCHED", c"DONE"])?;
        armed
            .guard
            .require(plan.selected.roots.state, &plan.selected.names.active)?;
        absent(plan.selected.roots.state, &plan.selected.names.inactive)?;
        plan.selected.vacancies_for_copy(false, true)
    }

    fn require(&self) -> io::Result<()> {
        Self::require_before_ready(self.armed)?;
        let stage = &self.stage;
        let selected = self.armed.plan.selected;
        if !selected.target_staging.same_mount(stage)? {
            return Err(invalid(
                "the owned copy no longer shares its original mount",
            ));
        }
        stage.require_named()?;
        let at = &self.armed.plan.journal.parent;
        self.witness.require(at, &self.name)?;
        if read_raw(&self.witness, COPY_LIMIT)? != self.raw {
            return Err(invalid("unproved original DATA copy ownership"));
        }
        self.witness.require(at, &self.name)
    }

    pub(super) fn into_stage(self) -> io::Result<HeldDir> {
        self.require()?;
        self.stage.only(None)?;
        Ok(self.stage)
    }

    pub(super) fn restart(&self, proof: &Proof<'_>) -> io::Result<()> {
        if !self.armed.plan.resuming_copy {
            return self.stage.only(None);
        }
        self.require()?;
        let states = self.inventory(proof)?;
        let stale = self.stale()?;
        // No unlink until both full surveys passed. Remove the most dependent
        // inactive records first, then discard owned entries in reverse order.
        let plan = &self.armed.plan;
        plan.source.require_same(&plan.selected.observe_source()?)?;
        self.require()?;
        for file in stale {
            self.require()?;
            file.parent.unlink_opaque_original(&file.name, &file.pin)?;
        }
        for row in states.iter().rev() {
            self.discard(row)?;
        }
        self.stage.only(None)?;
        self.stage.file.sync_all()?;
        self.stage.only(None)?;
        self.require()
    }

    fn inventory(&self, proof: &Proof<'_>) -> io::Result<Vec<Row>> {
        let selected = self.armed.plan.selected;
        let source = &self.armed.plan.source.original;
        let mut names = BTreeSet::new();
        self.stage.each_name(ACCESS, |name| {
            if names.len()
                == usize::try_from(super::super::super::super::MAX_ROWS)
                    .map_err(|_| invalid("unbounded DATA copy inventory"))?
                || !names.insert(name.to_vec())
            {
                return Err(invalid("oversize or duplicate DATA copy inventory"));
            }
            Ok(())
        })?;
        let mut resolved = Vec::new();
        let mut ended = false;
        let mut evidence = source::walk(source, selected.scope, selected.pulse, |receipt| {
            let member = Member::open(source, receipt)?;
            if member.row != proof.for_receipt(receipt)? {
                return Err(invalid(
                    "the original source changed before discarding the incomplete copy",
                ));
            }
            let name = native_name(&entry_leaf(receipt.sequence, receipt.mutation.request))?;
            if names.remove(name.to_bytes()) {
                if ended {
                    return Err(invalid(
                        "the private copy does not have a typed sequential prefix",
                    ));
                }
                let row = Row::read(&self.stage, name, receipt, selected.pulse)?;
                ended = !row.complete;
                resolved.push(row);
            } else {
                ended = true;
            }
            Ok(member.row)
        })?;
        evidence.bind(
            Side::Source,
            selected.scope,
            selected.roots.data.0.token,
            source.entity.inode,
        )?;
        if evidence.bytes != self.armed.plan.source.bytes || !names.is_empty() {
            return Err(invalid(
                "unexpected or inconsistent material in the private copy is retained",
            ));
        }
        Ok(resolved)
    }

    fn stale(&self) -> io::Result<Vec<DiscardFile<'_>>> {
        let j = &self.armed.plan.journal;
        let route_temp = j.scoped_leaf(CANDIDATE, true)?;
        let proof_temp = j.scoped_leaf(TARGET, true)?;
        let ready_temp = j.scoped_leaf(R, true)?;
        let ready = DiscardFile::open(&j.parent, ready_temp)?;
        let candidate = DiscardFile::open(&j.directory, CANDIDATE.into())?;
        let candidate_draft = DiscardFile::open(&j.parent, route_temp)?;
        let target = DiscardFile::open(&j.directory, TARGET.into())?;
        let target_draft = DiscardFile::open(&j.parent, proof_temp)?;
        if (candidate.is_some() && candidate_draft.is_some())
            || (target.is_some() && target_draft.is_some())
            || ((candidate.is_some() || candidate_draft.is_some()) && target.is_none())
        {
            return Err(invalid(
                "mixed inactive DATA move phase records are retained",
            ));
        }
        let mut to_unlink = Vec::new();
        if ready.is_some() || target.is_some() || target_draft.is_some() {
            let bytes = Tree::current_metadata(self.armed, &self.stage)?;
            let i = j.original.value();
            let route = encode_route(i.scope.container, i.target)?;
            if let Some(ready) = ready {
                let candidate = candidate
                    .as_ref()
                    .ok_or_else(|| invalid("the inactive READY has no exact original candidate"))?;
                if target.is_none() {
                    return Err(invalid("the inactive READY has no original target proof"));
                }
                let exact = Ready {
                    guard: self.armed.guard.checked.value().birth,
                    target_inode: self.stage.entity.inode,
                    new_route: route,
                    new_birth: birth(&candidate.pin.file)?,
                    target_proof: digest(&bytes)?,
                }
                .encode(&j.original)?;
                ready.validate(&exact, false)?;
                to_unlink.push(ready);
            }
            if let Some(file) = candidate {
                file.validate(&route, true)?;
                to_unlink.push(file);
            }
            if let Some(file) = candidate_draft {
                file.validate(&route, false)?;
                to_unlink.push(file);
            }
            if let Some(file) = target {
                file.validate(&bytes, true)?;
                to_unlink.push(file);
            }
            if let Some(file) = target_draft {
                file.validate(&bytes, false)?;
                to_unlink.push(file);
            }
        }
        Ok(to_unlink)
    }

    fn discard(&self, row: &Row) -> io::Result<()> {
        self.require()?;
        let entry = super::super::super::child(&self.stage, &row.name)?;
        if entry.entity.inode != row.inode {
            return Err(invalid("a preflighted private copy entry changed"));
        }
        let receipt = still(&entry, c"receipt", row.receipt)?;
        let payload = still(&entry, c"payload", row.payload)?;
        if let Some(pin) = receipt {
            self.require()?;
            entry.unlink_opaque_original(c"receipt", &pin)?;
        } else {
            entry.confirm_absence(c"receipt")?;
        }
        if let Some(pin) = payload {
            self.require()?;
            entry.unlink_opaque_original(c"payload", &pin)?;
        } else {
            entry.confirm_absence(c"payload")?;
        }
        self.require()?;
        self.stage.unlink_empty_opaque_original(&row.name, &entry)
    }
}

struct DiscardFile<'a> {
    parent: &'a HeldDir,
    name: CString,
    pin: OpaquePin,
}
impl<'a> DiscardFile<'a> {
    fn open(parent: &'a HeldDir, name: CString) -> io::Result<Option<Self>> {
        Ok(parent
            .opaque_pin_with(&name, ACCESS)?
            .map(|pin| Self { parent, name, pin }))
    }
    fn validate(&self, expected: &[u8], complete: bool) -> io::Result<()> {
        if !publish::prefix_matches((self.parent, &self.name), &self.pin, expected)? && complete {
            return Err(invalid("an effective private copy record is not complete"));
        }
        Ok(())
    }
}

struct Row {
    name: CString,
    inode: u64,
    receipt: Option<Stamp>,
    payload: Option<Stamp>,
    complete: bool,
}
impl Row {
    fn read(
        stage: &HeldDir,
        name: CString,
        source: &Receipt,
        live: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        let entry = super::super::super::child(stage, &name)?;
        entry.each_name(ACCESS, |leaf| match leaf {
            b"receipt" | b"payload" => Ok(()),
            _ => Err(invalid("unexpected incomplete copy member is retained")),
        })?;
        let receipt = entry.opaque_pin_with(c"receipt", ACCESS)?;
        let payload = entry.opaque_pin_with(c"payload", ACCESS)?;
        let length = payload
            .as_ref()
            .map(|at| at.file.metadata().map(|m| m.len()))
            .transpose()?;
        let body_complete = match (&source.change, length) {
            (Change::Save { body, .. }, Some(len)) if len <= body.length => len == body.length,
            (Change::Save { .. }, None) => false,
            (_, None) => true,
            _ => return Err(invalid("unexpected private copy payload extent")),
        };
        let complete = if let Some(at) = &receipt {
            if !body_complete {
                return Err(invalid(
                    "a private copy receipt cannot precede its whole body",
                ));
            }
            publish::prefix_matches((&entry, c"receipt"), at, &codec::encode(source)?)?
        } else {
            // The old worker may have emitted exactly the declared byte count
            // and then failed its hash. Do not discard that unproven draft as
            // an ordinary crash tail. Earlier rows with receipts and genuinely
            // short drafts already have an unambiguous construction order.
            if let (Change::Save { body, .. }, Some(raw)) = (&source.change, &payload)
                && body_complete
            {
                raw.require(&entry, c"payload")?;
                check_body(body, &raw.file, &mut io::sink(), live)?;
                live()?;
                raw.require(&entry, c"payload")?;
            }
            false
        };
        let receipt = receipt.as_ref().map(source::stamp).transpose()?;
        let payload = payload.as_ref().map(source::stamp).transpose()?;
        entry.require_named()?;
        Ok(Self {
            name,
            inode: entry.entity.inode,
            receipt,
            payload,
            complete,
        })
    }
}
fn still(parent: &HeldDir, name: &CStr, expected: Option<Stamp>) -> io::Result<Option<OpaquePin>> {
    let found = parent.opaque_pin_with(name, ACCESS)?;
    if found.as_ref().map(source::stamp).transpose()? != expected {
        return Err(invalid("a preflighted private copy member changed"));
    }
    Ok(found)
}
