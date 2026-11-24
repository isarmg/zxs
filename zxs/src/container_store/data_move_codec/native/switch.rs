//! The sole canonical CR1 cut and durable SWITCHED proof.
#[path = "retire.rs"]
pub(in super::super) mod retire;
use super::super::super::super::{CUT_BYTES, INTENT_BYTES, Intent, Switched};
use super::super::{Source, decode_route, intend_with_old};
use super::{
    Armed, BoundJournal, CANDIDATE, CStr, Checked, Digest, Draft, File, HeldDir, HeldGuard,
    HeldReady, I, OpaquePin, PROOF, Pending, Place, R, READY_BYTES, ROUTE_BYTES, Ready, Selected,
    TARGET, absent, birth, create, invalid, io, pin, positions, read_raw, read_ready,
    recover_target, require_candidate, require_layout, same_file,
};
use std::{
    ffi::{c_char, c_int},
    os::fd::AsRawFd,
};

const S: &CStr = c"SWITCHED";
const NO_SWITCHED: &[&CStr] = &[I, PROOF, TARGET, R];
const HAS_SWITCHED: &[&CStr] = &[I, PROOF, TARGET, R, S];

unsafe extern "C" {
    fn renameat(
        from: c_int,
        original: *const c_char,
        to: c_int,
        destination: *const c_char,
    ) -> c_int;
}

impl<'held, 'scope> HeldReady<'held, 'scope> {
    pub(in super::super) fn switch(self) -> io::Result<HeldSwitch<'held, 'scope>> {
        self.switch_with(|| Ok(()), File::sync_all, File::sync_all)
    }

    pub(in super::super) fn switch_with(
        self,
        before_cut: impl FnOnce() -> io::Result<()>,
        sync_routes: impl FnOnce(&File) -> io::Result<()>,
        sync_journal: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<HeldSwitch<'held, 'scope>> {
        if self.place != Place::BothArrived {
            return Err(invalid(
                "selecting DATA requires exact target and candidate arrival",
            ));
        }
        self.require()?;
        before_cut()?;
        self.prove_arrival()?;

        let selected = self.armed.plan.selected;
        let routes = &selected.routes;
        let (from, into) = (&selected.names.route_stage, &selected.names.cid);
        routes.checked_child_path(from)?;
        routes.checked_child_path(into)?;
        // Re-pin both original native names *after* all slower proof and liveness
        // work. The protected namespace is serialized by the original EX/owners.
        self.armed.plan.route.require(selected)?;
        require_candidate(&self.draft.candidate, (routes, from), self.checked.value())?;
        let fd = routes.file.as_raw_fd();
        // SAFETY: the held parent and exact relative names are re-proved under
        // the cooperating exclusive lease. This is the sole canonical replace.
        if unsafe { renameat(fd, from.as_ptr(), fd, into.as_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let candidate = pin(routes, into)?;
        same_file(routes, &self.draft.candidate.file, &candidate.file)?;
        let HeldReady {
            armed,
            draft,
            ready,
            checked,
            raw,
            ..
        } = self;
        let Armed { plan, guard } = armed;
        let Pending {
            selected,
            source,
            journal,
            ..
        } = plan;
        let draft = Draft { candidate, ..draft };
        Chosen {
            selected,
            source,
            journal,
            guard,
            draft,
            ready,
            checked,
            raw,
        }
        .finish(sync_routes, sync_journal)
    }
}

struct Chosen<'held, 'scope, Origin = Source> {
    selected: &'held Selected<'scope>,
    source: Origin,
    journal: BoundJournal,
    guard: HeldGuard,
    draft: Draft, // candidate is now held under canonical C.
    ready: OpaquePin,
    checked: Checked<Ready>,
    raw: [u8; READY_BYTES],
}

impl<'held, 'scope> Chosen<'held, 'scope> {
    fn recover(selected: &'held Selected<'scope>, candidate: OpaquePin) -> io::Result<Self> {
        let (parent, directory, _) = positions(selected, true)?;
        let marker = directory.pin_record(S)?.is_some();
        require_layout(&directory, layout(marker))?;
        require_pending_absence(selected, &directory)?;
        let record = pin(&directory, I)?;
        let retained = Intent::decode(&read_raw(&record, INTENT_BYTES)?)?;
        let i = retained.value();
        let source = selected.observe_source()?;
        let expected = intend_with_old(selected, (i.old_route, i.old_birth), &source, &directory)?;
        let sidecar = pin(&directory, PROOF)?;
        let journal =
            BoundJournal::bind((parent, directory), (record, sidecar), &expected, &source)?;
        let guard = HeldGuard::open(
            selected.roots.state,
            &selected.names.active,
            &journal.original,
        )?
        .ok_or_else(|| invalid("exact chosen DATA move lost its active guard"))?;
        absent(selected.roots.state, &selected.names.inactive)?;
        let (ready, raw, chain) = read_ready(&journal, &guard)?;
        let checked = Ready::decode(&raw, chain.intent())?;
        let prior = if marker {
            Some(pin(&journal.directory, S)?)
        } else {
            None
        };
        require_marker(
            &journal.directory,
            prior.as_ref(),
            &Switched::encode(&checked)?,
        )?;
        require_pending_absence(selected, &journal.directory)?;
        // Do not read target content from an apparent selection until current
        // canonical CR1 is the one precise READY candidate, not a same-byte twin.
        require_candidate(
            &candidate,
            (&selected.routes, &selected.names.cid),
            checked.value(),
        )?;
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
            selected,
            source,
            journal,
            guard,
            draft,
            ready,
            checked,
            raw,
        })
    }
}

impl<Origin> Chosen<'_, '_, Origin> {
    fn require_route(&self) -> io::Result<()> {
        let selected = self.selected;
        require_pending_absence(selected, &self.journal.directory)?;
        // Keep the named canonical file check last before a scoped route sync.
        require_candidate(
            &self.draft.candidate,
            (&selected.routes, &selected.names.cid),
            self.checked.value(),
        )
    }

