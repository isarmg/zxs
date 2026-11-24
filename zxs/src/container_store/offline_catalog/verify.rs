//! Read and attest one exact existing container, never publish or reconcile it.
use super::{ACCESS, Namespace};
use crate::{
    container_store::{
        Change, DataRoot, Error, Id, Receipt, State, entry_leaf, held_entry, hex, invalid,
        recycle::{self, RecycleReceipt},
        recycle_receipt::RecycleRole,
        replay_selected_entries, routes,
    },
    fs::{Entity, HeldDir, OPAQUE_STAMP_BYTES, OpaquePin},
};
use std::{ffi::CString, io};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VerifiedLocation {
    Data,
    Recycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Verification {
    pub(crate) location: VerifiedLocation,
    pub(crate) receipts: u64,
    pub(crate) bodies: u64,
    pub(crate) bytes: u64,
}

struct Catalog<'a> {
    system: &'a HeldDir,
    enrolled: &'a [DataRoot],
    role: RecycleRole,
    routes: HeldDir,
    moves: Namespace<'a>,
    arrived: Namespace<'a>,
}

enum Position {
    Data(HeldDir),
    Recycle(RecycleReceipt),
}

struct Located {
    token: [u8; 32],
    position: Position,
    container: HeldDir,
}

impl Located {
    fn require_named(&self) -> io::Result<()> {
        if let Position::Data(parent) = &self.position {
            parent.require_named()?;
        }
        self.container.require_named()
    }

    fn require_same(&self, now: &Self) -> io::Result<()> {
        let position_same = match (&self.position, &now.position) {
            (Position::Data(a), Position::Data(b)) => a.entity == b.entity && a.same_mount(b)?,
            (Position::Recycle(a), Position::Recycle(b)) => a == b,
            _ => false,
        };
        if self.token != now.token
            || !position_same
            || self.container.entity != now.container.entity
            || !self.container.same_mount(&now.container)?
        {
            return Err(invalid("verified container selection changed"));
        }
        self.require_named()
    }

    fn ledger(&self, id: Id) -> io::Result<State> {
        self.require_named()?;
        let state = replay_selected_entries(&self.container, id, ACCESS)?;
        if state.name.is_none() {
            return Err(invalid("container is not a completed original"));
        }
        if let Position::Recycle(receipt) = &self.position
            && (state.sequence != receipt.original_sequence
                || state.receipts.contains_key(&receipt.mutation.request))
        {
            return Err(invalid("archived ledger differs from its exact selector"));
        }
        self.require_named()?;
        Ok(state)
    }
}

impl<'a> Catalog<'a> {
    fn open(
        system: &'a HeldDir,
        recycle_root: &'a HeldDir,
        enrolled: &'a [DataRoot],
        role: RecycleRole,
    ) -> io::Result<Self> {
        role.require()?;
        let routes = routes::existing_directory(system)?
            .ok_or_else(|| invalid("selected route directory is absent"))?;
        let moves = Namespace::open(system, c"container-moves")?;
        let arrived = Namespace::open(recycle_root, c"containers")?;
        Ok(Self {
            system,
            enrolled,
            role,
            routes,
            moves,
            arrived,
        })
    }

    fn locate(&self, id: Id) -> io::Result<Located> {
        self.routes.require_named()?;
        self.moves.verify()?;
        self.arrived.verify()?;
        let token = routes::lookup_route(&self.routes, id, ACCESS)?
            .ok_or_else(|| invalid("container has no original DATA route"))?;
        let data = self.enrolled.iter().find(|volume| volume.token == token);
        let selector = if let Some(moves) = &self.moves.selected {
            recycle::read_selector(moves, id, ACCESS)?
        } else {
            None
        };
        let (position, container) = if let Some(receipt) = selector {
            receipt.require_data(token)?;
            if data.is_none()
                && !recycle::read_retired_data(self.system, self.role.pool, token, ACCESS)?
            {
                return Err(invalid("archived original DATA token is unknown"));
            }
            let arrived = self
                .arrived
                .selected
                .as_ref()
                .ok_or_else(|| invalid("selected archive has no recycle directory"))?;
            let container = recycle::qualify_arrival(arrived, receipt, self.role, ACCESS)?;
            (Position::Recycle(receipt), container)
        } else {
            let root = &data
                .ok_or_else(|| invalid("original DATA token is not enrolled"))?
                .root;
            let parent = root
                .child_directory(c"data")?
                .ok_or_else(|| invalid("original DATA content directory is absent"))?;
            let name =
                CString::new(hex(id)).map_err(|_| invalid("invalid exact container name"))?;
            let container = parent
                .child_directory(&name)?
                .ok_or_else(|| invalid("route has no selected container"))?;
            (Position::Data(parent), container)
        };
        self.routes.require_named()?;
        self.moves.verify()?;
        self.arrived.verify()?;
        Ok(Located {
            token,
            position,
            container,
        })
    }
}

