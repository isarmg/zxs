//! Private READY: no DATA arrival, CR1 candidate publication, or source retirement.
#[path = "ready_tree.rs"]
mod tree;

use super::super::super::super::{BYTES as ROUTE_BYTES, encode as encode_route};
use super::super::super::{
    Journal, JournalBytes, READY_BYTES, Ready, digest,
    proof::{Expect, Proof},
};
use super::{
    BoundJournal, Checked, Digest, File, HeldDir, HeldGuard, I, OpaquePin, PROOF, Pending, Route,
    Selected, absent, birth, create, intend, invalid, pin, positions, read_raw, same_file,
};
use std::{ffi::CStr, io};
use tree::Tree;

const TARGET: &CStr = c"target-proof";
const CANDIDATE: &CStr = c"new-route";
const R: &CStr = c"READY";
const BEFORE_COPY: &[&CStr] = &[I, PROOF];
const HAS_TARGET_PROOF: &[&CStr] = &[I, PROOF, TARGET];
const HAS_CANDIDATE: &[&CStr] = &[I, PROOF, TARGET, CANDIDATE];
const HAS_READY: &[&CStr] = &[I, PROOF, TARGET, CANDIDATE, R];

struct Armed<'held, 'scope> {
    plan: Pending<'held, 'scope>,
    guard: HeldGuard,
}

impl Armed<'_, '_> {
    fn require(&self, layout: &[&CStr]) -> io::Result<()> {
        self.plan.selected.pre_ready_names()?;
        self.require_bound(layout)
    }

    fn require_bound(&self, layout: &[&CStr]) -> io::Result<()> {
        let selected = self.plan.selected;
        self.plan.require_bound()?;
        require_layout(&self.plan.journal.directory, layout)?;
        self.guard
            .require(selected.roots.state, &selected.names.active)?;
        absent(selected.roots.state, &selected.names.inactive)?;
        absent(&selected.routes, &selected.names.route_stage)?;
        absent(&selected.target_data, &selected.names.cid)
    }

    fn require_leaf(&self, at: &HeldDir) -> io::Result<()> {
        let selected = self.plan.selected;
        (selected.outer)()?;
        self.guard
            .require(selected.roots.state, &selected.names.active)?;
        self.plan.route.require(selected)?;
        self.plan.source.require_named()?;
        at.require_named()
    }
}

struct Draft {
    target: Tree,
    sidecar: OpaquePin,
    candidate: OpaquePin,
}

impl Draft {
    fn require(&self, armed: &Armed<'_, '_>, expected: &Ready) -> io::Result<()> {
        let journal = &armed.plan.journal.directory;
        self.sidecar.require(journal, TARGET)?;
        require_candidate(&self.candidate, journal, expected)?;
        self.target.require(armed)?;
        if expected.guard != armed.guard.checked.value().birth
            || expected.target_inode != self.target.at.entity.inode
        {
            return Err(invalid(
                "the DATA move READY names do not bind their original holders",
            ));
        }
        Ok(())
    }
}

pub(super) struct HeldReady<'held, 'scope> {
    armed: Armed<'held, 'scope>,
    draft: Draft,
    ready: OpaquePin,
    checked: Checked<Ready>,
    raw: [u8; READY_BYTES],
}

impl<'held, 'scope> Pending<'held, 'scope> {
    pub(super) fn copy_to_ready(self) -> io::Result<HeldReady<'held, 'scope>> {
        self.copy_to_ready_with(File::sync_all)
    }

