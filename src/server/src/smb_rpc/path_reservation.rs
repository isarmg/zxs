//! File handles and namespace mutations own one atomic path reservation.
use super::*;

fn overlaps(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|tail| tail.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub(super) struct PathReservation {
    shared: Arc<Shared>,
    keys: BTreeSet<(String, String)>,
}

impl PathReservation {
    // Callers project keys from admitted LookupPath/PortablePath values. No
    // raw protocol or disk spelling enters this registry without parsing.
    pub(super) fn acquire<'a>(
        shared: &Arc<Shared>,
        paths: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, ApiError> {
        let keys = paths
            .into_iter()
            .map(|(branch, key)| (branch.to_owned(), key.to_owned()))
            .collect::<BTreeSet<_>>();
        let mut writers = shared
            .file_writers
            .lock()
            .expect("invariant: service state lock poisoned");
        if writers.iter().any(|(branch, path)| {
            keys.iter()
                .any(|(candidate, key)| branch == candidate && overlaps(path, key))
        }) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::FileWriteBusy,
                "an affected SMB path has an active write handle or namespace operation",
            ));
        }
        // Do not acquire one side and then expose a partially reserved move.
        writers.extend(keys.iter().cloned());
        Ok(Self {
            shared: shared.clone(),
            keys,
        })
    }
}

impl Drop for PathReservation {
    fn drop(&mut self) {
        match self.shared.file_writers.lock() {
            Ok(mut writers) => {
                for key in &self.keys {
                    writers.remove(key);
                }
            }
            Err(_) => {
                tracing::error!("SMB path cleanup blocked by poisoned writer registry");
            }
        }
    }
}
