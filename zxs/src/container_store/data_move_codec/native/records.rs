//! One immutable INTENT and one native inactive-to-active interlock.
use super::super::super::{BYTES as ROUTE_BYTES, decode as decode_route};
use super::super::{GUARD_BYTES, INTENT_BYTES};
use super::{
    Birth, Checked, Digest, File, Guard, HeldDir, Intent, MOVE_PARENT, OpaquePin, ReadAccess,
    Selected, Source, absent, birth, invalid, no_replace, pin,
    proof::{Expect, Proof},
    read_raw, same_file,
};
use std::{
    ffi::CStr,
    io::{self, Write},
};

const I: &CStr = c"INTENT";
const PROOF: &CStr = c"source-proof";

struct Route {
    pin: OpaquePin,
    raw: [u8; ROUTE_BYTES],
    incarnation: Birth,
}

impl Route {
    fn original(selected: &Selected<'_>) -> io::Result<Self> {
        let name = &selected.names.cid;
        let pin = pin(&selected.routes, name)?;
        let raw = read_raw(&pin, ROUTE_BYTES)?
            .try_into()
            .map_err(|_| invalid("the original move CR1 extent is invalid"))?;
        if decode_route(raw)? != (selected.scope.container, selected.roots.data.0.token) {
            return Err(invalid("the original CR1 no longer selects this source"));
        }
        let incarnation = birth(&pin.file)?;
        let result = Self {
            pin,
            raw,
            incarnation,
        };
        result.require(selected)?;
        Ok(result)
    }

    fn require(&self, selected: &Selected<'_>) -> io::Result<()> {
        self.pin.require(&selected.routes, &selected.names.cid)?;
        if read_raw(&self.pin, ROUTE_BYTES)? != self.raw
            || birth(&self.pin.file)? != self.incarnation
        {
            return Err(invalid("the original held CR1 changed before DATA move"));
        }
        self.pin.require(&selected.routes, &selected.names.cid)
    }

    fn durable_vacancies(&self, selected: &Selected<'_>) -> io::Result<()> {
        self.durable_vacancies_with(selected, || selected.vacancies(true))
    }

    fn durable_vacancies_with(
        &self,
        selected: &Selected<'_>,
        flush: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        // The exact old CR1 must still be the selected incarnation after
        // any slow source/census/outer checks and before touching its parent.
        selected.require()?;
        self.require(selected)?;
        flush()?;
        self.require(selected)?;
        selected.vacancies(false)
    }
}

struct BoundJournal {
    parent: HeldDir,
    directory: HeldDir,
    original: Checked<Intent>,
    raw: [u8; INTENT_BYTES],
    record: OpaquePin,
    sidecar: OpaquePin,
}

impl BoundJournal {
    fn open(
        selected: &Selected<'_>,
        route: &Route,
        source: &Source,
        has_guard: bool,
    ) -> io::Result<Self> {
        let (parent, directory, fresh) = positions(selected, has_guard)?;
        let candidate = intend(selected, route, source, &directory)?;
        let raw = candidate.encode()?;
        let (record, sidecar) = if fresh {
            require_before_intent(selected, route, source)?;
            let sidecar = create(&directory, PROOF, &source.bytes)?;
            // Source proof can be larger than INTENT. Recheck its original
            // named inode, real prerequisites and durable emptiness after I/O.
            source.require_same(&selected.observe_source()?)?;
            require_before_intent(selected, route, source)?;
            sidecar.require(&directory, PROOF)?;
            (create(&directory, I, &raw)?, sidecar)
        } else {
            require_pre_ready_files(&directory)?;
            (pin(&directory, I)?, pin(&directory, PROOF)?)
        };
        require_pre_ready_files(&directory)?;
        let found = read_raw(&record, INTENT_BYTES)?;
        let original = Intent::decode(&found)?;
        if original.value() != &candidate || found != raw {
            return Err(invalid(
                "the immutable DATA move intent is not the same original",
            ));
        }
        let expected = Expect::source(&original);
        let found_proof = read_raw(&sidecar, expected.extent()?)?;
        Proof::decode(&found_proof, &expected)?;
        if found_proof != source.bytes {
            return Err(invalid(
                "the original source-proof differs from the native source",
            ));
        }
        let journal = Self {
            parent,
            directory,
            original,
            raw,
            record,
            sidecar,
        };
        journal.require()?;
        Ok(journal)
    }

    fn settle(&self, selected: &Selected<'_>) -> io::Result<()> {
        self.require()?;
        (selected.outer)()?;
        self.sidecar.file.sync_all()?;
        self.record.file.sync_all()?;
        self.directory.file.sync_all()?;
        self.parent.file.sync_all()?;
        selected.roots.system.file.sync_all()?;
        self.require()?;
        selected.require()
    }

