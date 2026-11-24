//! Finish a privately selected move without widening any pre-SWITCHED replay.
#[path = "move_done.rs"]
mod done;
#[path = "retire_tree.rs"]
mod tree;

use super::super::super::super::super::{
    Digest, Journal, JournalBytes, Phase, Ready, encode_done,
    proof::{Expect, Proof},
};
use super::{
    BoundJournal, CStr, CUT_BYTES, Chosen, Draft, File, HAS_SWITCHED, HeldDir, HeldGuard,
    HeldSwitch, I, INTENT_BYTES, Intent, PROOF, Place, S, Selected, Switched, absent, birth,
    invalid, io, pin, positions, read_raw, read_ready, recover_target, require_candidate,
    require_layout, require_marker, require_pending_absence, same_file,
};
use crate::fs::OpaquePin;
use done::BoundDone;
use tree::SourcePlan;

pub(crate) use done::Completion;
pub(in crate::container_store) use done::HistoryRequest;

pub(super) const D: &CStr = c"DONE";
pub(super) const FINISHED: &[&CStr] = &[I, PROOF, super::TARGET, super::R, S, D];

struct Retiring<'held, 'scope> {
    cut: Chosen<'held, 'scope, Vec<u8>>,
    switched: OpaquePin,
    raw: [u8; CUT_BYTES],
    original_live_source: Option<HeldDir>,
}

impl HeldSwitch<'_, '_> {
    pub(in crate::container_store) fn complete(self) -> io::Result<Completion> {
        let HeldSwitch {
            selected,
            marker,
            bytes,
        } = self;
        let Chosen {
            selected,
            source,
            journal,
            guard,
            draft,
            ready,
            checked,
            raw,
        } = selected;
        Retiring {
            cut: Chosen {
                selected,
                source: source.bytes,
                journal,
                guard,
                draft,
                ready,
                checked,
                raw,
            },
            switched: marker,
            raw: bytes,
            original_live_source: Some(source.original),
        }
        .complete(false)
    }
}

pub(in crate::container_store) fn resume(selected: &Selected<'_>) -> io::Result<Completion> {
    Retiring::recover(selected)?.complete(true)
}

pub(in crate::container_store) fn history(request: HistoryRequest<'_>) -> io::Result<Completion> {
    let bound = BoundDone::open(request)?;
    bound.release(|| Ok(())).map(Completion::Historical)
}

impl<'held, 'scope> Retiring<'held, 'scope> {
    fn recover(selected: &'held Selected<'scope>) -> io::Result<Self> {
        selected.pre_ready_names()?;
        let (parent, directory, _) = positions(selected, true)?;
        require_layout(&directory, HAS_SWITCHED)?;
        require_pending_absence(selected, &directory)?;
        let record = pin(&directory, I)?;
        let retained = Intent::decode(&read_raw(&record, INTENT_BYTES)?)?;
        require_scope(selected, retained.value())?;
        let sidecar = pin(&directory, PROOF)?;
        let (journal, source) =
            BoundJournal::read_records((parent, directory), (record, sidecar), retained.value())?;
        let guard = HeldGuard::open(
            selected.roots.state,
            &selected.names.active,
            &journal.original,
        )?
        .ok_or_else(|| invalid("retirement requires its exact active move guard"))?;
        absent(selected.roots.state, &selected.names.inactive)?;
        let (ready, ready_raw, chain) = read_ready(&journal, &guard)?;
        let checked = Ready::decode(&ready_raw, chain.intent())?;
        let marker = pin(&journal.directory, S)?;
        let raw = Switched::encode(&checked)?;
        require_marker(&journal.directory, Some(&marker), &raw)?;
        require_pending_absence(selected, &journal.directory)?;
        let site = (&selected.routes, selected.names.cid.as_c_str());
        let candidate = pin(site.0, site.1)?;
        require_candidate(&candidate, site, checked.value())?;
        let (target, sidecar) = recover_target(
            &journal,
            selected,
            Place::BothArrived,
            &chain,
            checked.value(),
        )?;
        let draft = Draft {
            target,
            sidecar,
            candidate,
        };
        Ok(Self {
            cut: Chosen {
                selected,
                source,
                journal,
                guard,
                draft,
                ready,
                checked,
                raw: ready_raw,
            },
            switched: marker,
            raw,
            original_live_source: None,
        })
    }

