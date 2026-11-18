//! A copy can retain a merge fact, but cannot reissue another FILE's command.
//! Prove target correspondences using the actual transfer receipt, original
//! command, and the immutable copied commit. Never open the source FILE graph.
use super::FileLifecycleRead;
use crate::business::history_copy::{identity, retained::{self, CopiedLifecycle}};
use crate::engine::history_window::HistoryCancellationCheckpoint;
use crate::persist::file_adoption;
use crate::state_projection::lifecycles::LifecycleView;
use crate::{Commit, FileBranchMerge, OperationId, OperationResult, Result};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::file_lifecycle::{LifecycleError, MAX_AUDIT_BYTES, MAX_AUDIT_EVENTS};

fn incomplete() -> crate::Error { LifecycleError::SourceIncomplete.into_error().into() }
fn limit() -> crate::Error { LifecycleError::Bytes.into_error().into() }

#[derive(Default)]
pub(super) struct Budget { members: usize, bytes: usize }

impl Budget {
    pub(super) fn members(&mut self, amount: usize) -> Result<()> {
        self.members = self.members.checked_add(amount).filter(|n| *n <= MAX_AUDIT_EVENTS).ok_or_else(limit)?;
        Ok(())
    }
    pub(super) fn encoded(&mut self, value: &impl data_serialization::Serialize) -> Result<()> {
        let amount=retained::bounded_bytes(value)?;
        self.bytes=self.bytes.checked_add(amount).filter(|n| *n <= MAX_AUDIT_BYTES).ok_or_else(limit)?;
        Ok(())
    }
}

pub(super) struct Transfer {
    operation: OperationId, source: String, source_history: String, pub(super) cutoff: u64,
}
impl Transfer {
    pub(super) fn capture(view: LifecycleView<'_>, history: &str, budget: &mut Budget) -> Result<Option<Self>> {
        let Some(copies)=view.copies() else {return Err(incomplete())};
        if copies.is_empty() {return Ok(None)}
        if copies.len()!=1 || view.malformed() {return Err(incomplete())}
        let op=*copies.keys().next().ok_or_else(incomplete)?;
        let result=view.state().operations.get(&op).filter(|r| r.operation_id==op).ok_or_else(incomplete)?;
        budget.encoded(&result.result)?;
        let file=view.file().as_str();
        let from=|c:&CopiedLifecycle| -> Result<Self> {
            if c.file_id!=file || c.history_tree_id!=history || c.source_sequence>=result.sequence
                || identity(op,&c.source_file_id)?!=c.file_id
            {return Err(incomplete())}
            Ok(Self {operation:op,source:c.source_file_id.clone(),source_history:c.source_history_tree_id.clone(),cutoff:c.source_sequence})
        };
        if result.kind==storage_contract::file_relocation::OBJECT_MOVE_KIND {
            Ok(Some(from(view.object_move(op)?.retained(file)?)?))
        } else if crate::business::history_copy::is_kind(&result.kind) || result.kind==crate::business::FILE_HISTORY_MOVE_KIND {
            Ok(Some(from(&retained::read(result,file)?)?))
        } else {
            Err(incomplete())
        }
    }

