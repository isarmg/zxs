//! Expiring process-local full-body proof. An upload checkpoint is never
//! changed here, and this state cannot itself select or acknowledge a write.
use super::{
    Authorize, Failure, Result, journal::Slot, material::Stage, record::Scope, unavailable,
};
use crate::{
    container_store::{Body, Id, Mutation, invalid},
    fs::HeldDir,
    hash::Sha256,
};
use std::{
    collections::BTreeMap,
    fmt, io,
    sync::Mutex,
    time::{Duration, Instant},
};

const SLOTS: usize = 2;
const IDLE: Duration = Duration::from_secs(300);
const READ_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct Expired;

impl fmt::Display for Expired {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("upload verification authorization expired")
    }
}
impl std::error::Error for Expired {}

pub(super) fn is_expiry(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(<dyn std::error::Error + Send + Sync>::is::<Expired>)
}

/// Only a positively classified token expiry may suspend and retain a proof.
#[cfg(test)]
pub(super) fn expired() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, Expired)
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Key {
    pool: Id,
    epoch: Id,
    container: Id,
    operation: Id,
    upload: [u8; 32],
}

impl Key {
    pub(super) fn exact(scope: Scope, mutation: Mutation, upload: [u8; 32]) -> Self {
        Self {
            pool: scope.pool,
            epoch: scope.epoch,
            container: mutation.container,
            operation: mutation.request,
            upload,
        }
    }
}

#[derive(Default)]
pub(super) struct Bank(Mutex<BTreeMap<Key, Entry>>);

enum Entry {
    Running,
    Parked {
        since: Instant,
        witness: Box<Witness>,
    },
}

pub(super) struct Lease<'a> {
    bank: &'a Bank,
    key: Key,
    prior: Option<Box<Witness>>,
    running: bool,
}

impl Bank {
    pub(super) fn acquire(&self, key: Key) -> io::Result<Lease<'_>> {
        self.acquire_at(key, Instant::now())
    }

    pub(super) fn discard_idle(&self, key: Key) {
        let retired = self.0.lock().ok().and_then(|mut entries| {
            if matches!(entries.get(&key), Some(Entry::Parked { .. })) {
                entries.remove(&key)
            } else {
                None
            }
        });
        // A witness owns native FDs; closing them is not a bank-lock action.
        drop(retired);
    }

    fn acquire_at(&self, key: Key, now: Instant) -> io::Result<Lease<'_>> {
        let mut retired = Vec::with_capacity(SLOTS);
        let outcome = {
            let mut entries = self.0.lock().map_err(|_| unavailable())?;
            let expired = entries.iter().filter_map(|(key, entry)| {
                if matches!(entry, Entry::Parked { since, .. } if now.saturating_duration_since(*since) >= IDLE) {
                    Some(*key)
                } else {
                    None
                }
            }).collect::<Vec<_>>();
            for key in expired {
                retired.extend(entries.remove(&key));
            }
            if matches!(entries.get(&key), Some(Entry::Running))
                || entries.len() >= SLOTS && !entries.contains_key(&key)
            {
                Err(io::ErrorKind::WouldBlock.into())
            } else {
                let prior = match entries.insert(key, Entry::Running) {
                    Some(Entry::Parked { witness, .. }) => Some(witness),
                    _ => None,
                };
                Ok(Lease {
                    bank: self,
                    key,
                    prior,
                    running: true,
                })
            }
        };
        drop(retired);
        outcome
    }
}

