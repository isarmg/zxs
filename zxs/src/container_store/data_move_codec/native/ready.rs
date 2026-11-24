//! Private READY and no-clobber arrivals; separate route selection owns the next cut.
#[path = "arrival.rs"]
mod arrival;
#[path = "switch.rs"]
pub(super) mod switch;
#[path = "ready_tree.rs"]
mod tree;
#[path = "copy_work.rs"]
mod work;

use super::super::super::super::{BYTES as ROUTE_BYTES, encode as encode_route};
use super::super::super::{
    Journal, JournalBytes, READY_BYTES, Ready, digest,
    proof::{Expect, Proof},
};
use super::{
    BoundJournal, Checked, File, HeldDir, HeldGuard, I, OpaquePin, PROOF, Pending, Route, Selected,
    absent, birth, intend, invalid, pin, positions, read_raw, same_file,
};
use arrival::Place;
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
        self.require_scope(layout)?;
        Place::Private.require_absence(self)
    }

    fn require_scope(&self, layout: &[&CStr]) -> io::Result<()> {
        let selected = self.plan.selected;
        self.plan.require_bound()?;
        require_layout(&self.plan.journal.directory, layout)?;
        self.guard
            .require(selected.roots.state, &selected.names.active)?;
        absent(selected.roots.state, &selected.names.inactive)
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
    fn require(&self, armed: &Armed<'_, '_>, place: Place, expected: &Ready) -> io::Result<()> {
        let journal = &armed.plan.journal.directory;
        self.sidecar.require(journal, TARGET)?;
        require_candidate(&self.candidate, place.candidate_site(armed), expected)?;
        self.target.require(place.tree_site(armed.plan.selected))?;
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
    place: Place,
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
        let journal = &armed.plan.journal;
        let expected = Expect::source(&journal.original);
        let proof = Proof::decode(&armed.plan.source.bytes, &expected)?;
        let owned = work::Work::open(&armed)?;
        owned.restart(&proof)?;
        armed.require(BEFORE_COPY)?;
        let target = Tree::copy(&armed, &proof, owned.into_stage()?)?;
        let (candidate, sidecar, planned) = stage_ready_inputs(&armed, &target)?;
        let draft = Draft {
            target,
            sidecar,
            candidate,
        };
        recheck_pending(&armed, &draft, &planned)?;
        let raw = planned.encode(&journal.original)?;
        let ready = journal.publish(R, &raw, || {
            armed.require_bound(HAS_CANDIDATE)?;
            draft.require(&armed, Place::Private, &planned)
        })?;
        HeldReady {
            checked: Ready::decode(&raw, &journal.original)?,
            armed,
            place: Place::Private,
            draft,
            ready,
            raw,
        }
        .settle_with(sync_ready_parent, File::sync_all)
    }
}

fn stage_ready_inputs(
    armed: &Armed<'_, '_>,
    target: &Tree,
) -> io::Result<(OpaquePin, OpaquePin, Ready)> {
    let journal = &armed.plan.journal.directory;
    armed.require(BEFORE_COPY)?;
    target.require(Place::Private.tree_site(armed.plan.selected))?;
    let sidecar = armed.plan.journal.publish(TARGET, &target.bytes, || {
        armed.require_bound(BEFORE_COPY)?;
        target.require(Place::Private.tree_site(armed.plan.selected))
    })?;
    let i = armed.plan.journal.original.value();
    let raw = encode_route(i.scope.container, i.target)?;
    armed.require(HAS_TARGET_PROOF)?;
    sidecar.require(journal, TARGET)?;
    target.require(Place::Private.tree_site(armed.plan.selected))?;
    let candidate = armed.plan.journal.publish(CANDIDATE, &raw, || {
        armed.require_bound(HAS_TARGET_PROOF)?;
        sidecar.require(journal, TARGET)?;
        target.require(Place::Private.tree_site(armed.plan.selected))
    })?;
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
    draft.require(armed, Place::Private, planned)?;
    draft.target.require_metadata(armed)?;
    draft.target.settle(armed, Place::Private)?;
    armed.require_bound(HAS_CANDIDATE)?;
    draft.require(armed, Place::Private, planned)?;
    journal.file.sync_all()?; // Both prerequisite names precede creation of READY.
    armed
        .plan
        .source
        .require_same(&armed.plan.selected.observe_source()?)?;
    // Reprove the same staged tree after all fallible slow scope/source checks.
    armed.require(HAS_CANDIDATE)?;
    draft.target.require_metadata(armed)?;
    armed.require_bound(HAS_CANDIDATE)?;
    draft.require(armed, Place::Private, planned)
}

