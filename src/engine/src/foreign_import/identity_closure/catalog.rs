//! Exhaustive State-field classification. A newly added State field forces the
//! identity owner to decide whether it defines objects or retains provenance.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum Domain {
    Repository,
    History,
    Branch,
    DirectoryBranch,
    DirectoryTree,
    CommitTree,
    Entry,
    File,
    Commit,
    Version,
    PathNode,
    MetadataNode,
    DeletionNode,
    FileMerge,
    MergeLink,
    Boundary,
    PlacementRule,
    ProtectionPolicy,
}
impl Domain {
    pub(super) fn prefix(self) -> &'static str {
        match self {
            Self::Repository => "repo",
            Self::History => "history",
            Self::Branch => "br",
            Self::DirectoryBranch => "dbr",
            Self::DirectoryTree => "dtree",
            Self::CommitTree => "tree",
            Self::Entry => "entry",
            Self::File => "file",
            Self::Commit => "commit",
            Self::Version => "ver",
            Self::PathNode => "fnode",
            Self::MetadataNode => "fmeta",
            Self::DeletionNode => "fdel",
            Self::FileMerge => "fmerge",
            Self::MergeLink => "link",
            Self::Boundary => "boundary",
            Self::PlacementRule => "rule",
            Self::ProtectionPolicy => "policy",
        }
    }
}
pub(super) struct Catalog {
    pub(super) objects: BTreeMap<Domain, BTreeSet<String>>,
    pub(super) principals: BTreeSet<String>,
    pub(super) count: usize,
    retained_bytes: usize,
    implicit_owners: BTreeMap<(Domain, String), String>,
}
impl Catalog {
    fn insert(&mut self, domain: Domain, id: &str) -> Result<()> {
        storage_contract::validate_id(id, domain.prefix())?;
        let values = self.objects.entry(domain).or_default();
        if values.contains(id) {
            return Ok(());
        }
        if self.count == MAX_IDENTITIES {
            return Err(budget());
        }
        self.retained_bytes += id.len() * 6 + 192;
        if self.retained_bytes > MAX_MAPPING_BYTES - 65536 {
            return Err(budget());
        }
        values.insert(id.to_owned());
        self.count += 1;
        Ok(())
    }
    fn owned(&mut self, domain: Domain, id: &str, history: &str) -> Result<()> {
        self.insert(domain, id)?;
        match self.implicit_owners.entry((domain, id.to_owned())) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(history.to_owned());
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() != history => {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "foreign implicit object identity belongs to more than one original history",
                ));
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
        Ok(())
    }
    fn actor(&mut self, actor: &str) -> Result<()> {
        Engine::require_actor(actor)?;
        if !self.principals.contains(actor) {
            if self.principals.len() == MAX_IDENTITIES {
                return Err(budget());
            }
            let next = actor
                .len()
                .checked_mul(6)
                .and_then(|bytes| bytes.checked_add(192))
                .and_then(|bytes| self.retained_bytes.checked_add(bytes))
                .ok_or_else(budget)?;
            if next > MAX_MAPPING_BYTES - 65536 {
                return Err(budget());
            }
            self.retained_bytes = next;
            self.principals.insert(actor.to_owned());
        }
        Ok(())
    }
    fn keys<'a>(&mut self, domain: Domain, values: impl Iterator<Item = &'a String>) -> Result<()> {
        for id in values {
            self.insert(domain, id)?;
        }
        Ok(())
    }
    fn entries(&mut self, history: &str, entries: &[storage_contract::Entry]) -> Result<()> {
        for entry in entries {
            self.owned(Domain::Entry, &entry.entry_id, history)?;
            if entry.kind == storage_contract::NamespaceEntryKind::File {
                self.owned(Domain::File, &entry.file_id, history)?;
            }
        }
        Ok(())
    }
    pub(super) fn all_objects(&self) -> BTreeSet<String> {
        self.objects
            .values()
            .flat_map(|values| values.iter().cloned())
            .collect()
    }
    pub(super) fn capture(state: &State) -> Result<Self> {
        let State {
            sequence: _,
            business_generation: _,
            topology_generation: _,
            repositories,
            histories,
            branches,
            directory_branches,
            directory_trees,
            file_path_nodes,
            file_metadata_nodes,
            file_deletions,
            file_branch_merges,
            commits,
            versions,
            merge_links,
            history_boundaries,
            placement_rules,
            protection_policies,
            file_selections,
            file_write_origins,
            file_branch_aliases,
            // These are projections/references to the typed objects collected
            // below, not independent IDs. Native State admission proves closure.
            directory_git_repositories: _,
            file_contexts: _,
            file_node_parents: _,
            file_path_notes: _,
            directory_tree_selections: _,
            file_locations: _,
            file_backup_paths: _,
            file_backup_policies: _,
            directory_selections: _,
            directory_selection_generations: _,
            file_selection_generations: _,
            file_notes: _,
            version_notes: _,
            file_names: _,
            placements: _,
            content_states: _,
            // Their opaque result/input/auth bytes stay in the original native
            // archive. They must never be recursively string-rewritten.
            operations,
            operation_principals,
            plans,
            control_auth: _,
            events: _,
        } = state;
        if operations.len() > MAX_IDENTITIES || plans.len() > MAX_IDENTITIES {
            return Err(budget());
        }
        let mut result = Self {
            objects: BTreeMap::new(),
            principals: BTreeSet::new(),
            count: 0,
            retained_bytes: 0,
            implicit_owners: BTreeMap::new(),
        };
        macro_rules! collect {
            ($($domain:ident => $values:ident),* $(,)?) => {$(
                result.keys(Domain::$domain, $values.keys())?;
            )*};
        }
        collect! {
            Repository => repositories, History => histories, Branch => branches,
            DirectoryBranch => directory_branches, DirectoryTree => directory_trees,
            PathNode => file_path_nodes, MetadataNode => file_metadata_nodes,
            FileMerge => file_branch_merges, Commit => commits, Version => versions,
            MergeLink => merge_links, Boundary => history_boundaries,
            PlacementRule => placement_rules, ProtectionPolicy => protection_policies,
            File => file_selections, File => file_write_origins, Branch => file_branch_aliases,
        }
        for commit in commits.values() {
            result.owned(Domain::CommitTree, &commit.tree_id, &commit.history_tree_id)?;
            result.keys(Domain::File, commit.pending_file_versions.keys())?;
            result.entries(&commit.history_tree_id, &commit.entries)?;
            result.actor(&commit.author)?;
        }
        for branch in branches.values() {
            if let Some(file) = &branch.file_id {
                result.insert(Domain::File, file)?;
            }
            result.keys(Domain::File, branch.pending_file_versions.keys())?;
        }
        for branch in directory_branches.values() {
            let history = &repositories[&branch.repository_id].history_tree_id;
            if !repositories.contains_key(&branch.directory_id) {
                result.owned(Domain::Entry, &branch.directory_id, history)?;
            }
            result.entries(history, &branch.entries)?;
            result.keys(Domain::File, branch.inherited_files.keys())?;
        }
        for tree in directory_trees.values() {
            let branch = &directory_branches[&tree.directory_branch_id];
            let history = &repositories[&branch.repository_id].history_tree_id;
            result.entries(history, &tree.entries)?;
            result.keys(Domain::File, tree.inherited_files.keys())?;
        }
        for version in versions.values() {
            result.owned(Domain::File, &version.file_id, &version.history_tree_id)?;
        }
        for node in file_deletions.values() {
            result.insert(Domain::DeletionNode, &node.node_id)?;
            result.actor(&node.author)?;
        }
        for node in file_path_nodes.values() {
            result.actor(&node.author)?;
        }
        for node in file_metadata_nodes.values() {
            result.actor(&node.author)?;
        }
        for link in merge_links.values() {
            result.actor(&link.author)?;
        }
        for principal in operation_principals.values() {
            result.actor(principal)?;
        }
        for plan in plans.values() {
            result.actor(&plan.principal)?;
        }
        // Receipt/plan keys are fixed-format identifiers in an admitted
        // source. Account for their new audit names before allocating maps.
        let audit_bytes = operations
            .len()
            .checked_add(plans.len())
            .and_then(|count| count.checked_mul(256))
            .and_then(|bytes| result.retained_bytes.checked_add(bytes))
            .ok_or_else(budget)?;
        if audit_bytes > MAX_MAPPING_BYTES - 65536 {
            return Err(budget());
        }
        Ok(result)
    }
}
