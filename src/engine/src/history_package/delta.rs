//! Precise projection edits admitted from exact transaction-local write sets.
use super::changes::projection_fields;
use crate::{Branch, State};
use std::collections::BTreeMap;
use storage_contract::MapChanges;

pub(super) type BranchEdits = BTreeMap<String, Branch>;

pub(super) enum ProjectionDelta {
    StableBranches(BTreeMap<String, BranchEdits>),
    Full,
}

impl ProjectionDelta {
    pub(super) fn between(before: &State, after: &State) -> Self {
        macro_rules! classify {
            (branch { branches, }
             history { $($history:ident,)* }
             repository { $($repository:ident,)* }
             version { $($version:ident,)* }
             file { $($file:ident,)* }
             directory { $($directory:ident,)* }
             special { $($special:ident,)* }
             independent { $($independent:ident,)* }) => {{
                let State {
                    sequence: _, business_generation: _, topology_generation: _,
                    control_auth: _, events: _,
                    branches: _, $($history: _,)* $($repository: _,)*
                    $($version: _,)* $($file: _,)* $($directory: _,)*
                    $($special: _,)* $($independent: _,)*
                } = after;
                $(if !after.$history.changes_since(&before.$history).is_empty() { return Self::Full; })*
                $(if !after.$repository.changes_since(&before.$repository).is_empty() { return Self::Full; })*
                $(if !after.$version.changes_since(&before.$version).is_empty() { return Self::Full; })*
                $(if !after.$file.changes_since(&before.$file).is_empty() { return Self::Full; })*
                $(if !after.$directory.changes_since(&before.$directory).is_empty() { return Self::Full; })*
                $(if !after.$special.changes_since(&before.$special).is_empty() { return Self::Full; })*
            }};
        }
        projection_fields!(classify);
        let MapChanges::Keys(keys) = after.branches.changes_since(&before.branches) else {
            return Self::Full;
        };
        let mut histories = BTreeMap::<String, BranchEdits>::new();
        for key in keys.iter() {
            let (Some(previous), Some(next)) = (before.branches.get(key), after.branches.get(key))
            else {
                return Self::Full;
            };
            // Record IDs matching their keys remain an admission/private
            // mutation invariant, not a reason to hide a bug behind fallback.
            // Owner changes alter selections and aliases and require the
            // complete owner-closure path.
            if previous.history_tree_id != next.history_tree_id || previous.file_id != next.file_id
            {
                return Self::Full;
            }
            if previous != next {
                histories
                    .entry(next.history_tree_id.clone())
                    .or_default()
                    .insert(key.clone(), next.clone());
            }
        }
        Self::StableBranches(histories)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/history_package/delta/tests.rs"]
mod tests;