struct Entry {
    original: HeldDir,
    receipt: OpaquePin,
    body: Option<OpaquePin>,
}

#[derive(Eq, PartialEq)]
struct Witness {
    directory: Entity,
    receipt: [u8; OPAQUE_STAMP_BYTES],
    body: Option<[u8; OPAQUE_STAMP_BYTES]>,
}

impl Entry {
    fn open(container: &HeldDir, stored: &Receipt) -> io::Result<Self> {
        let original = held_entry(
            container,
            &entry_leaf(stored.sequence, stored.mutation.request),
        )?;
        let wants_body = matches!(stored.change, Change::Save { .. });
        original.each_name(ACCESS, |name| {
            if name == b"receipt" || wants_body && name == b"payload" {
                Ok(())
            } else {
                Err(invalid("unknown member in exact container verification"))
            }
        })?;
        let receipt = original
            .opaque_pin_with(c"receipt", ACCESS)?
            .ok_or_else(|| invalid("verified receipt is absent"))?;
        let body = if wants_body {
            Some(
                original
                    .opaque_pin_with(c"payload", ACCESS)?
                    .ok_or_else(|| invalid("verified body is absent"))?,
            )
        } else {
            None
        };
        Ok(Self {
            original,
            receipt,
            body,
        })
    }

    fn witness(&self) -> io::Result<Witness> {
        self.receipt.require(&self.original, c"receipt")?;
        if let Some(body) = &self.body {
            body.require(&self.original, c"payload")?;
        }
        Ok(Witness {
            directory: self.original.entity,
            receipt: self.receipt.stamp(),
            body: self.body.as_ref().map(OpaquePin::stamp),
        })
    }
}

fn copy_error(error: Error) -> io::Error {
    match error {
        Error::Io(error) => error,
        _ => invalid("verified payload differs from its original length or digest"),
    }
}

/// The exact exclusive offline state lease is retained by the caller.
/// This entrypoint does not create, repair, select, sync or remove anything.
pub(crate) fn verify_container(
    system: &HeldDir,
    recycle_root: &HeldDir,
    enrolled: &[DataRoot],
    pool: Id,
    recycle_token: [u8; 32],
    id: Id,
    require_original: impl Fn() -> io::Result<()>,
) -> io::Result<Verification> {
    require_original()?;
    recycle::Recycle::require_roles(system, recycle_root, enrolled)?;
    let catalog = Catalog::open(
        system,
        recycle_root,
        enrolled,
        RecycleRole {
            pool,
            token: recycle_token,
        },
    )?;
    let report = inspect(&catalog, id, &require_original)?;
    recycle::Recycle::require_roles(system, recycle_root, enrolled)?;
    require_original()?;
    Ok(report)
}

fn inspect(
    catalog: &Catalog<'_>,
    id: Id,
    require_original: &impl Fn() -> io::Result<()>,
) -> io::Result<Verification> {
    let selected = catalog.locate(id)?;
    let state = selected.ledger(id)?;
    let mut report = Verification {
        location: match &selected.position {
            Position::Data(_) => VerifiedLocation::Data,
            Position::Recycle(_) => VerifiedLocation::Recycle,
        },
        receipts: u64::try_from(state.receipts.len())
            .map_err(|_| invalid("receipt count overflow"))?,
        bodies: 0,
        bytes: 0,
    };
    let mut witnesses = Vec::new();
    witnesses
        .try_reserve_exact(state.receipts.len())
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    for receipt in state.receipts.values() {
        require_original()?;
        let entry = Entry::open(&selected.container, receipt)?;
        if let Change::Save { body, .. } = &receipt.change {
            let source = entry
                .body
                .as_ref()
                .ok_or_else(|| invalid("verified payload is absent"))?;
            let next = report
                .bytes
                .checked_add(body.length)
                .ok_or_else(|| invalid("whole byte count overflow"))?;
            body.copy_exact(&mut &source.file, &mut io::sink(), || {
                selected.require_named()
            })
            .map_err(copy_error)?;
            report.bodies += 1;
            report.bytes = next;
        }
        witnesses.push(entry.witness()?);
    }
    if report.bodies == 0 || report.bodies > report.receipts {
        return Err(invalid("no complete first body was verified"));
    }
    let now = catalog.locate(id)?;
    selected.require_same(&now)?;
    let observed = now.ledger(id)?;
    if observed.receipts != state.receipts {
        return Err(invalid("canonical receipts changed during verification"));
    }
    for (receipt, witness) in observed.receipts.values().zip(witnesses.iter()) {
        if Entry::open(&now.container, receipt)?.witness()? != *witness {
            return Err(invalid("exact verified file or entry was replaced"));
        }
    }
    selected.require_same(&catalog.locate(id)?)?;
    require_original()?;
    Ok(report)
}

#[cfg(test)]
#[path = "../../../tests/unit/offline_verify.rs"]
mod tests;
