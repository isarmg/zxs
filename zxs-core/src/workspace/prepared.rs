//! A cold preview may authorize only its own never-armed full-file original.
use super::{
    BasicLane, Basis, ExistingBasicSession, Id, Intent, Lease, Original, PreparedCancellation,
    Resolution, Target, Workspace, db_basis, invalid, payload,
};
use crate::Result;
use std::{path::Path, sync::Arc};

/// A read-only preview of one ordinary freeze, bound to its original workspace.
pub struct PreparedSubmission {
    owner: Arc<()>,
    original: Original,
}

impl PreparedSubmission {
    #[must_use]
    pub fn intent(&self) -> &Intent {
        &self.original.intent
    }

    #[must_use]
    pub fn working_path(&self) -> &str {
        &self.original.path
    }

    #[must_use]
    pub fn basis(&self) -> Option<&Basis> {
        self.original.prior.as_ref()
    }
}

impl PreparedCancellation {
    #[must_use]
    pub fn basis(&self) -> Option<&Basis> {
        self.original.prior.as_ref()
    }

    /// The desktop ordinary lane does not take over a resumable original.
    #[must_use]
    pub fn is_ordinary_full_file(&self) -> bool {
        ordinary(&self.original)
    }
}

fn ordinary(original: &Original) -> bool {
    let intent = &original.intent;
    original.upload.is_none()
        && match (&intent.target, &original.prior) {
            (Target::FirstMain { .. }, None) => true,
            (
                Target::Branch {
                    branch,
                    expected_head,
                },
                Some(basis),
            ) => {
                basis.container == intent.container_id
                    && &basis.branch == branch
                    && &basis.node == expected_head
                    && basis.seen_sequence == intent.seen_sequence
            }
            _ => false,
        }
}

fn never_armed(original: &Original, phase: &str) -> Result<()> {
    if phase != "prepared" || !ordinary(original) {
        return Err(invalid(
            "only an ordinary full file proven never armed can use this choice",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) enum FullSend<'a> {
    Existing(&'a Id),
    Prepared(&'a PreparedSubmission),
}

impl<'a> FullSend<'a> {
    pub(super) fn operation(self) -> &'a Id {
        match self {
            Self::Existing(op) => op,
            Self::Prepared(selected) => &selected.intent().operation_id,
        }
    }

    pub(super) fn hold(
        self,
        workspace: &Workspace,
    ) -> Result<(Original, String, Option<u64>, Lease)> {
        if let Self::Prepared(selected) = self
            && !Arc::ptr_eq(&workspace.read_session_owner, &selected.owner)
        {
            return Err(invalid("the selected freeze belongs to another workspace"));
        }
        let op = self.operation();
        let (initial, _, _) = workspace.original(op)?;
        let lease = workspace.hold_container(&initial.intent.container_id)?;
        let (original, phase, prior) = workspace.original(op)?;
        if original != initial {
            return Err(invalid("the selected original record changed"));
        }
        self.require(&original, &phase)?;
        Ok((original, phase, prior, lease))
    }

    pub(super) fn require(self, current: &Original, phase: &str) -> Result<()> {
        if let Self::Prepared(selected) = self {
            never_armed(current, phase)?;
            if current != &selected.original {
                return Err(invalid("the exact reviewed freeze changed"));
            }
        }
        Ok(())
    }
}

impl Workspace {
    /// Inspect a never-armed ordinary freeze without a session, new capture, or full scan.
    ///
    /// # Errors
    /// Refuses a changed mapping or missing/replaced sealed file and all non-prepared lanes.
    /// Approval rechecks the same record under the container lease and verifies all bytes.
    pub fn review_prepared_submit(&self, op: &Id) -> Result<PreparedSubmission> {
        self.validate()?;
        let (original, phase, _) = self.original(op)?;
        never_armed(&original, &phase)?;
        if db_basis(&self.db, &original.path)? != original.prior {
            return Err(invalid("the original working mapping changed"));
        }
        let source = self.root.source(Path::new(&payload(op)))?;
        if source.before != original.sealed || source.before.length != original.intent.body.length {
            return Err(invalid("the exact originally frozen file is unavailable"));
        }
        Ok(PreparedSubmission {
            owner: Arc::clone(&self.read_session_owner),
            original,
        })
    }

    /// Consume one explicit approval of this previously never-armed ordinary freeze.
    ///
    /// # Errors
    /// Stops before mutation if the workspace, phase, exact original, receipt, or custody
    /// cannot be confirmed. Never switches a resumable or already attempted original to R3.
    pub async fn submit_prepared_existing(
        &mut self,
        session: &ExistingBasicSession,
        selected: PreparedSubmission,
        confirmed: bool,
    ) -> Result<Resolution> {
        self.submit_using(
            BasicLane::Existing(session),
            FullSend::Prepared(&selected),
            confirmed,
        )
        .await
    }
}

#[cfg(test)]
#[path = "prepared_tests.rs"]
mod tests;
