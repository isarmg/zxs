//! The actual replay cursor may outlive startup only with its exact installed
//! prefix and admitted environment. This is read evidence, never a writer or
//! a replacement for the original Candidate/material and format gates.
use crate::disk::DiskResources;
use crate::engine::{OriginalReceiptCursor, PlanningContext};
use crate::{Error, OriginalReceiptError, Result, State, ValidatedConfig};
use std::sync::{Arc, Weak};

#[derive(Clone)]
pub(super) struct ReceiptEnvironment {
    config: ValidatedConfig,
    resources: Weak<DiskResources>,
}

impl ReceiptEnvironment {
    pub(super) fn capture(config: &ValidatedConfig, resources: &Arc<DiskResources>) -> Self {
        Self { config: config.clone(), resources: Arc::downgrade(resources) }
    }

    pub(super) fn require(
        &self, config: &ValidatedConfig, resources: &Arc<DiskResources>,
    ) -> Result<()> {
        if !self.config.same_admission(config)
            || !Weak::ptr_eq(&self.resources, &Arc::downgrade(resources))
        {
            return Err(Error::OriginalReceipt(OriginalReceiptError::Prefix));
        }
        Ok(())
    }
}

/// A snapshot identity copied only at an observed-prefix installation, not
/// parsed from a request or compared as an alternate source of authority.
#[derive(Clone)]
enum ReceiptSnapshot {
    Genesis,
    Published(Box<str>),
}

impl ReceiptSnapshot {
    fn capture(snapshot: &Option<String>) -> Self {
        match snapshot {
            Some(snapshot) => Self::Published(snapshot.as_str().into()),
            None => Self::Genesis,
        }
    }

    fn matches(&self, snapshot: &Option<String>) -> bool {
        match (self, snapshot) {
            (Self::Genesis, None) => true,
            (Self::Published(expected), Some(actual)) => expected.as_ref() == actual.as_str(),
            (Self::Genesis, Some(_)) | (Self::Published(_), None) => false,
        }
    }
}

/// Private fields, no Deserialize, no State/Config-only constructor. The cursor
/// retains only actual original/outcome pairs; this binding prevents reusing
/// them after an unobserved State, resource or generation replacement.
pub(crate) struct InstalledOriginalReceipts {
    environment: ReceiptEnvironment,
    cursor: Arc<OriginalReceiptCursor>,
    prefix: Weak<State>,
    snapshot: ReceiptSnapshot,
}

impl OriginalReceiptCursor {
    pub(in crate::engine) fn install(
        self: &Arc<Self>, config: &ValidatedConfig, resources: &Arc<DiskResources>,
        prefix: &Arc<State>, snapshot: &Option<String>,
    ) -> Result<Option<Arc<InstalledOriginalReceipts>>> {
        self.require_directory_index(prefix)?;
        if !self.records.values().any(|pair| pair.sequence().get() <= prefix.sequence) {
            // No indexed records exist. Missing originals for any indexed
            // record were rejected above, never decoded from visible State.
            return Ok(None);
        }
        let environment = self.environment.as_ref()
            .ok_or(Error::OriginalReceipt(OriginalReceiptError::Prefix))?;
        environment.require(config, resources)?;
        Ok(Some(Arc::new(InstalledOriginalReceipts {
            environment: environment.clone(),
            cursor: Arc::clone(self),
            prefix: Arc::downgrade(prefix),
            snapshot: ReceiptSnapshot::capture(snapshot),
        })))
    }
}

impl InstalledOriginalReceipts {
    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Weak::ptr_eq(&self.prefix, &Arc::downgrade(context.state))
            || !self.snapshot.matches(context.snapshot)
        {
            return Err(Error::OriginalReceipt(OriginalReceiptError::Prefix));
        }
        self.environment.require(context.config, context.disk_resources)
    }

    /// Read only the original decision/actor for this operation from the
    /// actual installed prefix. A visible recovered result is not an original.
    pub(in crate::engine) fn file_decision<'a>(
        &'a self, context: &PlanningContext<'_>, operation: storage_contract::OperationId,
    ) -> Result<(&'a crate::OperationResult, &'a storage_contract::Principal)> {
        self.require_context(context)?;
        self.cursor.receipt(context.state, operation)
    }

    pub(in crate::engine) fn automatic_after<'a>(
        &'a self, context: &PlanningContext<'_>, after: Option<storage_contract::automatic_observation::NodeKey>,
    ) -> Result<impl Iterator<Item = (storage_contract::automatic_observation::NodeKey, storage_contract::OperationId)> + 'a> {
        use std::ops::Bound;
        self.require_context(context)?;
        let limit = context.state.sequence;
        // Include the cursor row itself: the caller must verify it is an
        // authentic member of the original window, not just an ordered key.
        Ok(self.cursor.automatic.range((after.map_or(Bound::Unbounded, Bound::Included), Bound::Unbounded))
            .filter(move |(_, (seq, _))| *seq <= limit).map(|(key, (_, op))| (*key, *op)))
    }

    pub(in crate::engine) fn replay_cursor(
        &self, context: &PlanningContext<'_>,
    ) -> Result<OriginalReceiptCursor> {
        self.require_context(context)?;
        // Share only observations reached by this installed prefix. A pending
        // later slot must again cross its actual barrier, not be preinstalled.
        let records = self.cursor.records.iter().filter(|(_, pair)| {
            pair.sequence().get() <= context.state.sequence
        }).map(|(operation, pair)| (*operation, Arc::clone(pair))).collect();
        let automatic = if self.cursor.automatic.values().all(|(seq, _)| *seq <= context.state.sequence) {
            Arc::clone(&self.cursor.automatic)
        } else {
            Arc::new(self.cursor.automatic.iter().filter(|(_, (seq, _))| *seq <= context.state.sequence)
                .map(|(key, value)| (*key, *value)).collect())
        };
        Ok(OriginalReceiptCursor { environment: Some(self.environment.clone()), records, automatic })
    }

    pub(crate) fn index<'a>(
        &'a self, context: &PlanningContext<'a>,
    ) -> std::result::Result<
        crate::snapshot_worktree::ReceiptIndex<'a>, crate::snapshot_worktree::RegistrationError,
    > {
        self.require_context(context)
            .map_err(crate::snapshot_worktree::RegistrationError::OriginalReceipt)?;
        // This is a complete index, not a scoped decoder that could skip an
        // earlier conflicting event. It borrows the same owned cursor records.
        crate::snapshot_worktree::ReceiptIndex::capture_with_originals(context.state, &self.cursor)
    }

    pub(crate) fn retained_index<'a>(
        &'a self, prefix: &'a Arc<State>,
    ) -> std::result::Result<
        crate::snapshot_worktree::ReceiptIndex<'a>, crate::snapshot_worktree::RegistrationError,
    > {
        if !Weak::ptr_eq(&self.prefix, &Arc::downgrade(prefix)) {
            return Err(crate::snapshot_worktree::RegistrationError::OriginalReceipt(
                Error::OriginalReceipt(OriginalReceiptError::Prefix),
            ));
        }
        crate::snapshot_worktree::ReceiptIndex::capture_with_originals(prefix, &self.cursor)
    }
}