    /// Only direct, fully verified correspondences give jump permission. The
    /// caller can still show a strictly local retained decision with open links.
    pub(super) fn exact(
        &self, read: &FileLifecycleRead, local: &FileBranchMerge, original: Option<&OperationResult>, budget: &mut Budget,
    ) -> Result<bool> {
        let Some(r)=original else { return Ok(false) };
        if !file_adoption::is_kind(&r.kind) || r.result["file_id"].as_str()!=Some(self.source.as_str())
            || r.result["history_tree_id"].as_str()!=Some(self.source_history.as_str()) {
            return Ok(false);
        }
        budget.encoded(&r.result)?;
        let field=|k:&str|r.result.get(k).and_then(json_codec::Value::as_str).ok_or_else(incomplete);
        let chosen=field("chosen_node_id")?;
        let survivor=field("surviving_branch_id")?;
        let mapped=|id:&str|identity(self.operation,id);
        let actor=read.source.operation_principals.get(&r.operation_id).ok_or_else(incomplete)?;
        if actor!=local.principal.as_str() || r.operation_id!=local.operation_id || r.sequence!=local.sequence
            || r.sequence>self.cutoff || r.sequence==0 || r.commit_state!=crate::CommitState::Published
            || r.history_tree_id.as_deref()!=Some(self.source_history.as_str()) || r.branch_id.as_deref()!=Some(survivor)
            || field("branch_id")?!=survivor || field("node_id")?!=chosen || r.result["reused_node"].as_bool()!=Some(true)
            || mapped(field("merge_id")?)?!=local.merge_id || mapped(survivor)?!=local.surviving_branch_id
            || mapped(chosen)?!=local.chosen_node_id || mapped(field("version_id")?)?!=file_adoption::real_body(&read.source,local,&local.chosen_node_id)?.version_id
        {return Err(incomplete())}
        let sources=file_adoption::command_sources(r,actor,&self.source,chosen,survivor)?;
        if sources.len()!=local.original_inputs.len() || sources.len()<2 {return Err(incomplete())}
        let retired=r.result["merged_branch_ids"].as_array().ok_or_else(incomplete)?;
        if retired.len()!=sources.len()-1 {return Err(incomplete())}
        let mut mapped_retired=BTreeSet::new();
        for value in retired {
            read.read.cancellation.checkpoint()?;
            budget.members(1)?;
            let id=value.as_str().ok_or_else(incomplete)?;
            if id==survivor || !mapped_retired.insert(mapped(id)?) {return Err(incomplete())}
        }
        if mapped_retired!=local.input_branch_ids.iter().filter(|id|*id!=&local.surviving_branch_id).cloned().collect() {
            return Err(incomplete());
        }
        for (n,input) in local.original_inputs.iter().enumerate() {
            read.read.cancellation.checkpoint()?;
            budget.members(1)?;
            let (branch,node)=sources.at(n);
            if mapped(branch)?!=input.branch_id || mapped(node)?!=input.source_node_id
                || mapped(sources.expected(n))?!=input.expected_node_id { return Err(incomplete()) }
        }
        Ok(true)
    }
}

/// The copied target's immutable, independently admitted branch commit is a
/// witness to the event even when no authorized route back to its older FILE
/// command survives. Such an event never gets a jump from a row label alone.
pub(super) struct Witnesses<'a>(BTreeMap<(OperationId,&'a str), &'a Commit>);
impl<'a> Witnesses<'a> {
    pub(super) fn capture(read:&'a FileLifecycleRead, budget:&mut Budget) -> Result<Self> {
        let mut writes=BTreeMap::new();
        for c in read.source.commits.values() {
            read.read.cancellation.checkpoint()?;
            budget.members(1)?;
            if c.history_tree_id!=read.read.context.history_tree_id.as_str() || c.kind!="file_metadata" {continue}
            if let Some(node)=c.provenance.as_deref()
                && writes.insert((c.operation_id,node),c).is_some() {return Err(incomplete())}
        }
        Ok(Self(writes))
    }
    pub(super) fn require(&self, read:&FileLifecycleRead, local:&FileBranchMerge, budget:&mut Budget) -> Result<()> {
        let c=*self.0.get(&(local.operation_id,local.chosen_node_id.as_str())).ok_or_else(incomplete)?;
        let [prior]=c.parent_commit_ids.as_slice() else {return Err(incomplete())};
        let previous=read.source.commits.get(prior).ok_or_else(incomplete)?;
        let body=file_adoption::real_body(&read.source,local,&local.chosen_node_id)?;
        if !local.is_existing_adoption() || local.original_inputs.len()<2
            || local.input_branch_ids.len()!=local.original_inputs.len()
            || !local.input_branch_ids.contains(&local.surviving_branch_id)
            || local.input_branch_ids.iter().collect::<BTreeSet<_>>().len()!=local.input_branch_ids.len()
            || local.sequence==0 || previous.history_tree_id!=local.history_tree_id || previous.commit_id==c.commit_id || previous.sequence>c.sequence
            || c.author!=local.principal.as_str() || c.sequence!=local.sequence {return Err(incomplete())}
        budget.members(c.entries.len())?;
        for e in &c.entries {
            read.read.cancellation.checkpoint()?;
            if e.kind==crate::NamespaceEntryKind::File && e.file_id==local.file_id && e.version_id==body.version_id {
                return Ok(());
            }
        }
        Err(incomplete())
    }
}
