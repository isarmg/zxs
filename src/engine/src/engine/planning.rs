//! Borrowed preparation inputs, without journal or publication authority.
use super::{Engine, MutationPlan, Prepared};
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
    pub(crate) state: &'a Arc<State>,
    pub(crate) snapshot: &'a Option<String>,
    pub(crate) next_slot: u64,
    pub(crate) blocked: bool,
    pub(super) admission: Admission<'a>,
    pub(super) published_state_base: Option<&'a Arc<super::publication::PublishedStateBase>>,
}
impl Engine {
    pub(crate) fn planning_context(&self) -> PlanningContext<'_> {
        PlanningContext {
            root: &self.root,
            config: &self.config,
            state: &self.state,
            snapshot: &self.snapshot,
            next_slot: self.next_slot,
            blocked: self.blocked,
            admission: Admission::Live(&self.commit_preparation),
            published_state_base: self.published_state_base.as_ref(),
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
        self.config
            .disks
            .iter()
            .find(|d| d.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::SlotNotFound,
                    "storage slot is not registered",
                )
            })
    }
    pub(crate) fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        let first = self.disk(primary)?;
        if first.level != 1 || copies == 0 {
            return Err(Error::new(
                storage_contract::ErrorCode::PlacementInvalid,
                "placement requires a primary slot and at least one copy",
            ));
        }
        let mut route = vec![first.slot_id.clone()];
        let mut current = first;
        while route.len() < copies as usize {
            let next = self
                .config
                .disks
                .iter()
                .find(|d| {
                    d.level == current.level + 1
                        && d.upstream_slot_id
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(&current.slot_id))
                })
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::RouteIncomplete,
                        "requested copies exceed the configured fixed route",
                    )
                })?;
            route.push(next.slot_id.clone());
            current = next;
        }
        Ok(route)
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
        for disk in self.members() {
            validate_seal(self.config, &disk)?;
        }
        Ok(())
    }
}

impl<'a> PlanningContext<'a> {
    pub(crate) fn plan_new_mutation<F>(
        &self,
        kind: &str,
        principal: &str,
        operation_id: OperationId,
        digest: String,
        change: F,
    ) -> Result<MutationPlan>
    where
        F: FnOnce(&Self, &mut State, &mut Vec<Prepared>) -> Result<json_codec::Value>,
    {
        // Published and planning views share only the immutable base. Every
        // mutation still owns an independent next State until publication.
        let mut next = self.state.as_ref().clone();
        next.begin_tracking();
        crate::container_storage::capture_names(&mut next);
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
        let mut prepared = Vec::new();
        let mut result = change(self, &mut next, &mut prepared)?;
        crate::container_storage::capture_names(&mut next);
        result["timestamp"] = json_codec::json!(crate::business::now().to_string());
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
        next.operations.insert(operation_id, op.clone());
        next.operation_principals
            .insert(operation_id, principal.into());
        next.events.push(Event {
            sequence: next.sequence,
            operation_id,
            kind: kind.into(),
            object_id: op
                .commit_id
                .clone()
                .or_else(|| op.history_tree_id.clone())
                .unwrap_or_else(|| operation_id.to_string()),
        });
        Ok(MutationPlan::Prepared {
            next: Box::new(next),
            prepared,
            result: op,
        })
    }
}