impl Lease<'_> {
    pub(super) fn witness_or(
        &mut self,
        initialize: impl FnOnce() -> io::Result<Witness>,
    ) -> io::Result<Box<Witness>> {
        match self.prior.take() {
            Some(prior) => Ok(prior),
            None => initialize().map(Box::new),
        }
    }

    pub(super) fn pause(self, witness: Box<Witness>) -> io::Result<()> {
        self.pause_at(witness, Instant::now())
    }

    fn pause_at(mut self, witness: Box<Witness>, since: Instant) -> io::Result<()> {
        let mut entries = self.bank.0.lock().map_err(|_| unavailable())?;
        if !matches!(entries.get(&self.key), Some(Entry::Running)) {
            drop(entries);
            return Err(unavailable());
        }
        let retired = entries.insert(self.key, Entry::Parked { since, witness });
        self.running = false;
        drop(entries);
        drop(retired);
        Ok(())
    }
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if !self.running {
            return;
        }
        let retired = self
            .bank
            .0
            .lock()
            .ok()
            .and_then(|mut entries| entries.remove(&self.key));
        drop(retired);
    }
}

pub(super) struct Witness {
    slot: Slot,
    held: super::material::HeldBytes,
    state: HashState,
    #[cfg(test)]
    drop_probe: Option<tests::ObserveDrop>,
}

impl Witness {
    pub(super) fn capture(
        directory: &HeldDir,
        current: &Slot,
        staged: &Stage<'_>,
    ) -> io::Result<Self> {
        let this = Self {
            slot: current.duplicate(directory)?,
            held: staged.held_bytes()?,
            state: HashState::new(),
            #[cfg(test)]
            drop_probe: None,
        };
        this.require(directory, current, staged)?;
        Ok(this)
    }

    pub(super) fn require(
        &self,
        directory: &HeldDir,
        current: &Slot,
        staged: &Stage<'_>,
    ) -> io::Result<()> {
        self.slot.require_same(directory, current)?;
        self.held.require(staged)
    }

    pub(super) fn verify(
        &mut self,
        directory: &HeldDir,
        current: &Slot,
        staged: &Stage<'_>,
        authorize: Authorize<'_>,
    ) -> Result<()> {
        authorize()?;
        self.require(directory, current, staged)?;
        self.state.advance(
            &current.record.intent.body,
            |raw, at| staged.read_held_at(raw, at),
            authorize,
        )?;
        self.require(directory, current, staged)?;
        authorize()?;
        Ok(())
    }
}

enum HashState {
    Prefix { offset: u64, hash: Sha256 },
    Exact,
}

impl HashState {
    fn new() -> Self {
        Self::Prefix {
            offset: 0,
            hash: Sha256::new(),
        }
    }

    fn advance(
        &mut self,
        body: &Body,
        mut read: impl FnMut(&mut [u8], u64) -> io::Result<usize>,
        authorize: Authorize<'_>,
    ) -> Result<()> {
        let Self::Prefix { offset, hash } = self else {
            authorize()?;
            return Ok(());
        };
        if *offset > body.length {
            return Err(invalid("upload proof offset exceeds the original raw length").into());
        }
        let mut block = vec![0; READ_BYTES];
        while *offset < body.length {
            let max = usize::try_from((body.length - *offset).min(READ_BYTES as u64))
                .map_err(|_| invalid("invalid upload proof block length"))?;
            let got = observe(&mut read, &mut block[..max], *offset, authorize)?;
            if got == 0 {
                return Err(Failure::InvalidBody);
            }
            hash.update(
                block
                    .get(..got)
                    .filter(|_| got <= max)
                    .ok_or_else(|| invalid("invalid whole proof read width"))?,
            )?;
            *offset += got as u64;
        }
        if observe(&mut read, &mut [0; 1], body.length, authorize)? != 0 {
            return Err(Failure::InvalidBody);
        }
        if std::mem::replace(hash, Sha256::new()).finish() != body.sha256 {
            return Err(Failure::InvalidBody);
        }
        *self = Self::Exact;
        Ok(())
    }
}

fn observe(
    read: &mut impl FnMut(&mut [u8], u64) -> io::Result<usize>,
    output: &mut [u8],
    at: u64,
    authorize: Authorize<'_>,
) -> io::Result<usize> {
    loop {
        authorize()?;
        match read(output, at) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => {
                let bytes = result?;
                // The final syscall may have blocked across expiry. The
                // unaccepted bytes never advance the saved hash or offset.
                authorize()?;
                return Ok(bytes);
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resumable_proof.rs"]
mod tests;
