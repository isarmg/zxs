//! Routed native semantics separate actual retained-copy admission from unused
//! logical prefixes. This is a conservative superset of container obligations.
use super::*;

#[derive(Default)]
pub(super) struct FileRoutes {
    bindings: BTreeMap<String, u64>,
    pub(super) backups: BTreeSet<String>,
}

fn changed() -> Error {
    Error::new(
        ErrorCode::ScopeChanged,
        "retained file routes no longer match their captured recursive backup policy",
    )
}

/// One version scan with ordered-map lookups, not a full scan per file. Hidden,
/// unavailable and historical versions remain obligations; no head-only filter.
pub(super) fn collect(
    state: &State,
    files: &BTreeSet<String>,
) -> Result<BTreeMap<String, FileRoutes>> {
    let mut routes: BTreeMap<String, FileRoutes> = BTreeMap::new();
    if files.is_empty() {
        return Ok(routes);
    }
    for version in state.versions.values() {
        if !files.contains(&version.file_id) {
            continue;
        }
        let placement = state
            .placements
            .get(&version.version_id)
            .ok_or_else(invalid)?;
        let route = routes.entry(version.file_id.clone()).or_default();
        for (index, slot) in placement.route_slot_ids.iter().enumerate() {
            let generation = *placement
                .binding_generations
                .get(slot)
                .ok_or_else(invalid)?;
            if route
                .bindings
                .insert(slot.clone(), generation)
                .is_some_and(|old| old != generation)
            {
                return Err(changed());
            }
            if index != 0 {
                route.backups.insert(slot.clone());
            }
        }
    }
    Ok(routes)
}

impl FileRoutes {
    pub(super) fn verify(&self, current: &CurrentNamespaces, bound: &BoundPolicy) -> Result<()> {
        bound
            .capture
            .verify_follow_bindings(current, &self.bindings)?;
        if self
            .backups
            .iter()
            .any(|slot| !bound.roots.contains_key(slot))
        {
            return Err(changed());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/engine/current_recursive_route_tests.rs"]
mod tests;
