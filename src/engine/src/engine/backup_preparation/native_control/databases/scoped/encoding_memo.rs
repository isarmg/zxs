//! Request-local pure SQLite bytes. This is never admission or a verification result.
use crate::{Result, history_package::preparation_checkpoint};
use data_serialization::Serialize;
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    System,
    DiskPair,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    context: [u8; 32],
    input: [u8; 32],
    kind: Kind,
}

#[derive(Clone)]
enum Bytes {
    System(Arc<[u8]>),
    DiskPair(Arc<[u8]>, Arc<[u8]>),
}

#[derive(Default)]
struct State {
    entries: BTreeMap<Key, Bytes>,
    bytes: usize,
    reserved_bytes: usize,
    reserved_entries: usize,
}

/// The account includes key/value storage and payload bytes. Tree/allocator
/// overhead has a separate fixed entry bound; original Scopes output is separate.
pub(super) struct ScopeEncodingMemo {
    state: Mutex<State>,
    maximum_bytes: usize,
    maximum_entries: usize,
}

impl ScopeEncodingMemo {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            maximum_bytes: MAX_BYTES,
            maximum_entries: MAX_ENTRIES,
        }
    }

    fn get(&self, key: &Key) -> Option<Bytes> {
        self.state.lock().expect("encoding memo lock poisoned").entries.get(key).cloned()
    }

    fn reserve(&self, key: &Key, bytes: usize) -> bool {
        let mut state = self.state.lock().expect("encoding memo lock poisoned");
        if state.entries.contains_key(key)
            || state.entries.len() + state.reserved_entries >= self.maximum_entries
            || state.bytes.checked_add(state.reserved_bytes)
                .and_then(|current| current.checked_add(bytes))
                .is_none_or(|total| total > self.maximum_bytes)
        {
            return false;
        }
        state.reserved_bytes += bytes;
        state.reserved_entries += 1;
        true
    }
}

/// New material remains pending until the complete capture has passed its final
/// binding/cancellation checks. Dropping a failed capture releases reservations.
pub(super) struct Session<'a> {
    memo: Option<&'a ScopeEncodingMemo>,
    context: [u8; 32],
    pending: BTreeMap<Key, Bytes>,
    reserved_bytes: usize,
    reserved_entries: usize,
}