impl<'held, 'scope> HeldReady<'held, 'scope> {
    /// Exact READY before CR1 selection: fully private, target arrived, or
    /// target and candidate arrived. No pre-READY or mixed material is adopted.
    pub(super) fn reopen(selected: &'held Selected<'scope>) -> io::Result<Self> {
        selected.pre_ready_names()?;
        let route = Route::original(selected)?;
        let (parent, directory) = positions(selected)?;
        let place = Place::observe(selected, &directory)?;
        require_layout(&directory, place.layout())?;
        let source = selected.observe_source()?;
        let intent = intend(selected, &route, &source, &directory)?;
        let inputs = (pin(&directory, I)?, pin(&directory, PROOF)?);
        let journal = BoundJournal::bind((parent, directory), inputs, &intent, &source)?;
        let plan = Pending {
            selected,
            route,
            source,
            journal,
            resuming_copy: false,
        };
        let guard = plan.active()?;
        let armed = Armed { plan, guard };
        selected.pre_ready_names()?;
        armed.require_scope(place.layout())?;
        place.require_absence(&armed)?;
        let (ready, raw, chain) = read_ready(&armed.plan.journal, &armed.guard)?;
        let checked = Ready::decode(&raw, chain.intent())?;
        let draft = reopen_draft(&armed, place, &chain, checked.value())?;
        Self {
            armed,
            place,
            draft,
            ready,
            checked,
            raw,
        }
        .settle_with(File::sync_all, File::sync_all)
    }

    fn settle_with(
        self,
        sync_ready_parent: impl FnOnce(&File) -> io::Result<()>,
        sync_routes: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        self.require()?;
        let plan = &self.armed.plan;
        let journal = &plan.journal;
        self.draft.target.settle(&self.armed, self.place)?;
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
        self.sync_candidate_parent(sync_routes)?;
        self.draft.target.require_metadata(&self.armed)?;
        self.require_position()?;
        Ok(self)
    }

    fn require(&self) -> io::Result<()> {
        self.armed.plan.selected.pre_ready_names()?;
        self.require_position()
    }

    fn require_position(&self) -> io::Result<()> {
        let journal = &self.armed.plan.journal;
        journal.drafts_absent(&[TARGET, CANDIDATE, R, c"SWITCHED", c"DONE"])?;
        self.armed.require_scope(self.place.layout())?;
        self.place.require_absence(&self.armed)?;
        self.require_records()
    }

    fn require_records(&self) -> io::Result<()> {
        let directory = &self.armed.plan.journal.directory;
        self.draft
            .require(&self.armed, self.place, self.checked.value())?;
        self.ready.require(directory, R)?;
        if read_raw(&self.ready, READY_BYTES)? != self.raw {
            return Err(invalid("the exact DATA move READY bytes changed"));
        }
        self.ready.require(directory, R)
    }

    #[cfg(test)]
    pub(super) fn id(&self) -> super::super::Digest {
        self.checked.id()
    }
}

fn reopen_draft(
    armed: &Armed<'_, '_>,
    place: Place,
    chain: &Journal,
    expected: &Ready,
) -> io::Result<Draft> {
    let site = place.candidate_site(armed);
    let candidate = pin(site.0, site.1)?;
    require_candidate(&candidate, site, expected)?;
    let (target, sidecar) = recover_target(
        &armed.plan.journal,
        armed.plan.selected,
        place,
        chain,
        expected,
    )?;
    Ok(Draft {
        target,
        sidecar,
        candidate,
    })
}

fn read_ready(
    journal: &BoundJournal,
    guard: &HeldGuard,
) -> io::Result<(OpaquePin, [u8; READY_BYTES], Journal)> {
    let ready = pin(&journal.directory, R)?;
    let raw: [u8; READY_BYTES] = read_raw(&ready, READY_BYTES)?
        .try_into()
        .map_err(|_| invalid("the DATA move READY has the wrong extent"))?;
    let chain = Journal::decode(JournalBytes {
        intent: &journal.raw,
        guard: Some(&guard.raw),
        ready: Some(&raw),
        switched: None,
        done: None,
    })?;
    ready.require(&journal.directory, R)?;
    Ok((ready, raw, chain))
}

fn recover_target(
    journal: &BoundJournal,
    selected: &Selected<'_>,
    place: Place,
    chain: &Journal,
    expected: &Ready,
) -> io::Result<(Tree, OpaquePin)> {
    let directory = &journal.directory;
    let sidecar = pin(directory, TARGET)?;
    let expect = Expect::target(chain)?;
    let bytes = read_raw(&sidecar, expect.extent()?)?;
    let proof = Proof::decode(&bytes, &expect)?;
    sidecar.require(directory, TARGET)?;
    let target = Tree::cold(
        (selected, &journal.original),
        (&proof, &bytes),
        place.tree_site(selected),
        expected.target_inode,
    )?;
    Ok((target, sidecar))
}

fn require_candidate(
    candidate: &OpaquePin,
    (directory, name): (&HeldDir, &CStr),
    expected: &Ready,
) -> io::Result<()> {
    candidate.require(directory, name)?;
    if read_raw(candidate, ROUTE_BYTES)? != expected.new_route
        || birth(&candidate.file)? != expected.new_birth
    {
        return Err(invalid(
            "the exact prepared DATA move CR1 candidate changed",
        ));
    }
    candidate.require(directory, name)
}

fn require_layout(directory: &HeldDir, layout: &[&CStr]) -> io::Result<()> {
    directory.require_named()?;
    let mut observed = [false; 6]; // Largest known exact journal phase is DONE.
    if layout.len() > observed.len() {
        return Err(invalid("unknown DATA move journal layout"));
    }
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

#[cfg(test)]
#[path = "../../../../tests/unit/data_move_copy_restart.rs"]
mod copy_restart_tests;
