//! Ordered no-clobber arrivals under the original ready/guard and old CR1.
use super::super::super::{child, renameat2};
use super::{
    Armed, CANDIDATE, CStr, File, HAS_READY, HeldDir, HeldReady, I, PROOF, R, Selected, TARGET,
    absent, invalid, io, pin, require_candidate, same_file,
};
use std::os::fd::AsRawFd;

const WITHOUT_PRIVATE_CANDIDATE: &[&CStr] = &[I, PROOF, TARGET, R];
const RENAME_NOREPLACE: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    Private,
    TargetArrived,
    BothArrived,
}

impl Place {
    pub(super) fn observe(selected: &Selected<'_>, journal: &HeldDir) -> io::Result<Self> {
        match (
            present(&selected.target_staging, &selected.names.target_stage)?,
            present(&selected.target_data, &selected.names.cid)?,
            present(journal, CANDIDATE)?,
            present(&selected.routes, &selected.names.route_stage)?,
        ) {
            (true, false, true, false) => Ok(Self::Private),
            (false, true, true, false) => Ok(Self::TargetArrived),
            (false, true, false, true) => Ok(Self::BothArrived),
            _ => Err(invalid(
                "mixed or unavailable DATA move READY positions are preserved",
            )),
        }
    }

    pub(super) fn layout(self) -> &'static [&'static CStr] {
        match self {
            Self::Private | Self::TargetArrived => HAS_READY,
            Self::BothArrived => WITHOUT_PRIVATE_CANDIDATE,
        }
    }

    pub(super) fn tree_site<'h>(self, selected: &'h Selected<'_>) -> (&'h HeldDir, &'h CStr) {
        match self {
            Self::Private => (&selected.target_staging, &selected.names.target_stage),
            Self::TargetArrived | Self::BothArrived => (&selected.target_data, &selected.names.cid),
        }
    }

    pub(super) fn candidate_site<'h>(self, armed: &'h Armed<'_, '_>) -> (&'h HeldDir, &'h CStr) {
        match self {
            Self::Private | Self::TargetArrived => (&armed.plan.journal.directory, CANDIDATE),
            Self::BothArrived => {
                let selected = armed.plan.selected;
                (&selected.routes, &selected.names.route_stage)
            }
        }
    }

    pub(super) fn require_absence(self, armed: &Armed<'_, '_>) -> io::Result<()> {
        let selected = armed.plan.selected;
        if self == Self::Private {
            absent(&selected.target_data, &selected.names.cid)?;
        } else {
            absent(&selected.target_staging, &selected.names.target_stage)?;
        }
        if self == Self::BothArrived {
            absent(&armed.plan.journal.directory, CANDIDATE)
        } else {
            absent(&selected.routes, &selected.names.route_stage)
        }
    }
}

impl HeldReady<'_, '_> {
    pub(in super::super) fn arrive(self) -> io::Result<Self> {
        self.arrive_with(File::sync_all)
    }

    pub(in super::super) fn arrive_with(
        self,
        sync_routes: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        let original = match self.place {
            Place::Private => self.place_target()?,
            Place::TargetArrived => self,
            Place::BothArrived => {
                self.prove_arrival()?;
                return Ok(self);
            }
        };
        original.place_candidate(sync_routes)
    }

    pub(super) fn prove_arrival(&self) -> io::Result<()> {
        self.require()?;
        let plan = &self.armed.plan;
        plan.source.require_same(&plan.selected.observe_source()?)?;
        self.draft.target.require_metadata(&self.armed)?;
        self.require_position()
    }

    fn place_target(mut self) -> io::Result<Self> {
        self.prove_arrival()?;
        let selected = self.armed.plan.selected;
        let from = Place::Private.tree_site(selected);
        let into = Place::TargetArrived.tree_site(selected);
        move_once(from, into, || self.require_position())?;
        let moved = child(into.0, into.1)?;
        let original = &self.draft.target.at;
        if original.entity != moved.entity || !original.same_mount(&moved)? {
            return Err(invalid(
                "the arrived DATA tree is not the original held staging tree",
            ));
        }
        self.draft.target.at = moved;
        self.place = Place::TargetArrived;
        // Both target DATA parents must be durable before moving the paper.
        self.settle_with(File::sync_all, File::sync_all)
    }

    fn place_candidate(
        mut self,
        sync_routes: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        self.prove_arrival()?;
        let from = Place::TargetArrived.candidate_site(&self.armed);
        let into = Place::BothArrived.candidate_site(&self.armed);
        move_once(from, into, || self.require_position())?;
        let moved = pin(into.0, into.1)?;
        same_file(into.0, &self.draft.candidate.file, &moved.file)?;
        // A rename can change ctime. Only the new pin's live stamp is used.
        require_candidate(&moved, into, self.checked.value())?;
        self.draft.candidate = moved;
        self.place = Place::BothArrived;
        self.settle_with(File::sync_all, sync_routes)
    }

    pub(super) fn sync_candidate_parent(
        &self,
        sync_routes: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.place != Place::BothArrived {
            return Ok(());
        }
        self.require_position()?;
        let plan = &self.armed.plan;
        // The old route is still authority. Do not flush a replacement
        // route through this shared parent after a slower scope check.
        plan.route.require(plan.selected)?;
        sync_routes(&plan.selected.routes.file)?;
        plan.route.require(plan.selected)?;
        self.require_position()
    }
}

fn present(parent: &HeldDir, name: &CStr) -> io::Result<bool> {
    parent.checked_child_path(name)?;
    parent.require_named()?;
    let found = parent.pin_record(name)?.is_some();
    parent.require_named()?;
    Ok(found)
}

fn move_once(
    (from, source): (&HeldDir, &CStr),
    (into, destination): (&HeldDir, &CStr),
    prove: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    from.checked_child_path(source)?;
    into.checked_child_path(destination)?;
    from.require_named()?;
    into.require_named()?;
    if !from.same_mount(into)? {
        return Err(invalid(
            "DATA move arrival cannot cross the held parent mount",
        ));
    }
    prove()?;
    absent(into, destination)?;
    // SAFETY: two held same-mount parent FDs and checked single components,
    // with a kernel-enforced no-replace cut. No ordinary route name is a destination.
    if unsafe {
        renameat2(
            from.file.as_raw_fd(),
            source.as_ptr(),
            into.file.as_raw_fd(),
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
