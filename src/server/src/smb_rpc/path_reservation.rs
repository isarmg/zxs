//! Two-branch namespace mutations reserve both paths in one writer-registry step.
use super::*;

pub(super) fn overlaps(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|tail| tail.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub(super) struct MovePaths {
    shared: Arc<Shared>,
    keys: BTreeSet<(String, String)>,
}

impl MovePaths {
    pub(super) fn acquire(
        shared: &Arc<Shared>,
        paths: [(&str, &str); 2],
    ) -> Result<Self, ApiError> {
        let keys = paths
            .into_iter()
            .map(|(branch, path)| (branch.to_owned(), name_policy::path_key(path)))
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

impl Drop for MovePaths {
    fn drop(&mut self) {
        match self.shared.file_writers.lock() {
            Ok(mut writers) => {
                for key in &self.keys {
                    writers.remove(key);
                }
            }
            Err(_) => {
                tracing::error!("move path cleanup blocked by poisoned writer registry");
            }
        }
    }
}
