//! One combined receipt advances a directory head; no child completion or
//! local material can stand in for this original admitted record.
use super::{Allocations, NodePlan, PublishedNode, Receipt, ReceiptIndex};
use crate::persist::directory_draft_commit::{DirectoryResult, native::Envelope};
use crate::persist::snapshot_worktree::NodeAllocation;
use crate::snapshot_worktree::drafts::DirectoryDraftIntent;
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotNodeBinding};
use crate::OperationResult;
use storage_contract::snapshot_worktree::SnapshotHandle;

pub(super) struct WorkingRecord<'a> {
    receipt: Receipt<'a>,
    visible: &'a OperationResult,
    envelope: Envelope,
    node: Option<WorkingNode>,
}

struct WorkingNode {
    plan: NodePlan,
    manifest: AdmittedSnapshotManifest,
    binding: SnapshotNodeBinding,
}

impl<'a> ReceiptIndex<'a> {
    pub(in crate::snapshot_worktree) fn public_working_receipt(
        &self,
        lookup: storage_contract::snapshot_worktree::draft_commit::DirectoryDraftLookup,
        principal: &storage_contract::Principal,
    ) -> Result<storage_contract::snapshot_worktree::draft_commit::DirectoryDraftReceipt> {
        use storage_contract::snapshot_worktree::draft_commit::{
            DirectoryDraftOutcome, DirectoryDraftReceipt,
        };
        let record = self.working.get(&lookup.operation_id)
            .ok_or(RegistrationError::Operation(lookup.operation_id))?;
        if record.receipt.principal != *principal
            || record.envelope.directory.request().source.handle != lookup.source
        {
            return Err(RegistrationError::Digest(lookup.operation_id));
        }
        let result = match &record.envelope.directory {
            DirectoryResult::Changed(value) => DirectoryDraftOutcome::Changed {
                snapshot: SnapshotHandle {
                    publication: record.receipt.original.operation_id,
                    repository_id: lookup.source.repository_id,
                    branch_id: lookup.source.branch_id,
                    node_id: value.construction.node,
                },
            },
            DirectoryResult::Unchanged(_) => DirectoryDraftOutcome::Unchanged {
                snapshot: lookup.source,
            },
        };
        // Only the original proves the selected relation. Status/protection
        // come from the installed visible outcome, never the original promise.
        Ok(DirectoryDraftReceipt {
            operation_id: record.visible.operation_id,
            publication_sequence: storage_contract::Sequence::new(record.visible.sequence),
            commit_state: record.visible.commit_state,
            protection_at_publish: record.visible.protection_at_publish,
            result,
        })
    }

    /// Existing wins before a fresh picker/CAS. Derived source bindings are
    /// taken from this exact indexed original, never recaptured from new HEAD.
    pub(in crate::snapshot_worktree) fn retry_working(
        &self, intent: &DirectoryDraftIntent<'_>,
    ) -> Result<Option<&'a OperationResult>> {
        if !self.source.operations.contains_key(&intent.operation) { return Ok(None); }
        let record = self.working.get(&intent.operation)
            .ok_or(RegistrationError::Operation(intent.operation))?;
        let request = record.envelope.directory.request();
        if record.receipt.principal != *intent.principal || request.source.handle != intent.source
            || request.message != intent.message || request.selected.len() != intent.selected.len()
            || request.selected.iter().zip(intent.selected).any(|(recorded, actual)| {
                recorded.target != actual.target || recorded.expectation() != actual.expected
            })
        {
            return Err(RegistrationError::Digest(intent.operation));
        }
        // Original bytes prove the request and node relation. A recovered
        // outcome remains the actual answer, including its degraded status.
        Ok(Some(record.visible))
    }

    pub(super) fn working_node(&self, handle: SnapshotHandle) -> Result<PublishedNode<'_>> {
        let record = self.working.get(&handle.publication).ok_or(RegistrationError::Source)?;
        let node = record.node.as_ref().ok_or(RegistrationError::Source)?;
        let published = PublishedNode {
            plan: &node.plan, manifest: &node.manifest, binding: node.binding,
        };
        if published.handle() != handle { return Err(RegistrationError::Source); }
        Ok(published)
    }

    pub(super) fn publish_working(
        &mut self, receipt: Receipt<'a>, visible: &'a OperationResult,
        envelope: Envelope, allocations: &mut Allocations,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let request = envelope.directory.request();
        if request.operation != operation || request.principal != receipt.principal
            || self.completions.contains_key(&operation) || self.planned_files.contains(&operation)
            || self.working.contains_key(&operation)
        {
            return Err(RegistrationError::Operation(operation));
        }
        let source = self.draft_head(request.source.handle)?;
        if source.binding() != request.source.binding || source.host() != &request.source.host {
            return Err(RegistrationError::Source);
        }
        let manifest = crate::snapshot_worktree::drafts::reconstruct_indexed_manifest(
            source.manifest(), &envelope.directory, crate::working_tree::snapshot::MAX_ENTRIES,
        ).map_err(|source| RegistrationError::DirectoryResult(Box::new(source)))?;
        let node = match &envelope.directory {
            DirectoryResult::Unchanged(_) => None,
            DirectoryResult::Changed(value) => {
                let original = source.binding();
                if value.binding.shares_node_entities(original) {
                    return Err(RegistrationError::Identity);
                }
                let plan = NodePlan {
                    request: source.plan.request.clone(), host: source.plan.host.clone(),
                    allocated: NodeAllocation {
                        repository: request.source.handle.repository_id,
                        branch: request.source.handle.branch_id,
                        node: value.construction.node, completion: operation,
                    },
                    branch_name: source.plan.branch_name.clone(), material: manifest.reference(),
                };
                if !allocations.nodes.insert(value.construction.node) {
                    return Err(RegistrationError::Identity);
                }
                Some(WorkingNode { plan, manifest, binding: value.binding })
            }
        };
        if let Some(node) = &node {
            self.branch_heads.insert(
                (request.source.handle.repository_id, request.source.handle.branch_id),
                node.plan.handle(),
            );
        }
        self.working.insert(operation, WorkingRecord { receipt, visible, envelope, node });
        Ok(())
    }
}
