//! Advisory in-process capacity for an unrouted first intake only.
//! No operation receipt, DATA route, or ownership is minted here.
use super::Id;
use std::{collections::BTreeMap, io, sync::Mutex};

type Token = [u8; 32];
pub(super) type Sample = (Token, u64);

const MAX_INTAKES: usize = 64;
const HEADROOM: u64 = 1024 * 1024;

#[derive(Default)]
pub(super) struct Placement {
    active: Mutex<Active>,
}

#[derive(Default)]
struct Active {
    generation: u64,
    containers: BTreeMap<Id, Pending>,
}

struct Pending {
    token: Token,
    leases: usize,
    bytes: u64,
}

pub(super) struct Intake<'a> {
    placement: &'a Placement,
    container: Id,
    pub(super) token: Token,
    bytes: u64,
}

fn unavailable() -> io::Error {
    io::ErrorKind::NotConnected.into()
}

fn full() -> io::Error {
    io::ErrorKind::StorageFull.into()
}

fn busy() -> io::Error {
    io::ErrorKind::WouldBlock.into()
}

impl Placement {
    pub(super) fn generation(&self) -> io::Result<u64> {
        Ok(self.active.lock().map_err(|_| busy())?.generation)
    }

    /// Samples are taken outside the lock; a completed intake cannot make
    /// old free-byte samples valid after it released its accounting.
    pub(super) fn reserve(
        &self,
        generation: u64,
        id: Id,
        bytes: u64,
        samples: &[Sample],
    ) -> io::Result<Option<Intake<'_>>> {
        let mut active = self.active.lock().map_err(|_| busy())?;
        if active.generation != generation {
            return Ok(None);
        }
        if active
            .containers
            .values()
            .map(|row| row.leases)
            .sum::<usize>()
            >= MAX_INTAKES
        {
            return Err(busy());
        }
        let needed = bytes.checked_add(HEADROOM).ok_or_else(full)?;
        let chosen = active.containers.get(&id).map(|row| row.token);
        let Some(token) = active.select(chosen, needed, samples)? else {
            return Err(full());
        };
        let next = active.generation.checked_add(1).ok_or_else(busy)?;
        let row = active.containers.entry(id).or_insert(Pending {
            token,
            leases: 0,
            bytes: 0,
        });
        row.bytes = row.bytes.checked_add(bytes).ok_or_else(full)?;
        row.leases += 1;
        active.generation = next;
        Ok(Some(Intake {
            placement: self,
            container: id,
            token,
            bytes,
        }))
    }
}

impl Active {
    fn select(
        &self,
        chosen: Option<Token>,
        needed: u64,
        samples: &[Sample],
    ) -> io::Result<Option<Token>> {
        let mut seen = false;
        let mut best = None;
        for &(token, sample) in samples {
            if chosen.is_some_and(|fixed| fixed != token) {
                continue;
            }
            seen = true;
            let reserved = self
                .containers
                .values()
                .filter(|row| row.token == token)
                .map(|row| row.bytes)
                .fold(0_u64, u64::saturating_add);
            let free = sample.saturating_sub(reserved);
            if free >= needed
                && best.is_none_or(|(previous, key)| {
                    free > previous || (free == previous && token < key)
                })
            {
                best = Some((free, token));
            }
        }
        if !seen {
            return Err(unavailable());
        }
        Ok(best.map(|(_, token)| token))
    }
}

impl Drop for Intake<'_> {
    fn drop(&mut self) {
        let Ok(mut active) = self.placement.active.lock() else {
            return;
        };
        let Some(row) = active.containers.get_mut(&self.container) else {
            return;
        };
        row.leases -= 1;
        row.bytes -= self.bytes;
        if row.leases == 0 {
            active.containers.remove(&self.container);
        }
        active.generation = active.generation.saturating_add(1);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/placement.rs"]
mod tests;