    fn require(&self, finished: bool) -> io::Result<()> {
        let current = self.cut.selected;
        (current.outer)()?;
        let j = &self.cut.journal;
        j.draft_absent(S)?;
        if j.parent.pin_record(&j.scoped_leaf(D, true)?)?.is_some() {
            // A known draft may have survived normal finish, but only after
            // the original source root was already fully retired.
            absent(&current.original_data, &current.names.cid)?;
        }
        self.cut
            .require_position_with(if finished { FINISHED } else { HAS_SWITCHED })?;
        require_marker(&self.cut.journal.directory, Some(&self.switched), &self.raw)
    }

    fn sync_cold_switch(&self) -> io::Result<()> {
        self.require(false)?;
        self.cut.require_route()?;
        self.cut.selected.routes.file.sync_all()?;
        self.cut.require_route()?;
        self.require(false)?;
        self.switched.file.sync_all()?;
        self.cut.journal.directory.file.sync_all()?;
        self.cut.journal.parent.file.sync_all()?;
        self.cut.selected.roots.system.file.sync_all()?;
        self.require(false)
    }

    fn chain(&self) -> io::Result<Journal> {
        Journal::decode(JournalBytes {
            intent: &self.cut.journal.raw,
            guard: Some(&self.cut.guard.raw),
            ready: Some(&self.cut.raw),
            switched: Some(&self.raw),
            done: None,
        })
    }

    fn complete(self, cold: bool) -> io::Result<Completion> {
        self.require(false)?;
        let chain = self.chain()?;
        let Phase::Switched(s) = chain.phase() else {
            return Err(invalid("retirement requires exact SWITCHED lineage"));
        };
        let i = chain.intent().value();
        let original = Proof::decode(&self.cut.source, &Expect::source(chain.intent()))?;
        let copied = Proof::decode(&self.cut.draft.target.bytes, &Expect::target(&chain)?)?;
        let selected = self.cut.selected;
        let target = &self.cut.draft.target.at;
        if cold {
            self.sync_cold_switch()?;
        }
        let plan = SourcePlan::prove(
            selected,
            target,
            (&original, &copied),
            (i.source_inode, self.original_live_source.as_ref()),
            (i.rows, i.ledger),
        )?;
        plan.remove(selected, target, &|| self.require(false))?;
        self.finish(encode_done(s)?)
    }

    fn finish(self, raw: [u8; CUT_BYTES]) -> io::Result<Completion> {
        self.require_finished_source(false)?;
        let directory = &self.cut.journal.directory;
        let written = self
            .cut
            .journal
            .publish(D, &raw, || self.require_finished_source(false))?;
        let completed = BoundDone::open(HistoryRequest::from_selection(self.cut.selected))?;
        same_file(directory, &written.file, completed.done_file())?;
        completed
            .release(|| self.require_finished_source(true))
            .map(Completion::Completed)
    }

    fn require_finished_source(&self, done: bool) -> io::Result<()> {
        self.require(done)?;
        let selected = self.cut.selected;
        self.cut
            .draft
            .target
            .require_metadata_at((selected, &self.cut.journal.original))?;
        absent(&selected.original_data, &selected.names.cid)?;
        self.require(done)
    }
}

fn require_scope(selected: &Selected<'_>, i: &Intent) -> io::Result<()> {
    let (source, target) = selected.roots.data;
    if i.scope != selected.scope
        || (i.system, i.source, i.target) != (selected.system_token, source.token, target.token)
        || (i.source_operand, i.target_operand) != selected.operands
        || birth(&selected.roots.state.file)? != i.state_parent
        || birth(&selected.routes.file)? != i.routes_parent
    {
        return Err(invalid(
            "retirement does not match the original intent and roles",
        ));
    }
    Ok(())
}