    fn require_position_with(&self, files: &[&CStr]) -> io::Result<()> {
        let selected = self.selected;
        selected.require_local()?;
        self.journal.require_original()?;
        require_layout(&self.journal.directory, files)?;
        let i = self.journal.original.value();
        if birth(&selected.roots.state.file)? != i.state_parent
            || birth(&selected.routes.file)? != i.routes_parent
            || self.checked.value().guard != self.guard.checked.value().birth
            || self.checked.value().target_inode != self.draft.target.at.entity.inode
        {
            return Err(invalid("the original DATA move selection custody changed"));
        }
        self.guard
            .require(selected.roots.state, &selected.names.active)?;
        absent(selected.roots.state, &selected.names.inactive)?;
        self.draft
            .target
            .require(Place::BothArrived.tree_site(selected))?;
        let journal = &self.journal.directory;
        self.draft.sidecar.require(journal, TARGET)?;
        self.ready.require(journal, R)?;
        if read_raw(&self.ready, READY_BYTES)? != self.raw {
            return Err(invalid(
                "the exact DATA move READY record changed after selection",
            ));
        }
        self.ready.require(journal, R)?;
        self.require_route()
    }
}

impl<'held, 'scope> Chosen<'held, 'scope> {
    fn require_position(&self, marker: bool) -> io::Result<()> {
        self.source.require_named()?;
        self.require_position_with(layout(marker))
    }

    fn qualify(&self, marker: Option<&OpaquePin>, bytes: &[u8; CUT_BYTES]) -> io::Result<()> {
        self.selected.pre_ready_names()?;
        self.require_position(marker.is_some())?;
        require_marker(&self.journal.directory, marker, bytes)?;
        self.source.require_same(&self.selected.observe_source()?)?;
        self.draft
            .target
            .require_metadata_at((self.selected, &self.journal.original))?;
        self.require_position(marker.is_some())?;
        require_marker(&self.journal.directory, marker, bytes)
    }

