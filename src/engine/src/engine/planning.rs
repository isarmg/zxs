//! Borrowed preparation inputs, without journal or publication authority.
use super::{Engine, MutationPlan};
use crate::{disk::*, *};
use json_codec::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) enum Admission<'a> {
    Live(&'a AtomicBool),
    Detached,
}

pub(crate) struct PlanningContext<'a> {
    pub(crate) root: &'a Path,
    pub(crate) config: &'a ValidatedConfig,
    pub(crate) disk_resources: &'a Arc<DiskResources>,
    pub(crate) state: &'a Arc<State>,
    pub(crate) snapshot: &'a Option<String>,
    pub(crate) next_slot: u64,
    pub(crate) blocked: bool,
    pub(crate) metadata_integrity: &'a Arc<super::provenance::MetadataIntegrity>,
    pub(super) admission: Admission<'a>,
    pub(super) integrity_scrub_schedule:
        Option<&'a std::sync::Weak<super::integrity_scrub::ScheduleState>>,
    pub(super) published_state_base: Option<&'a Arc<super::publication::PublishedStateBase>>,
    pub(crate) segment_caches: &'a Arc<crate::segment_store::SegmentCaches>,
    pub(crate) operation_locator_root: Option<&'a crate::metadata_map::Reference>,
}
impl Engine {
    pub(crate) fn planning_context(&self) -> PlanningContext<'_> {
        PlanningContext {
            root: &self.root,
            config: &self.config,
            disk_resources: &self.disk_resources,
            state: &self.state,
            snapshot: &self.snapshot,
            next_slot: self.next_slot,
            blocked: self.blocked,
            metadata_integrity: &self.metadata_integrity,
            segment_caches: &self.segment_caches,
            operation_locator_root: self.operation_locator_root.as_ref(),
            admission: Admission::Live(&self.commit_preparation),
            published_state_base: self.published_state_base.as_ref(),
            integrity_scrub_schedule: self.integrity_scrub_schedule.as_ref(),
        }
    }
}
impl<'a> PlanningContext<'a> {
    pub(crate) fn members(&self) -> Vec<DiskConfig> {
        let mut members = self
            .config
            .disks
            .iter()
            .filter(|d| d.metadata_member)
            .cloned()
            .collect::<Vec<_>>();
        members.sort_by(|a, b| a.disk_id.cmp(&b.disk_id));
        members
    }
    pub(crate) fn disk(&self, slot: &str) -> Result<&'a DiskConfig> {
        self.config.disk(slot)
    }
    pub(crate) fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        self.config.route(primary, copies)
    }
    pub(crate) fn repository_for_history(&self, history: &str) -> Result<&'a Repository> {
        let h = self.state.histories.get(history).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::HistoryNotFound,
                "history tree does not exist",
            )
        })?;
        self.state
            .repositories
            .get(&h.repository_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "history has no repository",
                )
            })
    }
    pub(crate) fn repository_for_branch(&self, branch: &str) -> Result<&'a Repository> {
        let b = self.state.branches.get(branch).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "branch does not exist",
            )
        })?;
        self.repository_for_history(&b.history_tree_id)
    }
    pub(crate) fn used_bytes(&self, slot: &str) -> u64 {
        self.state
            .placements
            .values()
            .filter(|p| p.route_slot_ids.iter().any(|s| s == slot))
            .filter(|p| {
                self.state
                    .content_states
                    .get(&p.version_id)
                    .is_none_or(|s| *s != ContentState::Purged && *s != ContentState::Migrated)
            })
            .map(|p| {
                self.state
                    .versions
                    .get(&p.version_id)
                    .expect("invariant: every placement references an existing version")
            })
            .fold(0u64, |sum, v| sum.saturating_add(v.logical_size))
    }
    pub(crate) fn require_write(&self) -> Result<()> {
        match self.admission {
            Admission::Live(active) if active.load(Ordering::Acquire) => {
                return Err(Error::new(
                    ErrorCode::CommitPreparationBusy,
                    "workspace payload preparation temporarily fences other writes",
                )
                .retryable());
            }
            Admission::Live(_) | Admission::Detached => {}
        }
        if self.blocked {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "an unresolved decision or unavailable metadata member blocks writes",
            )
            .retryable());
        }
        Ok(())
    }
}

