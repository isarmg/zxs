//! Read-only, captured-scope facts for a bounded batch selection. Mutation
//! admission still revalidates these identities and immutable tree baselines.
use super::*;

pub(in crate::engine_backend) fn read(engine: &Engine, input: &Value) -> Result<Value, ApiError> {
    keys(input, &["repository_id", "path", "sources", "entries"])?;
    let repo = field(input, "repository_id")?;
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("path must be a string"))?;
    let sources = input
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::invalid("sources must be an array"))?;
    let selected = input
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::invalid("entries must be an array"))?;
    if sources.len() > 500
        || selected.len() > 500
        || sources.len().saturating_mul(selected.len()) > 10_000
    {
        return Err(ApiError::new(
            429,
            "RESOURCE_LIMIT_EXCEEDED",
            "scope query exceeds the bounded selection limit",
        ));
    }
    let scopes = scoped_directory_branches(engine, repo, Some(path))?
        .into_iter()
        .map(|branch| (branch.directory_branch_id.as_str(), branch))
        .collect::<BTreeMap<_, _>>();
    let mut selections = BTreeSet::new();
    let mut selected_keys = BTreeSet::new();
    for entry in selected {
        keys(entry, &["entry_id", "path"])?;
        let id = field(entry, "entry_id")?;
        let path = field(entry, "path")?;
        if id.len() > 256 {
            return Err(ApiError::invalid("entry identity is too long"));
        }
        name_policy::validate_lookup_path(path).map_err(ApiError::invalid)?;
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        if name_policy::path_key(parent)
            != name_policy::path_key(input["path"].as_str().unwrap_or_default())
        {
            return Err(ApiError::invalid(
                "selected entries must be direct children of the requested scope",
            ));
        }
        if !selected_keys.insert((id, name_policy::path_key(path))) {
            return Err(ApiError::invalid("duplicate selected entry"));
        }
        selections.insert((id, path));
    }
    let mut seen = BTreeSet::new();
    let mut branches = Vec::with_capacity(sources.len());
    for source in sources {
        keys(source, &["directory_branch_id", "expected_tree_id"])?;
        let id = field(source, "directory_branch_id")?;
        let head = field(source, "expected_tree_id")?;
        if !seen.insert(id) {
            return Err(ApiError::invalid("duplicate source scope"));
        }
        let branch = scopes
            .get(id)
            .filter(|branch| branch.head_tree_id == head)
            .ok_or_else(|| {
                ApiError::conflict(
                    "LISTING_CHANGED",
                    "source scope changed; refresh before preparing a batch",
                )
            })?;
        let entries = selected_entry_facts(&branch.entries, &selections);
        let mut summary = listing::branch_summary(
            branch,
            engine
                .state()
                .directory_selections
                .get(&branch.directory_id)
                == Some(&branch.directory_branch_id),
        );
        summary["entries"] = entries.into();
        branches.push(summary);
    }
    Ok(json!({"repository_id":repo,"directory_branches":branches}))
}

fn selected_entry_facts(
    entries: &[storage_contract::Entry],
    selections: &BTreeSet<(&str, &str)>,
) -> Vec<Value> {
    // Index only selected identities. Preserve single-identity versus
    // exact-path alias semantics without emitting other branch entries.
    let ids = selections
        .iter()
        .map(|(id, _)| *id)
        .collect::<BTreeSet<_>>();
    let mut matches = BTreeMap::<&str, (usize, Option<&storage_contract::Entry>)>::new();
    let selected_keys = selections
        .iter()
        .map(|(id, path)| (*id, name_policy::path_key(path)))
        .collect::<BTreeSet<_>>();
    let mut exact = BTreeMap::new();
    for entry in entries {
        if ids.contains(entry.entry_id.as_str()) {
            let value = matches.entry(&entry.entry_id).or_default();
            value.0 = value.0.saturating_add(1);
            if value.0 == 1 {
                value.1 = Some(entry);
            }
            let key = (entry.entry_id.as_str(), name_policy::path_key(&entry.path));
            if selected_keys.contains(&key) {
                exact.insert(key, entry);
            }
        }
    }
    let mut facts = Vec::with_capacity(selections.len());
    for (id, path) in selections {
        let chosen = match matches.get(id) {
            Some((1, entry)) => *entry,
            _ => exact.get(&(*id, name_policy::path_key(path))).copied(),
        };
        if let Some(entry) = chosen {
            facts.push(json!({"entry_id":entry.entry_id,"path":entry.path,"selected_path":path}));
        }
    }
    facts
}
#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/scope_query_tests.rs"]
mod tests;
