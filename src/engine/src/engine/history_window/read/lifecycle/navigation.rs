//! Exact original relationships within the already captured FILE read scope.
//! Never substitute the current HEAD, text, a digest or a deletion/merge ID.
use super::{Event, EventBudget, FileLifecycleRead, copy_merges::{Budget,Transfer,Witnesses}};
use crate::{Result, engine::history_window::{nodes,HistoryCancellationCheckpoint}};
use storage_contract::file_lifecycle::*;
use storage_contract::FileNodeId;
use std::collections::BTreeSet;

impl FileLifecycleRead {
    fn scope(&self) -> nodes::Scope<'_> {
        nodes::Scope {file:self.request.file_id.as_str(),history:self.read.context.history_tree_id.as_str()}
    }
    pub(super) fn body_at(&self, id:&str) -> Result<Option<FileNodeId>> {
        self.read.cancellation.checkpoint()?;
        let found = nodes::node(&self.source,self.scope(),id);
        match found {
            Some(nodes::Node::Content(v)) => Ok(Some(FileNodeId::Version(v.version_id.parse()?))),
            Some(nodes::Node::Path(p)) => Ok(Some(FileNodeId::Anchor(p.node_id.parse()?))),
            Some(nodes::Node::Metadata(_)) => Err(LifecycleError::SourceIncomplete.into_error().into()),
            None => Ok(None),
        }
    }
    fn allowed_branch(&self, id:&str) -> bool {
        self.source.branches.get(id).is_some_and(|b|
            b.file_id.as_deref()==Some(self.request.file_id.as_str()) && b.history_tree_id==self.scope().history
            && b.state!=crate::BranchState::Migrated)
    }
    pub(super) fn frozen_heads(&self, heads:&std::collections::BTreeMap<String,String>) -> Result<References> {
        if heads.len()>MAX_LINKS {return Ok(References::Open{})}
        let mut links=Vec::with_capacity(heads.len());
        for (branch,head) in heads {
            self.read.cancellation.checkpoint()?;
            if !self.allowed_branch(branch) {return Ok(References::Open{})}
            links.push(NodeLink{role:LinkRole::BranchHead{branch:branch.parse()?}, node:self.body_at(head)?});
        }
        Ok(References::Complete{links})
    }
    pub(super) fn merge_events(
        &self, view:crate::state_projection::lifecycles::LifecycleView<'_>,
        events:&mut Vec<Event>, budget:&mut EventBudget,
    ) -> Result<()> {
        let mut work=Budget::default();
        let mut transfer=None;
        let mut witnesses=None;
        for merge in self.source.file_branch_merges.values() {
            work.members(1)?;
            self.read.cancellation.checkpoint()?;
            if merge.file_id!=self.scope().file || merge.history_tree_id!=self.scope().history {continue}
            if !self.allowed_branch(&merge.surviving_branch_id) {continue}
            work.members(merge.original_inputs.len())?;
            let original=self.source.operations.get(&merge.operation_id);
            let native=original.filter(|r| crate::persist::file_adoption::is_kind(&r.kind)
                && r.result["merge_id"].as_str()==Some(merge.merge_id.as_str()));
            let exact=if let Some(receipt)=native {
                work.encoded(&receipt.result)?;
                // The native verifier checks actual request, actor and all
                // original source/expected HEADs against *this* FILE graph.
                crate::persist::file_adoption::original(&self.source,receipt)?;
                true
            } else {
                if transfer.is_none() {
                    transfer=Some(Transfer::capture(view,self.scope().history,&mut work)?);
                }
                let Some(copy)=transfer.as_ref().and_then(Option::as_ref) else {continue};
                if merge.sequence>copy.cutoff {continue}
                if witnesses.is_none() {witnesses=Some(Witnesses::capture(self,&mut work)?)}
                witnesses.as_ref().expect("initialized above").require(self,merge,&mut work)?;
                merge.original_inputs.len()<=MAX_LINKS-2 && copy.exact(self,merge,original,&mut work)?
            };
            let references=if !exact || merge.original_inputs.len()>MAX_LINKS-2 {
                References::Open{}
            } else {
                let mut links=Vec::with_capacity(merge.original_inputs.len()+2);
                let mut authorized=true;
                for source in &merge.original_inputs {
                    self.read.cancellation.checkpoint()?;
                    if !self.allowed_branch(&source.branch_id) {authorized=false;break}
                    links.push(NodeLink {role:LinkRole::MergeInput {branch:source.branch_id.parse()?},
                        node:self.body_at(&source.source_node_id)?});
                }
                if !authorized || links.iter().map(|r|(r.role,r.node)).collect::<BTreeSet<_>>().len()!=links.len() {
                    References::Open{}
                } else {
                    links.push(NodeLink{role:LinkRole::Chosen,node:self.body_at(&merge.chosen_node_id)?});
                    links.push(NodeLink{role:LinkRole::Result,node:self.body_at(&merge.result_node_id)?});
                    References::Complete{links}
                }
            };
            let event=Event { operation_id:merge.operation_id, sequence:merge.sequence, principal:merge.principal.clone(),
                references, occurred_at:None, detail:Detail::Merged{merge_id:merge.merge_id.parse()?,survivor:merge.surviving_branch_id.parse()?} };
            budget.admit(&event)?; events.push(event);
        }
        Ok(())
    }

    fn native_boundary(&self, event: &Event) -> bool {
        let Some(receipt)=self.source.operations.get(&event.operation_id) else {return false};
        if receipt.sequence!=event.sequence {return false}
        match &event.detail {
            Detail::Import { .. } => false,
            // The source pool sequence on a copied event is not evidence of
            // a complete interval in this destination, even with exact links.
            Detail::Merged { merge_id,.. } => crate::persist::file_adoption::is_kind(&receipt.kind)
                && receipt.result["merge_id"].as_str()==Some(merge_id.to_string().as_str()),
            _ => true,
        }
    }

    /// A complete original enumeration is shared by all neighboring boundaries
    /// on this page. Without enough work budget, leave *all* sections open.
    pub(super) fn intervals(&self, events:&[Event]) -> Result<Vec<Interval>> {
        let mut sections:Vec<_>=events.windows(2).map(|p| Interval {
            older:p[0].event_key(),newer:p[1].event_key(),body:IntervalBody::Open{},
        }).collect();
        if sections.is_empty() {return Ok(sections)}
        let mut rows:Vec<Option<(Vec<IntervalNode>,usize)>> = (0..sections.len()).map(|_| Some((vec![],0))).collect();
        let mut work=0usize;
        macro_rules! spend { () => {{
            self.read.cancellation.checkpoint()?;
            if work>=MAX_AUDIT_EVENTS {return Ok(sections)}
            work+=1;
        }} }
        // Check the complete retained corpus including ineligible/retired members.
        // An inaccessible member in the interval blocks *completeness*, not only its link.
        for (id,seq,file,hist) in self.source.versions.values().map(|v|
            (&v.version_id,v.version_sequence,&v.file_id,&v.history_tree_id))
            .chain(self.source.file_path_nodes.values().map(|p|(&p.node_id,p.sequence,&p.file_id,&p.history_tree_id))) {
            spend!();
            if file!=self.scope().file || hist!=self.scope().history {continue}
            // A co-publication has no separate time order. Neither adjoining
            // interval is entitled to choose a side for that node.
            let n=events.partition_point(|e| e.sequence<seq);
            if n<events.len() && events[n].sequence==seq {
                let end=events.partition_point(|e|e.sequence<=seq);
                let width=rows.len();
                for item in &mut rows[n.saturating_sub(1)..end.min(width)] { *item=None; }
                continue
            }
            if n==0 || n>=events.len() {continue}
            let k=n-1;
            let Some((nodes_so_far,edges))=rows[k].as_mut() else {continue};
            let parents=nodes::base_parents(&self.source,id);
            if nodes_so_far.len()>=MAX_INTERVAL_NODES || parents.len()>MAX_INTERVAL_EDGES-*edges {
                rows[k]=None; continue
            }
            let Some(node)=self.body_at(id)? else {rows[k]=None;continue};
            let mut actual=Vec::with_capacity(parents.len());
            let mut complete=true;
            for parent in parents {
                spend!();
                let Some(p)=self.body_at(parent)? else {complete=false;break};
                actual.push(p);
            }
            if !complete {rows[k]=None; continue}
            *edges+=actual.len();
            nodes_so_far.push(IntervalNode{node,parents:actual});
        }
        for (k,(section,candidate)) in sections.iter_mut().zip(rows).enumerate() {
            self.read.cancellation.checkpoint()?;
            let Some((mut nodes,_))=candidate else{continue};
            let upper=&events[k+1];
            if section.older.sequence>=section.newer.sequence
                || [ &events[k], upper ].iter().any(|e| !self.native_boundary(e)) {continue}
            // A native deletion freezes every FILE branch HEAD. Other events
            // (including an undo that cites an earlier tombstone) do not.
            let mut branches=Vec::new();
            if !nodes.is_empty() {
                if !matches!(upper.detail,Detail::Delete{..}) {continue}
                let References::Complete{links}=&upper.references else {continue};
                let members:BTreeSet<_>=nodes.iter().map(|n|n.node).collect();
                let mut proved=true;
                for link in links {
                    let (LinkRole::BranchHead{branch},Some(node))=(link.role,link.node) else {proved=false;break};
                    if members.contains(&node) {branches.push(BranchTip{branch,node});}
                }
                if !proved {continue}
            }
            nodes.sort_by_key(|row|row.node);
            section.body=IntervalBody::Complete{nodes,branches};
        }
        Ok(sections)
    }
}