    fn require(&self) -> io::Result<()> {
        self.parent.require_named()?;
        self.directory.require_named()?;
        require_pre_ready_files(&self.directory)?;
        self.sidecar.require(&self.directory, PROOF)?;
        self.record.require(&self.directory, I)?;
        if read_raw(&self.record, INTENT_BYTES)? != self.raw
            || birth(&self.directory.file)? != self.original.value().journal
        {
            return Err(invalid("the held immutable DATA move journal changed"));
        }
        self.record.require(&self.directory, I)
    }
}

fn positions(selected: &Selected<'_>, has_guard: bool) -> io::Result<(HeldDir, HeldDir, bool)> {
    let system = selected.roots.system;
    let parent = match system.child_directory(MOVE_PARENT)? {
        Some(existing) => existing,
        None if has_guard => return Err(invalid("a guard has no original DATA move journal")),
        None => {
            (selected.outer)()?;
            system.new_child_directory(MOVE_PARENT)?
        }
    };
    let name = &selected.names.operation;
    let (directory, fresh) = match parent.child_directory(name)? {
        Some(existing) => (existing, false),
        None if has_guard => return Err(invalid("a guard has no original DATA move intent")),
        None => {
            (selected.outer)()?;
            (parent.new_child_directory(name)?, true)
        }
    };
    Ok((parent, directory, fresh))
}

fn require_pre_ready_files(directory: &HeldDir) -> io::Result<()> {
    let mut files = (false, false);
    directory.each_name(ReadAccess::NoAtime, |name| match name {
        b"INTENT" if !files.0 => {
            files.0 = true;
            Ok(())
        }
        b"source-proof" if !files.1 => {
            files.1 = true;
            Ok(())
        }
        _ => Err(invalid("the DATA move journal is not strictly pre-READY")),
    })?;
    if files != (true, true) {
        return Err(invalid(
            "incomplete DATA move intent and proof are preserved",
        ));
    }
    Ok(())
}

fn require_before_intent(
    selected: &Selected<'_>,
    route: &Route,
    source: &Source,
) -> io::Result<()> {
    selected.pre_ready_names()?;
    route.durable_vacancies(selected)?;
    (selected.outer)()?;
    route.require(selected)?;
    source.require_named()?;
    selected.vacancies(false)
}

fn create(parent: &HeldDir, name: &CStr, bytes: &[u8]) -> io::Result<OpaquePin> {
    let mut output = parent.create_opaque_record(name)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    let selected = pin(parent, name)?;
    same_file(parent, &output, &selected.file)?;
    if read_raw(&selected, bytes.len())? != bytes {
        return Err(invalid("the prepared original DATA move record changed"));
    }
    selected.require(parent, name)?;
    Ok(selected)
}

fn intend(
    selected: &Selected<'_>,
    route: &Route,
    source: &Source,
    journal: &HeldDir,
) -> io::Result<Intent> {
    let (from, to) = selected.roots.data;
    Ok(Intent {
        scope: selected.scope,
        system: selected.system_token,
        source: from.token,
        target: to.token,
        source_operand: selected.operands.0,
        target_operand: selected.operands.1,
        state_parent: birth(&selected.roots.state.file)?,
        routes_parent: birth(&selected.routes.file)?,
        journal: birth(&journal.file)?,
        old_route: route.raw,
        old_birth: route.incarnation,
        source_inode: source.inode(),
        rows: source.count,
        source_proof: source.hash,
        ledger: source.ledger,
    })
}

pub(super) struct Pending<'held, 'scope> {
    selected: &'held Selected<'scope>,
    route: Route,
    source: Source,
    journal: BoundJournal,
}

impl<'held, 'scope> Pending<'held, 'scope> {
    pub(super) fn prepare(selected: &'held Selected<'scope>) -> io::Result<Self> {
        selected.pre_ready_names()?;
        let state = selected.roots.state;
        let has_guard = state.pin_record(&selected.names.active)?.is_some()
            || state.pin_record(&selected.names.inactive)?.is_some();
        let route = Route::original(selected)?;
        selected.vacancies(false)?;
        let source = selected.observe_source()?;
        route.require(selected)?;
        let journal = BoundJournal::open(selected, &route, &source, has_guard)?;
        let result = Self {
            selected,
            route,
            source,
            journal,
        };
        result.require_held()?;
        result.require_existing_guard_shape()?;
        result.route.durable_vacancies(selected)?;
        result.journal.settle(selected)?;
        result.require_held()?;
        Ok(result)
    }

    pub(super) fn id(&self) -> Digest {
        self.journal.original.id()
    }

    fn require_existing_guard_shape(&self) -> io::Result<()> {
        let selected = self.selected;
        let state = selected.roots.state;
        let intent = &self.journal.original;
        if HeldGuard::open(state, &selected.names.active, intent)?.is_some() {
            absent(state, &selected.names.inactive)
        } else {
            HeldGuard::open(state, &selected.names.inactive, intent).map(|_| ())
        }
    }