impl<'a> Session<'a> {
    pub(super) fn new(memo: Option<(&'a ScopeEncodingMemo, [u8; 32])>) -> Self {
        Self {
            memo: memo.map(|(memo, _)| memo),
            context: memo.map(|(_, context)| context).unwrap_or([0; 32]),
            pending: BTreeMap::new(),
            reserved_bytes: 0,
            reserved_entries: 0,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.memo.is_some()
    }

    pub(super) fn system(
        &mut self,
        input: [u8; 32],
        encode: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let key = Key { context: self.context, input, kind: Kind::System };
        if let Some(Bytes::System(bytes)) = self.memo.and_then(|memo| memo.get(&key)) {
            return Ok(bytes.as_ref().to_vec());
        }
        let bytes = encode()?;
        if self.reserve(key, bytes.len()) {
            self.pending.insert(key, Bytes::System(Arc::from(bytes.as_slice())));
        }
        Ok(bytes)
    }

    pub(super) fn disk_pair(
        &mut self,
        input: Option<[u8; 32]>,
        encode: impl FnOnce() -> Result<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let Some(input) = input.filter(|_| self.enabled()) else {
            return encode();
        };
        let key = Key { context: self.context, input, kind: Kind::DiskPair };
        if let Some(Bytes::DiskPair(current, history)) = self.memo.and_then(|memo| memo.get(&key)) {
            return Ok((current.as_ref().to_vec(), history.as_ref().to_vec()));
        }
        let (current, history) = encode()?;
        if let Some(bytes) = current.len().checked_add(history.len()) {
            if self.reserve(key, bytes) {
                self.pending.insert(
                    key,
                    Bytes::DiskPair(Arc::from(current.as_slice()), Arc::from(history.as_slice())),
                );
            }
        }
        Ok((current, history))
    }

    fn reserve(&mut self, key: Key, payload: usize) -> bool {
        let Some(memo) = self.memo else {
            return false;
        };
        let Some(bytes) = payload.checked_add(std::mem::size_of::<(Key, Bytes)>()) else {
            return false;
        };
        if self.pending.contains_key(&key) || !memo.reserve(&key, bytes) {
            return false;
        }
        self.reserved_bytes += bytes;
        self.reserved_entries += 1;
        true
    }

    /// Call only after the caller's complete fresh readback, binding and stop
    /// checks. No encoding closure or filesystem access runs under this lock.
    pub(super) fn commit(mut self) {
        let Some(memo) = self.memo else {
            return;
        };
        let mut state = memo.state.lock().expect("encoding memo lock poisoned");
        state.reserved_bytes -= self.reserved_bytes;
        state.reserved_entries -= self.reserved_entries;
        self.reserved_bytes = 0;
        self.reserved_entries = 0;
        for (key, value) in std::mem::take(&mut self.pending) {
            if !state.entries.contains_key(&key) {
                let payload = match &value {
                    Bytes::System(bytes) => bytes.len(),
                    Bytes::DiskPair(current, history) => current.len() + history.len(),
                };
                state.bytes += payload + std::mem::size_of::<(Key, Bytes)>();
                state.entries.insert(key, value);
            }
        }
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if self.reserved_entries == 0 {
            return;
        }
        if let Some(memo) = self.memo {
            let mut state = memo.state.lock().expect("encoding memo lock poisoned");
            state.reserved_bytes -= self.reserved_bytes;
            state.reserved_entries -= self.reserved_entries;
        }
    }
}

/// Hash every serialized input field without retaining a second full input
/// image. Oversize memo input declines caching, not the operation.
pub(super) fn digest(input: &impl Serialize, stop: &AtomicBool) -> Result<Option<[u8; 32]>> {
    digest_bounded(input, stop, MAX_INPUT_BYTES)
}

fn digest_bounded(
    input: &impl Serialize,
    stop: &AtomicBool,
    maximum: usize,
) -> Result<Option<[u8; 32]>> {
    struct Sink<'a> {
        hash: blake3::Hasher,
        bytes: usize,
        stop: &'a AtomicBool,
        failure: Option<crate::Error>,
        oversized: bool,
        maximum: usize,
    }
    impl Write for Sink<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Err(error) = preparation_checkpoint(self.stop) {
                self.failure = Some(error);
                // Write::write_all retries Interrupted; use a terminal I/O error and
                // return the original typed cancellation immediately after encoding.
                return Err(std::io::Error::other("memo digest cancelled"));
            }
            let Some(total) = self.bytes.checked_add(bytes.len())
                .filter(|total| *total <= self.maximum) else {
                self.oversized = true;
                return Err(std::io::ErrorKind::FileTooLarge.into());
            };
            self.bytes = total;
            for chunk in bytes.chunks(32 * 1024) {
                if let Err(error) = preparation_checkpoint(self.stop) {
                    self.failure = Some(error);
                    return Err(std::io::Error::other("memo digest cancelled"));
                }
                self.hash.update(chunk);
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    preparation_checkpoint(stop)?;
    let mut sink = Sink {
        hash: blake3::Hasher::new(),
        bytes: 0,
        stop,
        failure: None,
        oversized: false,
        maximum,
    };
    let encoded = json_codec::to_writer(&mut sink, input);
    if let Some(error) = sink.failure {
        return Err(error);
    }
    preparation_checkpoint(stop)?;
    if sink.oversized {
        return Ok(None);
    }
    encoded?;
    Ok(Some(*sink.hash.finalize().as_bytes()))
}


/// Pending encodings for one complete registry verification. The binding
/// digest is lazy: callers retain their original I/O and failure order.
pub(super) struct Transaction<'a> {
    memo: Option<&'a ScopeEncodingMemo>,
    binding: &'a dyn Fn(&AtomicBool) -> Result<Option<[u8; 32]>>,
    pending: Vec<Session<'a>>,
}

impl<'a> Transaction<'a> {
    pub(super) fn new(
        memo: Option<&'a ScopeEncodingMemo>,
        binding: &'a dyn Fn(&AtomicBool) -> Result<Option<[u8; 32]>>,
    ) -> Self {
        Self {
            memo,
            binding,
            pending: Vec::new(),
        }
    }

    pub(super) fn session(
        &self,
        input: &impl Serialize,
        stop: &AtomicBool,
    ) -> Result<Session<'a>> {
        let context = if self.memo.is_some() {
            match (self.binding)(stop)? {
                Some(binding) => digest(
                    &("zxs-backup-original-scoped-sqlite-encoding-v1", binding, input),
                    stop,
                )?,
                None => None,
            }
        } else {
            None
        };
        Ok(Session::new(self.memo.zip(context)))
    }

    pub(super) fn retain(&mut self, session: Session<'a>) {
        // Empty sessions retain no work. Every retained session owns at least
        // one reservation from the single shared byte and entry account.
        if session.reserved_entries != 0 {
            self.pending.push(session);
        }
    }

    pub(super) fn commit(self) {
        for session in self.pending {
            session.commit();
        }
    }
}

#[cfg(test)]
#[path = "../../../../../../../../test/engine/unit/backup_scope_encoding_memo_tests.rs"]
mod tests;
