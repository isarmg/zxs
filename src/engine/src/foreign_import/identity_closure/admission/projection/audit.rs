//! Rebase audit references without creating live receipts or imported accounts.
use super::*;

pub(super) struct Audit<'a> {
    recipe: &'a ForeignAdmissionRecipe,
    principals: &'a BTreeMap<String, String>,
    operations: BTreeMap<OperationId, OperationId>,
    occupied: BTreeSet<OperationId>,
    fence: u64,
    scope: String,
}
impl<'a> Audit<'a> {
    pub(super) fn new(
        recipe: &'a ForeignAdmissionRecipe,
        base: &State,
        principals: &'a BTreeMap<String, String>,
        fence: u64,
    ) -> Result<Self> {
        let mut occupied: BTreeSet<_> = base.operations.keys().copied().collect();
        occupied.extend(recipe.source.0.operations.keys().copied());
        for state in [base, &recipe.source.0] {
            occupied.extend(state.commits.values().map(|row| row.operation_id));
            occupied.extend(state.file_path_nodes.values().map(|row| row.operation_id));
            occupied.extend(
                state
                    .file_metadata_nodes
                    .values()
                    .map(|row| row.operation_id),
            );
            occupied.extend(state.file_deletions.values().map(|row| row.operation_id));
            occupied.extend(
                state
                    .file_branch_merges
                    .values()
                    .map(|row| row.operation_id),
            );
            occupied.extend(state.merge_links.values().map(|row| row.operation_id));
        }
        occupied.insert(recipe.identities.baseline.operation_id);
        Ok(Self {
            recipe,
            principals,
            operations: BTreeMap::new(),
            occupied,
            fence,
            scope: recipe.digest()?,
        })
    }
    fn operation(&mut self, id: &mut OperationId) -> Result<()> {
        if let Some(mapped) = self.operations.get(id) {
            *id = *mapped;
            return Ok(());
        }
        let hash = object_hash("foreign-joint-record-operation", &(&self.scope, *id))?;
        let mapped = format!("op_{}", &hash[..32]).parse::<OperationId>()?;
        if !self.occupied.insert(mapped) {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "foreign audit operation collides with a retained or current operation",
            ));
        }
        self.operations.insert(*id, mapped);
        *id = mapped;
        Ok(())
    }
    fn principal(&self, actor: &mut String) -> Result<()> {
        *actor = self.principals.get(actor).cloned().ok_or_else(|| {
            invalid("foreign record author is absent from the bound naming closure")
        })?;
        Ok(())
    }
    fn sequence(&self, sequence: &mut u64) -> Result<()> {
        if *sequence > self.recipe.source.0.sequence {
            return Err(invalid(
                "foreign graph sequence exceeds its original published State",
            ));
        }
        *sequence = self.fence.checked_add(*sequence).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "foreign record sequence exhausted",
            )
        })?;
        Ok(())
    }
    pub(super) fn rewrite(&mut self, mapped: &mut HistoryManifest) -> Result<()> {
        let mut commits = mapped.commits.clone();
        for id in mapped.commits.keys() {
            let value = commits
                .get_mut(id)
                .expect("invariant: same cloned commit map");
            self.operation(&mut value.operation_id)?;
            self.principal(&mut value.author)?;
            self.sequence(&mut value.sequence)?;
        }
        mapped.commits = commits;
        let mut trees = mapped.directory_trees.clone();
        for id in mapped.directory_trees.keys() {
            self.sequence(
                &mut trees
                    .get_mut(id)
                    .expect("invariant: same cloned directory tree map")
                    .sequence,
            )?;
        }
        mapped.directory_trees = trees;
        for branch in mapped.directory_branches.values_mut() {
            self.sequence(&mut branch.sequence)?;
        }
        for repository in mapped.directory_git_repositories.values_mut() {
            self.sequence(&mut repository.initialized_sequence)?;
        }
        for version in mapped.versions.values_mut() {
            self.sequence(&mut version.version_sequence)?;
        }
        for node in mapped.file_path_nodes.values_mut() {
            self.operation(&mut node.operation_id)?;
            self.principal(&mut node.author)?;
            self.sequence(&mut node.sequence)?;
        }
        for node in mapped.file_metadata_nodes.values_mut() {
            self.operation(&mut node.operation_id)?;
            self.principal(&mut node.author)?;
            self.sequence(&mut node.sequence)?;
        }
        for merge in mapped.file_branch_merges.values_mut() {
            self.operation(&mut merge.operation_id)?;
            self.sequence(&mut merge.sequence)?;
        }
        for link in mapped.merge_links.values_mut() {
            self.operation(&mut link.operation_id)?;
            self.principal(&mut link.author)?;
        }
        Ok(())
    }
}