    pub(super) fn copy_to_ready_with(
        self,
        sync_ready_parent: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<HeldReady<'held, 'scope>> {
        let guard = self.arm()?;
        let armed = Armed { plan: self, guard };
        armed.require(BEFORE_COPY)?;
        let journal = &armed.plan.journal;
        let expected = Expect::source(&journal.original);
        let proof = Proof::decode(&armed.plan.source.bytes, &expected)?;
        let target = Tree::copy(&armed, &proof)?;
        let (candidate, sidecar, planned) = stage_ready_inputs(&armed, &target)?;
        let draft = Draft {
            target,
            sidecar,
            candidate,
        };
        recheck_pending(&armed, &draft, &planned)?;
        let raw = planned.encode(&journal.original)?;
        let ready = create(&journal.directory, R, &raw)?;
        HeldReady {
            checked: Ready::decode(&raw, &journal.original)?,
            armed,
            draft,
            ready,
            raw,
        }
        .settle_with(sync_ready_parent)
    }
}

fn stage_ready_inputs(
    armed: &Armed<'_, '_>,
    target: &Tree,
) -> io::Result<(OpaquePin, OpaquePin, Ready)> {
    let journal = &armed.plan.journal.directory;
    armed.require(BEFORE_COPY)?;
    target.require(armed)?;
    let sidecar = create(journal, TARGET, &target.bytes)?;
    let i = armed.plan.journal.original.value();
    let raw = encode_route(i.scope.container, i.target)?;
    armed.require(HAS_TARGET_PROOF)?;
    sidecar.require(journal, TARGET)?;
    target.require(armed)?;
    let candidate = create(journal, CANDIDATE, &raw)?;
    let ready = Ready {
        guard: armed.guard.checked.value().birth,
        target_inode: target.at.entity.inode,
        new_route: raw,
        new_birth: birth(&candidate.file)?,
        target_proof: digest(&target.bytes)?,
    };
    Ok((candidate, sidecar, ready))
}

fn recheck_pending(armed: &Armed<'_, '_>, draft: &Draft, planned: &Ready) -> io::Result<()> {
    let journal = &armed.plan.journal.directory;
    armed.require(HAS_CANDIDATE)?;
    draft.require(armed, planned)?;
    draft.target.require_metadata(armed)?;
    draft.target.settle(armed)?;
    armed.require_bound(HAS_CANDIDATE)?;
    draft.require(armed, planned)?;
    journal.file.sync_all()?; // Both prerequisite names precede creation of READY.
    armed
        .plan
        .source
        .require_same(&armed.plan.selected.observe_source()?)?;
    // Reprove the same staged tree after all fallible slow scope/source checks.
    armed.require(HAS_CANDIDATE)?;
    draft.target.require_metadata(armed)?;
    armed.require_bound(HAS_CANDIDATE)?;
    draft.require(armed, planned)
}

impl<'held, 'scope> HeldReady<'held, 'scope> {
    /// The exact before-arrival layout only. Later published names and any
    /// unproved pre-READY material are preserved and never repaired here.
    pub(super) fn reopen(selected: &'held Selected<'scope>) -> io::Result<Self> {
        selected.pre_ready_names()?;
        let route = Route::original(selected)?;
        let (parent, directory, _) = positions(selected, true)?;
        require_layout(&directory, HAS_READY)?;
        let source = selected.observe_source()?;
        let intent = intend(selected, &route, &source, &directory)?;
        let inputs = (pin(&directory, I)?, pin(&directory, PROOF)?);
        let journal = BoundJournal::bind((parent, directory), inputs, &intent, &source)?;
        let plan = Pending {
            selected,
            route,
            source,
            journal,
        };
        let guard = plan.active()?;
        let armed = Armed { plan, guard };
        armed.require(HAS_READY)?;
        let directory = &armed.plan.journal.directory;
        let ready = pin(directory, R)?;
        let raw: [u8; READY_BYTES] = read_raw(&ready, READY_BYTES)?
            .try_into()
            .map_err(|_| invalid("the DATA move READY has the wrong extent"))?;
        let chain = Journal::decode(JournalBytes {
            intent: &armed.plan.journal.raw,
            guard: Some(&armed.guard.raw),
            ready: Some(&raw),
            switched: None,
            done: None,
        })?;
        let checked = Ready::decode(&raw, chain.intent())?;
        let draft = reopen_draft(&armed, &chain, checked.value())?;
        Self {
            armed,
            draft,
            ready,
            checked,
            raw,
        }
        .settle_with(File::sync_all)
    }

    fn settle_with(
        self,
        sync_ready_parent: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        self.require()?;
        let plan = &self.armed.plan;
        let journal = &plan.journal;
        self.draft.target.settle(&self.armed)?;
        self.draft.sidecar.file.sync_all()?;
        self.draft.candidate.file.sync_all()?;
        self.ready.file.sync_all()?;
        journal.directory.file.sync_all()?;
        sync_ready_parent(&journal.parent.file)?;
        self.require()?;
        plan.selected.roots.system.file.sync_all()?;
        // Recovery might be seeing a visible but previously unconfirmed READY.
        // Only exact named evidence under the same current authority can settle.
        self.require()?;
        self.draft.target.require_metadata(&self.armed)?;
        self.armed.require_bound(HAS_READY)?;
        self.require_records()?;
        Ok(self)
    }

    fn require(&self) -> io::Result<()> {
        self.armed.require(HAS_READY)?;
        self.require_records()
    }

    fn require_records(&self) -> io::Result<()> {
        let directory = &self.armed.plan.journal.directory;
        self.draft.require(&self.armed, self.checked.value())?;
        self.ready.require(directory, R)?;
        if read_raw(&self.ready, READY_BYTES)? != self.raw {
            return Err(invalid("the exact DATA move READY bytes changed"));
        }
        self.ready.require(directory, R)
    }

    pub(super) fn id(&self) -> Digest {
        self.checked.id()
    }
}

fn reopen_draft(armed: &Armed<'_, '_>, chain: &Journal, expected: &Ready) -> io::Result<Draft> {
    let directory = &armed.plan.journal.directory;
    let candidate = pin(directory, CANDIDATE)?;
    require_candidate(&candidate, directory, expected)?;
    let sidecar = pin(directory, TARGET)?;
    let expect = Expect::target(chain)?;
    let bytes = read_raw(&sidecar, expect.extent()?)?;
    let proof = Proof::decode(&bytes, &expect)?;
    sidecar.require(directory, TARGET)?;
    let target = Tree::cold(armed, (&proof, &bytes))?;
    Ok(Draft {
        target,
        sidecar,
        candidate,
    })
}

fn require_candidate(candidate: &OpaquePin, journal: &HeldDir, expected: &Ready) -> io::Result<()> {
    candidate.require(journal, CANDIDATE)?;
    if read_raw(candidate, ROUTE_BYTES)? != expected.new_route
        || birth(&candidate.file)? != expected.new_birth
    {
        return Err(invalid(
            "the exact prepared DATA move CR1 candidate changed",
        ));
    }
    candidate.require(journal, CANDIDATE)
}

fn require_layout(directory: &HeldDir, layout: &[&CStr]) -> io::Result<()> {
    directory.require_named()?;
    let mut observed = [false; HAS_READY.len()];
    directory.each_name(super::ReadAccess::NoAtime, |name| {
        let index = layout
            .iter()
            .position(|expected| expected.to_bytes() == name)
            .ok_or_else(|| invalid("unexpected material in the exact DATA move journal"))?;
        if std::mem::replace(&mut observed[index], true) {
            return Err(invalid("noncanonical DATA move journal inventory"));
        }
        Ok(())
    })?;
    if observed[..layout.len()].contains(&false) {
        return Err(invalid("incomplete DATA move READY inventory is preserved"));
    }
    directory.require_named()
}