    fn require_held(&self) -> io::Result<()> {
        self.selected.require()?;
        self.journal.require()?;
        self.route.require(self.selected)?;
        self.source.require_named()?;
        let original = self.journal.original.value();
        if birth(&self.selected.roots.state.file)? != original.state_parent
            || birth(&self.selected.routes.file)? != original.routes_parent
        {
            return Err(invalid(
                "the original DATA move state or routes parent changed",
            ));
        }
        Ok(())
    }

    pub(super) fn arm(&self) -> io::Result<HeldGuard> {
        self.arm_with(File::sync_all)
    }

    fn arm_with(&self, sync_parent: impl FnOnce(&File) -> io::Result<()>) -> io::Result<HeldGuard> {
        let selected = self.selected;
        let state = selected.roots.state;
        let (draft, active) = (&selected.names.inactive, &selected.names.active);
        self.require_held()?;
        selected.pre_ready_names()?;
        self.source.require_same(&selected.observe_source()?)?;
        self.route.durable_vacancies(selected)?;
        let holding =
            if let Some(original) = HeldGuard::open(state, active, &self.journal.original)? {
                absent(state, draft)?;
                original
            } else {
                let prior = HeldGuard::open(state, draft, &self.journal.original)?
                    .map_or_else(|| self.stage_guard(state, draft), Ok)?;
                // A known visible but unconfirmed inactive prep is not yet G.
                prior.pin.file.sync_all()?;
                prior.require(state, draft)?;
                selected.pre_ready_names()?;
                self.source.require_same(&selected.observe_source()?)?;
                self.route.durable_vacancies(selected)?;
                state.confirm_absence(active)?;
                self.require_held()?;
                selected.vacancies(false)?;
                prior.require(state, draft)?;
                absent(state, active)?;
                no_replace(state, draft, active)?;
                let installed = self.active()?;
                same_file(state, &prior.pin.file, &installed.pin.file)?;
                if prior.checked.id() != installed.checked.id() {
                    return Err(invalid(
                        "active DATA move guard differs from its synced draft",
                    ));
                }
                installed
            };
        holding.require(state, active)?;
        absent(state, draft)?;
        holding.pin.file.sync_all()?;
        sync_parent(&state.file)?;
        self.require_held()?;
        holding.require(state, active)?;
        absent(state, draft)?;
        Ok(holding)
    }

    fn active(&self) -> io::Result<HeldGuard> {
        HeldGuard::open(
            self.selected.roots.state,
            &self.selected.names.active,
            &self.journal.original,
        )?
        .ok_or_else(|| invalid("the original active DATA move guard disappeared"))
    }

    fn stage_guard(&self, state: &HeldDir, draft: &CStr) -> io::Result<HeldGuard> {
        (self.selected.outer)()?;
        let mut original = state.create_opaque_record(draft)?;
        original.write_all(
            &Guard {
                birth: birth(&original)?,
            }
            .encode(&self.journal.original)?,
        )?;
        original.sync_all()?;
        let named = HeldGuard::open(state, draft, &self.journal.original)?
            .ok_or_else(|| invalid("the original inactive DATA move guard disappeared"))?;
        same_file(state, &original, &named.pin.file)?;
        named.require(state, draft)?;
        Ok(named)
    }
}

pub(super) struct HeldGuard {
    pin: OpaquePin,
    checked: Checked<Guard>,
    raw: [u8; GUARD_BYTES],
}

impl HeldGuard {
    fn open(state: &HeldDir, at: &CStr, intent: &Checked<Intent>) -> io::Result<Option<Self>> {
        let Some(pin) = state.opaque_pin_with(at, ReadAccess::NoAtime)? else {
            return Ok(None);
        };
        let raw: [u8; GUARD_BYTES] = read_raw(&pin, GUARD_BYTES)?
            .try_into()
            .map_err(|_| invalid("the private DATA move guard has the wrong extent"))?;
        let checked = Guard::decode(&raw, intent)?;
        let guard = Self { pin, checked, raw };
        guard.require(state, at)?;
        Ok(Some(guard))
    }

    fn require(&self, state: &HeldDir, name: &CStr) -> io::Result<()> {
        self.pin.require(state, name)?;
        if birth(&self.pin.file)? != self.checked.value().birth
            || read_raw(&self.pin, GUARD_BYTES)? != self.raw
        {
            return Err(invalid(
                "the named guard does not have the original DATA move incarnation",
            ));
        }
        self.pin.require(state, name)
    }

    pub(super) fn id(&self) -> Digest {
        self.checked.id()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/data_move_native.rs"]
mod tests;
