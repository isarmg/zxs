//! Check the complete native batch result, not a count or a generic operation ID.
use super::*;
use data_serialization::Deserialize;
use storage_contract::client_protocol::MAX_FILE_DELETION_FRONTIER_ENTRIES;

impl Request {
    pub(in crate::files::deletion) fn receipt_matches(
        &self,
        id: &str,
        principal: &str,
        receipt: &Value,
    ) -> bool {
        if receipt["operation_kind"] != "file.delete.batch" {
            return false;
        }
        let Some(sequence) =
            wire::publication_sequence(receipt, &["input", "sources", "deletion_nodes"])
        else {
            return false;
        };
        let result = &receipt["result"];
        if result["input"] != self.input() {
            return false;
        }
        let Some(sources) = result["sources"].as_array() else {
            return false;
        };
        let Some(nodes) = result["deletion_nodes"].as_array() else {
            return false;
        };
        if !self.sources_match(sources) {
            return false;
        }
        let expected = self.targets();
        if nodes.len() != expected.len() {
            return false;
        }
        let mut ids = BTreeSet::new();
        let mut history = None;
        let mut branches = 0;
        let mut parents = 0;
        for ((file, path), value) in expected.into_iter().zip(nodes) {
            if !value.is_object() {
                return false;
            }
            let node = match storage_contract::FileDeletionNode::deserialize(value) {
                Ok(node) => node,
                Err(_) => return false,
            };
            if wire::validate_node(&node).is_err()
                || node.repository_id != self.repository.as_str()
                || node.file_id != file
                || node.path != *path
                || node.operation_id.to_string() != id
                || node.author != principal
                || node.sequence != sequence
                || !ids.insert(node.node_id.clone())
                || history
                    .as_ref()
                    .is_some_and(|history| history != &node.history_tree_id)
            {
                return false;
            }
            // Each node has already passed the same 4096-entry boundary. At
            // most 100 nodes are considered; the sums cannot overflow usize.
            branches += node.branch_heads.len();
            parents += node.parent_node_ids.len();
            if branches > MAX_FILE_DELETION_FRONTIER_ENTRIES
                || parents > MAX_FILE_DELETION_FRONTIER_ENTRIES
            {
                return false;
            }
            history = Some(node.history_tree_id);
        }
        true
    }

    fn sources_match(&self, sources: &[Value]) -> bool {
        if sources.len() != self.entries.len() {
            return false;
        }
        let mut selected_tree = None;
        for (source, selected) in sources.iter().zip(&self.entries) {
            let Some(fields) = source.as_object().filter(|fields| fields.len() == 2) else {
                return false;
            };
            let Some(tree) = fields.get("tree_id").and_then(Value::as_str) else {
                return false;
            };
            if storage_contract::validate_id(tree, "dtree").is_err()
                || fields.get("file_id").and_then(Value::as_str) != Some(selected.file.as_str())
                || selected_tree.is_some_and(|original| original != tree)
            {
                return false;
            }
            // Current may reference an immutable selected tree distinct from
            // the named branch's CAS head. Core authenticates that relation;
            // the browser never fabricates or relabels a source tree.
            selected_tree = Some(tree);
        }
        true
    }
}
