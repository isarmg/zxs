//! Historical DONE holds only the current STATE and SYSTEM roles, never old DATA.
use super::super::super::super::super::{MAX_PRIVATE_ENTRIES, MOVE_PARENT, Names, Scope, child};
use super::super::super::super::publish;
use super::super::{R, READY_BYTES, TARGET};
use super::{
    CStr, CUT_BYTES, D, Digest, Expect, FINISHED, File, HeldDir, HeldGuard, I, INTENT_BYTES,
    Intent, Journal, JournalBytes, OpaquePin, PROOF, Phase, S, Selected, absent, birth, invalid,
    io, pin, read_raw, require_layout, same_file,
};
use crate::fs::ReadAccess;

const FIXED: [(&CStr, usize); 4] = [
    (I, INTENT_BYTES),
    (R, READY_BYTES),
    (S, CUT_BYTES),
    (D, CUT_BYTES),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Completion {
    Completed(Digest),
    Historical(Digest),
}

pub(in crate::container_store) struct HistoryRequest<'a> {
    pub(in crate::container_store) state: &'a HeldDir,
    pub(in crate::container_store) system: &'a HeldDir,
    pub(in crate::container_store) system_owner: &'a File,
    pub(in crate::container_store) identity: (Scope, Digest, (Digest, Digest)),
    pub(in crate::container_store) live: &'a dyn Fn() -> io::Result<()>,
}

impl<'a> HistoryRequest<'a> {
    pub(super) fn from_selection(selected: &'a Selected<'_>) -> Self {
        Self {
            state: selected.roots.state,
            system: selected.roots.system,
            system_owner: &selected.leases[0].original,
            identity: (selected.scope, selected.system_token, selected.operands),
            live: selected.outer,
        }
    }

    fn require(&self, names: &Names) -> io::Result<()> {
        (self.live)()?;
        self.state.require_named()?;
        self.system.require_named()?;
        let owner = pin(self.system, c"container-owner")?;
        if owner.file.metadata()?.len() != 0 {
            return Err(invalid(
                "historical move SYSTEM owner is not its original empty lease",
            ));
        }
        same_file(self.system, self.system_owner, &owner.file)?;
        owner.require(self.system, c"container-owner")?;
        let mut remaining = MAX_PRIVATE_ENTRIES;
        self.state.each_name(ReadAccess::NoAtime, |name| {
            remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| invalid("historical STATE inventory exceeds its bound"))?;
            if name.starts_with(b".server-stage-move-") && name != names.active.to_bytes() {
                return Err(invalid("another active or unknown DATA move guard exists"));
            }
            Ok(())
        })?;
        absent(self.state, c".server-stage-retire-data")?;
        absent(self.state, &names.inactive)
    }
}

pub(super) struct BoundDone<'a> {
    request: HistoryRequest<'a>,
    names: Names,
    parent: HeldDir,
    directory: HeldDir,
    routes: HeldDir,
    files: [OpaquePin; 4],
    raw: [Vec<u8>; 4],
    guard: Option<HeldGuard>,
    chain: Journal,
    id: Digest,
}

impl<'a> BoundDone<'a> {
    pub(super) fn open(request: HistoryRequest<'a>) -> io::Result<Self> {
        let names = Names::new(request.identity.0)?;
        request.require(&names)?;
        let parent = child(request.system, MOVE_PARENT)?;
        let directory = child(&parent, &names.operation)?;
        let routes = child(request.system, c"container-routes")?;
        require_layout(&directory, FINISHED)?;
        let files = [
            pin(&directory, I)?,
            pin(&directory, R)?,
            pin(&directory, S)?,
            pin(&directory, D)?,
        ];
        let raw = [
            read_raw(&files[0], INTENT_BYTES)?,
            read_raw(&files[1], READY_BYTES)?,
            read_raw(&files[2], CUT_BYTES)?,
            read_raw(&files[3], CUT_BYTES)?,
        ];
        let intent = Intent::decode(&raw[0])?;
        let guard = HeldGuard::open(request.state, &names.active, &intent)?;
        let chain = Journal::decode(JournalBytes {
            intent: &raw[0],
            guard: guard.as_ref().map(|g| g.raw.as_slice()),
            ready: Some(&raw[1]),
            switched: Some(&raw[2]),
            done: Some(&raw[3]),
        })?;
        let Phase::Done(id) = *chain.phase() else {
            return Err(invalid(
                "move history requires the exact original DONE lineage",
            ));
        };
        let result = Self {
            request,
            names,
            parent,
            directory,
            routes,
            files,
            raw,
            guard,
            chain,
            id,
        };
        result.require(false)?;
        Ok(result)
    }

    pub(super) fn done_file(&self) -> &File {
        &self.files[3].file
    }

    pub(super) fn release(
        self,
        before_release: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Digest> {
        self.require(false)?;
        self.done_file().sync_all()?;
        self.directory.file.sync_all()?;
        self.parent.file.sync_all()?;
        self.request.system.file.sync_all()?;
        self.require(false)?;
        before_release()?;
        self.require(false)?;
        let state = self.request.state;
        if let Some(original) = &self.guard {
            state.unlink_opaque_original(&self.names.active, &original.pin)?;
        } else {
            state.confirm_absence(&self.names.active)?;
        }
        self.require(true)?;
        Ok(self.id)
    }

    fn require(&self, released: bool) -> io::Result<()> {
        self.request.require(&self.names)?;
        self.parent.require_named()?;
        for name in [TARGET, c"new-route", super::super::R, S, D] {
            absent(
                &self.parent,
                &publish::scoped_leaf(self.request.identity.0, name, true)?,
            )?;
        }
        self.directory.require_named()?;
        self.routes.require_named()?;
        require_layout(&self.directory, FINISHED)?;
        let i = self.chain.intent().value();
        if (i.scope, i.system, (i.source_operand, i.target_operand)) != self.request.identity
            || i.state_parent != birth(&self.request.state.file)?
            || i.routes_parent != birth(&self.routes.file)?
            || i.journal != birth(&self.directory.file)?
        {
            return Err(invalid(
                "move DONE belongs to different original state, SYSTEM, or operands",
            ));
        }
        for ((name, len), (file, raw)) in FIXED.into_iter().zip(self.files.iter().zip(&self.raw)) {
            file.require(&self.directory, name)?;
            if read_raw(file, len)? != *raw {
                return Err(invalid("historical move record changed"));
            }
            file.require(&self.directory, name)?;
        }
        let expected_size = u64::try_from(Expect::source(self.chain.intent()).extent()?)
            .map_err(|_| invalid("invalid historical DATA move proof extent"))?;
        for name in [PROOF, TARGET] {
            let original = pin(&self.directory, name)?;
            if original.file.metadata()?.len() != expected_size {
                return Err(invalid("historical move proof has the wrong extent"));
            }
            original.require(&self.directory, name)?;
        }
        match (&self.guard, released) {
            (Some(guard), false) => guard.require(self.request.state, &self.names.active),
            _ => absent(self.request.state, &self.names.active),
        }
    }
}
