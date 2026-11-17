//! A bounded read-only byproduct of the mandatory native snapshot ledger pass.
//! Neither serialized nor an authority to install a State or mint an event.
use super::{DeletionOwner, LifecycleMembershipIndex, history_copy_owner, owner};
use crate::persist::path_changes::{self, PathChanged};
use crate::{OperationResult, Result, State};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use storage_contract::{OperationId, history_window::HistoryFileId};

const MAX_MEMBERS: usize = 2_048;
const MAX_PATH_BYTES: usize = 256 * 1024;
const PROCESS_BUDGET: usize = 4 * 1024 * 1024;
static RESERVED: AtomicUsize = AtomicUsize::new(0);

/// Recovery can temporarily retain more than one candidate. Bound all such
/// advisory summaries together; a miss changes no native admission decision.
#[derive(Default)]
struct Reservation(usize);

impl Reservation {
    fn grow(&mut self, bytes: usize) -> bool {
        if bytes == 0 {
            return true;
        }
        let reserved = RESERVED.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(bytes).filter(|next| *next <= PROCESS_BUDGET)
        });
        if reserved.is_err() {
            return false;
        }
        self.0 += bytes;
        true
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        RESERVED.fetch_sub(self.0, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Memberships {
    index: LifecycleMembershipIndex,
    reservation: Reservation,
}

impl Memberships {
    fn observe(&mut self, operation: OperationId, receipt: &OperationResult) -> bool {
        // Bound the optional owner classifier before it constructs a set.
        if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
            let names = [
                "source_deletions",
                "displaced_deletions",
                "imported_deletions",
                "retained_lifecycle",
            ];
            if names.iter().any(|name| {
                receipt.result.get(*name).and_then(json_codec::Value::as_array)
                    .is_some_and(|rows| rows.len() > MAX_MEMBERS)
            }) {
                return false;
            }
        }
        let deletion = owner(receipt);
        let copy = history_copy_owner(receipt);
        for (owner, is_copy) in [(&deletion, false), (&copy, true)] {
            let required = match owner {
                DeletionOwner::Other => 0,
                DeletionOwner::Malformed => return false,
                DeletionOwner::File(_) => 1,
                DeletionOwner::Files(files) => files.len(),
            };
            // A distinct FILE key can retain its own two maps and operation.
            if required > MAX_MEMBERS.saturating_sub(self.index.len())
                || !self.reservation.grow(required.saturating_mul(512))
                || !matches!(self.index.observe(operation, owner, is_copy), Ok(true))
            {
                return false;
            }
        }
        true
    }
}

type FilePaths = BTreeMap<HistoryFileId, Vec<PathChanged>>;

#[derive(Default)]
struct Paths {
    files: FilePaths,
    reservation: Reservation,
}

impl Paths {
    fn observe(&mut self, fact: PathChanged) -> Result<bool> {
        // Includes the canonical path spellings, derived lookup keys, and
        // room for map/vector growth; no raw receipt or disk buffer is kept.
        let bytes = fact.before.path.as_str().len()
            .saturating_add(fact.after.path.as_str().len())
            .saturating_mul(4)
            .saturating_add(fact.author.as_str().len())
            .saturating_add(768);
        if bytes > MAX_PATH_BYTES.saturating_sub(self.reservation.0)
            || !self.reservation.grow(bytes)
        {
            return Ok(false);
        }
        let file = HistoryFileId::parse(&fact.file_id.to_string())?;
        self.files.entry(file).or_default().push(fact);
        Ok(true)
    }
}

pub(crate) struct AdmittedLifecycle {
    members: Memberships,
    paths: Paths,
}

impl AdmittedLifecycle {
    pub(crate) fn admit(state: &State) -> Result<Option<Arc<Self>>> {
        let mut members = Memberships::default();
        // Account even for an empty snapshot or a large set of empty candidates.
        if !members.reservation.grow(384) {
            path_changes::admit_ledger(state)?;
            return Ok(None);
        }
        let mut members = Some(members);
        let mut paths = Some(Paths::default());
        path_changes::admit_indexed(
            state,
            |operation, receipt| {
                if members.as_mut().is_some_and(|index| !index.observe(operation, receipt)) {
                    members = None;
                }
                Ok(())
            },
            |fact| {
                if let Some(index) = &mut paths
                    && !index.observe(fact)?
                {
                    paths = None;
                }
                Ok(())
            },
        )?;
        Ok(members.zip(paths).map(|(members, paths)| Arc::new(Self { members, paths })))
    }

    pub(crate) fn source<'a>(
        &self,
        state: &'a State,
        file: &'a HistoryFileId,
    ) -> super::LifecycleSource<'a> {
        super::LifecycleSource::from_index(state, file, &self.members.index)
    }

    pub(crate) fn paths(&self, file: &HistoryFileId) -> &[PathChanged] {
        self.paths.files.get(file).map(Vec::as_slice).unwrap_or(&[])
    }
}
