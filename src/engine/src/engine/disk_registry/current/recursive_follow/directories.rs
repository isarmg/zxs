//! An empty snapshot follows only its captured stable identities. A new native
//! receipt supersedes the live policy generation; historical receipts stay intact.
use super::*;

impl CurrentNamespaces {
    pub(super) fn directory_follow_plan(
        &self,
        previous: &State,
        moves: &BTreeMap<String, (String, String)>,
        files: &mut FollowPlan,
    ) -> Result<DirectoryFollowPlan> {
        let mut plan = DirectoryFollowPlan::new();
        let affected = moves
            .keys()
            .filter_map(|entry| self.recursive_policies.directories.get(entry))
            .flat_map(|policies| policies.iter())
            .collect::<BTreeSet<_>>();
        for id in affected {
            let bound = self
                .active_recursive_policy(previous, id, true)?
                .expect("invariant: indexed directory has a recursive policy");
            bound
                .capture
                .verify_follow_routes(self, bound.roots.keys())?;
            let namespace = bound
                .directory_namespace
                .as_ref()
                .expect("invariant: indexed directory has a snapshot")
                .follow(moves, &bound.roots)?;
            let policy = if let Some(change) = files.policies.get(id) {
                change.policy.clone()
            } else {
                let mut policy = bound.policy.clone();
                policy.generation = policy.generation.checked_add(1).ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "directory policy generation exhausted",
                    )
                })?;
                files.policies.insert(
                    id.clone(),
                    PolicyChange {
                        source_operation_id: bound.authority,
                        policy: policy.clone(),
                    },
                );
                policy
            };
            plan.policies.insert(
                id.clone(),
                DirectoryFollowChange {
                    source_operation_id: bound.authority,
                    policy,
                    namespace,
                },
            );
        }
        Ok(plan)
    }
}