    fn finish(
        self,
        sync_routes: impl FnOnce(&File) -> io::Result<()>,
        sync_journal: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<HeldSwitch<'held, 'scope>> {
        let bytes = Switched::encode(&self.checked)?;
        let directory = &self.journal.directory;
        let previous = directory.opaque_pin_with(S, super::super::ReadAccess::NoAtime)?;
        self.qualify(previous.as_ref(), &bytes)?;

        self.require_route()?;
        sync_routes(&self.selected.routes.file)?;
        self.require_route()?;
        self.qualify(previous.as_ref(), &bytes)?;
        self.require_route()?;
        let marker = match previous {
            Some(exact) => exact,
            None => create(directory, S, &bytes)?,
        };

        self.require_position(true)?;
        require_marker(directory, Some(&marker), &bytes)?;
        marker.file.sync_all()?;
        sync_journal(&directory.file)?;
        self.require_position(true)?;
        require_marker(directory, Some(&marker), &bytes)?;
        self.journal.parent.file.sync_all()?;
        self.require_position(true)?;
        self.selected.roots.system.file.sync_all()?;
        self.qualify(Some(&marker), &bytes)?;
        Ok(HeldSwitch {
            selected: self,
            marker,
            bytes,
        })
    }
}

pub(in super::super) struct HeldSwitch<'held, 'scope> {
    selected: Chosen<'held, 'scope>,
    marker: OpaquePin,
    bytes: [u8; CUT_BYTES],
}

impl<'held, 'scope> HeldSwitch<'held, 'scope> {
    pub(in super::super) fn resume(selected: &'held Selected<'scope>) -> io::Result<Self> {
        Self::resume_with(selected, File::sync_all, File::sync_all)
    }

    pub(in super::super) fn resume_with(
        selected: &'held Selected<'scope>,
        sync_routes: impl FnOnce(&File) -> io::Result<()>,
        sync_journal: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        selected.pre_ready_names()?;
        let current = pin(&selected.routes, &selected.names.cid)?;
        let raw = read_raw(&current, ROUTE_BYTES)?
            .try_into()
            .map_err(|_| invalid("canonical DATA move route has an invalid extent"))?;
        let (cid, token) = decode_route(raw)?;
        current.require(&selected.routes, &selected.names.cid)?;
        let (source, target) = selected.roots.data;
        if cid != selected.scope.container {
            return Err(invalid("canonical DATA move route has the wrong container"));
        }
        if token == source.token {
            let (_, journal, _) = positions(selected, true)?;
            if Place::observe(selected, &journal)? != Place::BothArrived {
                return Err(invalid("old DATA route is not ready for exact selection"));
            }
            HeldReady::reopen(selected)?.switch_with(|| Ok(()), sync_routes, sync_journal)
        } else if token == target.token {
            Chosen::recover(selected, current)?.finish(sync_routes, sync_journal)
        } else {
            Err(invalid(
                "canonical DATA move route selects neither original role",
            ))
        }
    }

    pub(in super::super) fn id(&self) -> io::Result<Digest> {
        (self.selected.selected.outer)()?;
        self.selected.require_position(true)?;
        require_marker(
            &self.selected.journal.directory,
            Some(&self.marker),
            &self.bytes,
        )?;
        self.bytes[CUT_BYTES - 32..]
            .try_into()
            .map_err(|_| invalid("invalid SWITCHED identifier"))
    }
}

fn layout(marker: bool) -> &'static [&'static CStr] {
    if marker { HAS_SWITCHED } else { NO_SWITCHED }
}

fn require_pending_absence(selected: &Selected<'_>, journal: &HeldDir) -> io::Result<()> {
    absent(&selected.target_staging, &selected.names.target_stage)?;
    absent(journal, CANDIDATE)?;
    absent(&selected.routes, &selected.names.route_stage)
}

fn require_marker(
    directory: &HeldDir,
    marker: Option<&OpaquePin>,
    bytes: &[u8; CUT_BYTES],
) -> io::Result<()> {
    let Some(exact) = marker else {
        return absent(directory, S);
    };
    exact.require(directory, S)?;
    if read_raw(exact, CUT_BYTES)? != *bytes {
        return Err(invalid(
            "the DATA move SWITCHED record is not the exact original cut",
        ));
    }
    exact.require(directory, S)
}