impl<'a> PlanningContext<'a> {
    /// One permanent receipt by id over this context's published State; see
    /// `segment_store::ReceiptLookup` for the exact lookup order.
    pub(crate) fn find_receipt(
        &self,
        id: OperationId,
    ) -> Result<Option<storage_contract::OperationRecordEntry>> {
        let members = self.members();
        crate::segment_store::ReceiptLookup {
            state: self.state,
            reader: crate::segment_store::SegmentReader {
                resources: self.disk_resources,
                members: &members,
                caches: self.segment_caches,
                observe: None,
            },
            locator_root: self.operation_locator_root,
        }
        .find(id)
    }
    /// Run one read over published namespace objects through the bounded
    /// cache, from the planning view.
    pub(crate) fn with_namespace_source<T>(
        &self,
        read: impl FnOnce(&dyn crate::namespace::NamespaceSource) -> Result<T>,
    ) -> Result<T> {
        let members = self.members();
        let reader = crate::segment_store::SegmentReader {
            resources: self.disk_resources,
            members: &members,
            caches: self.segment_caches,
            observe: None,
        };
        let source =
            crate::namespace::LoaderSource(|reference: &storage_contract::NamespaceReference| {
                reader.namespace_node(reference)
            });
        read(&source)
    }
    pub(crate) fn plan_new_mutation<F>(
        &self,
        kind: &str,
        principal: &str,
        operation_id: OperationId,
        digest: String,
        change: F,
    ) -> Result<MutationPlan>
    where
        F: FnOnce(
            &Self,
            &mut State,
            &mut crate::engine::MutationScratch,
        ) -> Result<json_codec::Value>,
    {
        // Published and planning views share only the immutable base. Every
        // mutation still owns an independent next State until publication.
        let mut next = self.state.as_ref().clone();
        next.begin_tracking();
        let members = self.members();
        let reader = crate::segment_store::SegmentReader {
            resources: self.disk_resources,
            members: &members,
            caches: self.segment_caches,
            observe: None,
        };
        let published =
            crate::namespace::LoaderSource(|reference: &storage_contract::NamespaceReference| {
                reader.namespace_node(reference)
            });
        if !self
            .published_state_base
            .is_some_and(|base| base.names_captured_for(self.state))
        {
            crate::container_storage::capture_names(&mut next, &published)?;
        }
        next.sequence = self.next_slot;
        if !kind.starts_with("auth.") {
            next.business_generation =
                next.business_generation.checked_add(1).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "business generation exhausted",
                    )
                })?;
        }
        // Existing receipts return before this planner. Reject a bad clock
        // before invoking the new mutation's preparation work.
        let timestamp = crate::business::now()?;
        let mut scratch = crate::engine::MutationScratch::default();
        let mut result = change(self, &mut next, &mut scratch)?;
        // Fresh namespace objects of this transaction resolve before the
        // published store, so changed heads stream without self-publication.
        let draft = crate::namespace::DraftSource {
            fresh: &scratch.namespace,
            base: &published,
        };
        crate::container_storage::capture_changed_names(self.state, &mut next, &draft)?;
        let prepared = scratch.payloads;
        result["timestamp"] = json_codec::json!(timestamp.to_string());
        if let Some(branch_id) = result
            .get("branch_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            && let Some(branch) = next.branches.get(&branch_id)
        {
            result["branch_generation"] = json!(branch.generation.to_string());
        }
        result["membership_epoch"] = json!("1");
        let mut protection = Vec::new();
        for p in &prepared {
            let mut slots = Vec::new();
            for slot in &p.placement.route_slot_ids {
                let disk = self.disk(slot)?;
                slots.push(json!({"slot_id":slot,"copy_level":disk.level,"disk_id":disk.disk_id,"binding_generation":disk.binding_generation.to_string(),"state":"COMPLETE_VERIFIED"}));
            }
            protection.push(json!({"version_id":p.version.version_id,"primary_slot_id":p.placement.primary_slot_id,"target_copies":p.placement.target_copies,"required_slots":p.placement.route_slot_ids,"topology_generation":p.placement.topology_generation.to_string(),"slots":slots}));
        }
        result["version_protection"] = json!(protection);
        let protection = if prepared.is_empty() {
            ProtectionAtPublish::NotApplicable
        } else {
            ProtectionAtPublish::TargetMet
        };
        let op = OperationResult {
            operation_id,
            kind: kind.into(),
            commit_id: result
                .get("commit_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            history_tree_id: result
                .get("history_tree_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            branch_id: result
                .get("branch_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: protection,
            current_health: storage_contract::CurrentHealth::Healthy,
            request_digest: digest.clone(),
            sequence: next.sequence,
            result,
        };
        next.operations
            .push(storage_contract::OperationRecordEntry {
                principal: principal.into(),
                result: op.clone(),
            })?;
        next.events.push(Event {
            sequence: next.sequence,
            operation_id,
            kind: kind.into(),
            object_id: op
                .commit_id
                .clone()
                .or_else(|| op.history_tree_id.clone())
                .unwrap_or_else(|| operation_id.to_string()),
        })?;
        Ok(MutationPlan::Prepared {
            next: Box::new(next),
            prepared,
            namespace: scratch.namespace.take(),
            result: op,
        })
    }
}
