//! Candidate-bound paged history roots. The partitioning constants belong
//! to this disk format, not to a caller's metadata admission budget.
use super::HistoryManifest;
use crate::{
    Error, HistoryTree, Repository, Result,
    journal::{AuthenticatedMetadataObject, PreparedMetadataObject},
    metadata_map::{Encoder, MapUpdate, Reference, ReferenceKind, Resolver, decode_index},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::state_projection::{DerivedOwners, Field, HistoryEdits};

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PagedHistoryRoot {
    pub(crate) format: crate::metadata_map::RootFormat,
    pub(crate) repository: Repository,
    pub(crate) history: HistoryTree,
    pub(crate) maps: BTreeMap<String, Reference>,
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
    for field in Field::ALL.iter().map(|field| field.name()) {
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
        format: crate::metadata_map::RootFormat::RadixMapRoots,
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
    let decoded = decode_history_root(&root)?;
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

/// Warm graphs are already bound to the successfully published State. Read and
/// authenticate the exact complete old closure, without rebuilding its manifest
/// or repeating cold logical admission. Every old dependency is still installed.
pub(super) fn read_objects(
    root: AuthenticatedMetadataObject,
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<Vec<GraphObject>> {
    let decoded = decode_history_root(&root)?;
    let mut resolver = Resolver::new(load);
    for reference in decoded.maps.values() {
        resolver.retain_map(reference)?;
    }
    let mut objects = resolver.finish();
    objects.push(root);
    Ok(objects)
}

/// A newly introduced history uses indexed row membership, not a scan of every
/// State map. Private mutations preserve logical admission; cold imports do not
/// enter here until their external graph and State have passed full validation.
pub(super) fn prepare_indexed(
    state: &crate::State,
    id: &str,
    owners: &DerivedOwners,
    maximum: usize,
) -> Result<(String, Vec<PreparedMetadataObject>)> {
    let history = state
        .histories
        .get(id)
        .expect("invariant: preparing an existing history");
    let repository = state
        .repositories
        .get(&history.repository_id)
        .expect("invariant: a history references its admitted repository");
    let mut encoder = Encoder::new(maximum);
    let mut maps = BTreeMap::new();
    for &field in Field::ALL {
        let values = match owners.rows(id) {
            Some(rows) => rows.values(field, state)?,
            None => Map::new(),
        };
        maps.insert(field.name().to_owned(), encoder.map(values)?);
    }
    let root = PagedHistoryRoot {
        format: crate::metadata_map::RootFormat::RadixMapRoots,
        repository: repository.clone(),
        history: history.clone(),
        maps,
    };
    let hash = encoder.object("history-manifest", &root)?;
    Ok((hash, encoder.finish()))
}

/// Encode only exact field edits, including membership changes and deletions.
pub(super) struct PreparedUpdate {
    pub(super) hash: String,
    pub(super) created: Vec<PreparedMetadataObject>,
    pub(super) retained: Vec<PreparedMetadataObject>,
    pub(super) maximum_object_bytes: usize,
}

pub(super) fn prepare_update(
    objects: Vec<GraphObject>,
    edits: HistoryEdits,
    state: &crate::State,
    maximum: usize,
) -> Result<PreparedUpdate> {
    let root = objects
        .last()
        .expect("invariant: a resolved history graph ends with its root");
    let mut decoded = decode_history_root(root)?;
    let id = &decoded.history.history_tree_id;
    if edits.history {
        decoded.history = state
            .histories
            .get(id)
            .expect("invariant: updating an existing history")
            .clone();
    }
    if edits.repository {
        decoded.repository = state
            .repositories
            .get(&decoded.history.repository_id)
            .expect("invariant: a history references its admitted repository")
            .clone();
    }
    let available = objects
        .iter()
        .map(|object| ((object.kind(), object.hash().to_owned()), object))
        .collect::<BTreeMap<_, _>>();
    let load = |reference: &Reference| {
        Ok(*available
            .get(&(reference.kind().domain(), reference.hash().to_owned()))
            .expect("invariant: a resolved graph retains every bound dependency"))
    };
    let mut encoder = Encoder::new(maximum);
    let mut reachable = BTreeSet::new();
    for &field in Field::ALL {
        let reference = &decoded.maps[field.name()];
        if !edits.maps.contains_key(&field) {
            collect_reused(reference, &load, &mut reachable)?;
        }
    }
    for (field, replacements) in edits.maps {
        let MapUpdate {
            reference,
            reused_roots,
        } = encoder.edit_map_values(&decoded.maps[field.name()], replacements, load)?;
        decoded.maps.insert(field.name().into(), reference);
        for reference in reused_roots {
            collect_reused(&reference, &load, &mut reachable)?;
        }
    }
    let hash = encoder.object("history-manifest", &decoded)?;
    let mut retained = Vec::new();
    for object in objects {
        if reachable.contains(&(object.kind(), object.hash().to_owned())) {
            retained.push(object.into_prepared(maximum)?);
        }
    }
    let created = encoder.finish();
    let maximum_object_bytes = created
        .iter()
        .chain(&retained)
        .map(|object| object.bytes().len())
        .max()
        .expect("invariant: an updated history includes its newly encoded root");
    Ok(PreparedUpdate {
        hash,
        created,
        retained,
        maximum_object_bytes,
    })
}

fn collect_reused<'a>(
    reference: &Reference,
    load: &impl Fn(&Reference) -> Result<&'a AuthenticatedMetadataObject>,
    reachable: &mut BTreeSet<(&'static str, String)>,
) -> Result<()> {
    if !reachable.insert((reference.kind().domain(), reference.hash().to_owned())) {
        return Ok(());
    }
    if matches!(reference.kind(), ReferenceKind::Index) {
        for child in decode_index(load(reference)?)?.children.into_values() {
            collect_reused(&child, load, reachable)?;
        }
    }
    Ok(())
}

/// Shared root-schema boundary for full resolution and exact-closure scrub.
pub(crate) fn decode_history_root(root: &AuthenticatedMetadataObject) -> Result<PagedHistoryRoot> {
    let decoded: PagedHistoryRoot = json_codec::from_value(root.value().clone())?;
    if decoded.maps.len() != Field::ALL.len()
        || Field::ALL
            .iter()
            .any(|field| !decoded.maps.contains_key(field.name()))
    {
        return Err(invalid("history root has an invalid logical schema"));
    }
    for reference in decoded.maps.values() {
        reference.validate()?;
    }
    Ok(decoded)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/history_package/graph/tests.rs"]
mod tests;
