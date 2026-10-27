//! Candidate-bound paged history roots. The partitioning constants belong
//! to this disk format, not to a caller's metadata admission budget.
use super::HistoryManifest;
use crate::{
    Error, HistoryTree, Repository, Result,
    journal::{AuthenticatedMetadataObject, PreparedMetadataObject},
    metadata_map::{Encoder, Reference, Resolver},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Map, Value};
use std::collections::BTreeMap;

// This is a schema, not a list inferred from fields present in untrusted JSON.
// Even optional, omitted logical maps have explicit roots in the paged graph.
const MAP_FIELDS: &[&str] = &[
    "branches",
    "file_selections",
    "directory_branches",
    "directory_trees",
    "file_write_origins",
    "file_contexts",
    "file_path_nodes",
    "file_metadata_nodes",
    "file_branch_merges",
    "file_branch_aliases",
    "file_node_parents",
    "file_path_notes",
    "directory_tree_selections",
    "file_locations",
    "file_backup_paths",
    "file_backup_policies",
    "directory_selections",
    "directory_selection_generations",
    "file_selection_generations",
    "file_notes",
    "version_notes",
    "file_names",
    "commits",
    "versions",
    "placements",
    "merge_links",
    "boundary_marks",
    "content_states",
    "placement_rules",
    "protection_policies",
];

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct PagedHistoryRoot {
    repository: Repository,
    history: HistoryTree,
    maps: BTreeMap<String, Reference>,
}

pub(super) type GraphObject = AuthenticatedMetadataObject;
pub(super) type ResolvedGraph = (HistoryManifest, Vec<GraphObject>);

fn invalid(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::HistoryPackageInvalid, message)
}

/// Freeze the complete graph in dependency order. Admission limits may reject
/// a graph but never change its partitioning or its candidate-bound root hash.
pub(super) fn prepare(
    manifest: &HistoryManifest,
    maximum: usize,
) -> Result<(String, Vec<PreparedMetadataObject>)> {
    // Going through the logical serializer preserves all field-specific wire
    // rules, especially omitted optional maps and decimal-string u64 values.
    let Value::Object(mut logical) = json_codec::to_value(manifest)? else {
        return Err(invalid("history manifest is not an object"));
    };
    for field in ["repository", "history"] {
        logical.remove(field);
    }
    let mut encoder = Encoder::new(maximum);
    let mut maps = BTreeMap::new();
    for &field in MAP_FIELDS {
        let value = logical
            .remove(field)
            .unwrap_or_else(|| Value::Object(Map::new()));
        let Value::Object(map) = value else {
            return Err(invalid("history manifest map is not an object"));
        };
        maps.insert(field.to_owned(), encoder.map(map)?);
    }
    if !logical.is_empty() {
        return Err(invalid(
            "history graph schema omits a logical manifest field",
        ));
    }
    let root = PagedHistoryRoot {
        repository: manifest.repository.clone(),
        history: manifest.history.clone(),
        maps,
    };
    let hash = encoder.object("history-manifest", &root)?;
    Ok((hash, encoder.finish()))
}

/// Sources authenticate each address once before the resolver checks graph shape
/// and route-dependent constraints. Returned objects retain their canonical bytes.
pub(super) fn resolve_with_objects(
    root: AuthenticatedMetadataObject,
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<ResolvedGraph> {
    let decoded: PagedHistoryRoot = json_codec::from_value(root.value().clone())?;
    if decoded.maps.len() != MAP_FIELDS.len()
        || MAP_FIELDS
            .iter()
            .any(|field| !decoded.maps.contains_key(*field))
    {
        return Err(invalid("history root has an invalid logical schema"));
    }
    for reference in decoded.maps.values() {
        reference.validate()?;
    }
    let mut logical = Map::new();
    logical.insert(
        "repository".to_owned(),
        json_codec::to_value(decoded.repository)?,
    );
    logical.insert("history".to_owned(), json_codec::to_value(decoded.history)?);
    let mut resolver = Resolver::new(load);
    for (field, reference) in decoded.maps {
        let entries = resolver.map(&reference)?;
        logical.insert(field, Value::Object(entries));
    }
    let manifest = json_codec::from_value(Value::Object(logical))?;
    let mut objects = resolver.finish();
    objects.push(root);
    Ok((manifest, objects))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/history_package/graph/tests.rs"]
mod tests;
