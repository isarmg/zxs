//! Candidate-bound paged history roots. The partitioning constants belong
//! to this disk format, not to a caller's metadata admission budget.
use super::HistoryManifest;
use crate::{
    Error, HistoryTree, Repository, Result,
    journal::{AuthenticatedMetadataObject, PreparedMetadataObject},
    metadata_map::{
        Encoder, HistoryBinding, MapRoot, MapUpdate, ProofMeasure, Reference, ReferenceKind,
        Resolver, decode_index,
    },
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::state_projection::{DerivedOwners, Field, HistoryEdits};

#[derive(Clone, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PagedHistoryRoot {
    pub(crate) format: crate::metadata_map::RootFormat,
    pub(crate) repository: Repository,
    pub(crate) history: HistoryTree,
    pub(crate) maps: BTreeMap<String, Reference>,
}

/// Actual encoder output for one exact history root, not semantic admission or
/// disk health. Only full encoding or updates with an existing encoder origin
/// construct this type. In particular, resolving raw JSON cannot mint one.
pub(crate) struct EncodedHistory {
    hash: String,
    root: PagedHistoryRoot,
    maps: BTreeMap<Field, MapRoot>,
}

pub(super) struct PreparedHistory {
    pub(super) binding: HistoryBinding,
    pub(super) objects: Vec<PreparedMetadataObject>,
}

fn finish_encoded(
    mut encoder: Encoder,
    mut root: PagedHistoryRoot,
    maps: BTreeMap<Field, MapRoot>,
    maximum: usize,
) -> Result<PreparedHistory> {
    root.maps = maps
        .iter()
        .map(|(field, map)| (field.name().to_owned(), map.reference().clone()))
        .collect();
    let hash = encoder.object("history-manifest", &root)?;
    let objects = encoder.finish();
    let root_bytes = objects
        .last()
        .expect("invariant: encoded history ends with its root")
        .bytes()
        .len();
    let maximum_object_bytes = maps.values().fold(root_bytes, |maximum, map| {
        maximum.max(map.maximum_object_bytes())
    });
    if maximum_object_bytes > maximum {
        return Err(Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "history graph exceeds metadata reader budget",
        ));
    }
    let encoder = Arc::new(EncodedHistory {
        hash: hash.clone(),
        root,
        maps,
    });
    Ok(PreparedHistory {
        binding: HistoryBinding {
            hash,
            maximum_object_bytes,
            encoder: Some(encoder),
        },
        objects,
    })
}

pub(super) type GraphObject = AuthenticatedMetadataObject;
pub(super) type ResolvedGraph = (HistoryManifest, Vec<GraphObject>);

fn invalid(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::HistoryPackageInvalid, message)
}

/// Freeze the complete graph in dependency order. Admission limits may reject
/// a graph but never change its partitioning or its candidate-bound root hash.
pub(super) fn prepare_encoded(
    manifest: &HistoryManifest,
    maximum: usize,
) -> Result<PreparedHistory> {
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
    for &field in Field::ALL {
        let value = logical
            .remove(field.name())
            .unwrap_or_else(|| Value::Object(Map::new()));
        let Value::Object(map) = value else {
            return Err(invalid("history manifest map is not an object"));
        };
        maps.insert(field, encoder.map_root(map, ProofMeasure::Plain)?);
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
        maps: BTreeMap::new(),
    };
    finish_encoded(encoder, root, maps, maximum)
}

#[cfg(test)]
pub(super) fn prepare(
    manifest: &HistoryManifest,
    maximum: usize,
) -> Result<(String, Vec<PreparedMetadataObject>)> {
    let prepared = prepare_encoded(manifest, maximum)?;
    Ok((prepared.binding.hash, prepared.objects))
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
pub(super) fn prepare_indexed_encoded(
    state: &crate::State,
    id: &str,
    owners: &DerivedOwners,
    maximum: usize,
) -> Result<PreparedHistory> {
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
        maps.insert(field, encoder.map_root(values, ProofMeasure::Plain)?);
    }
    let root = PagedHistoryRoot {
        format: crate::metadata_map::RootFormat::RadixMapRoots,
        repository: repository.clone(),
        history: history.clone(),
        maps: BTreeMap::new(),
    };
    finish_encoded(encoder, root, maps, maximum)
}

#[cfg(test)]
pub(super) fn prepare_indexed(
    state: &crate::State,
    id: &str,
    owners: &DerivedOwners,
    maximum: usize,
) -> Result<(String, Vec<PreparedMetadataObject>)> {
    let prepared = prepare_indexed_encoded(state, id, owners, maximum)?;
    Ok((prepared.binding.hash, prepared.objects))
}

/// Only the caller holding the original PublishedDependencyReuse may omit I/O.
/// The descriptor itself is encoding provenance, never omission authority.
pub(super) fn update_encoded(
    basis: &EncodedHistory,
    edits: HistoryEdits,
    state: &crate::State,
    maximum: usize,
) -> Result<PreparedHistory> {
    let mut root = basis.root.clone();
    update_header(&mut root, &edits, state);
    let mut maps = basis.maps.clone();
    let mut encoder = Encoder::new(maximum);
    for (field, replacements) in edits.maps {
        let map = encoder.update_map(
            &maps[&field],
            replacements.keys().cloned().collect(),
            ProofMeasure::Plain,
            |key| match replacements.get(key) {
                // A removed owner row may still exist in global State.
                Some(value) => Ok(value.clone()),
                None => field.value(state, key),
            },
        )?;
        maps.insert(field, map);
    }
    finish_encoded(encoder, root, maps, maximum)
}

fn update_header(root: &mut PagedHistoryRoot, edits: &HistoryEdits, state: &crate::State) {
    if edits.history {
        root.history = state
            .histories
            .get(&root.history.history_tree_id)
            .expect("invariant: updating an existing history")
            .clone();
    }
    if edits.repository {
        root.repository = state
            .repositories
            .get(&root.history.repository_id)
            .expect("invariant: a history references its admitted repository")
            .clone();
    }
}

/// Encode only exact field edits, including membership changes and deletions.
pub(super) struct PreparedUpdate {
    pub(super) hash: String,
    pub(super) created: Vec<PreparedMetadataObject>,
    pub(super) retained: Vec<PreparedMetadataObject>,
    pub(super) maximum_object_bytes: usize,
    pub(super) encoder: Option<Arc<EncodedHistory>>,
}

#[cfg(test)]
pub(super) fn prepare_update(
    objects: Vec<GraphObject>,
    edits: HistoryEdits,
    state: &crate::State,
    maximum: usize,
) -> Result<PreparedUpdate> {
    prepare_update_with_origin(objects, None, edits, state, maximum)
}

pub(super) fn prepare_update_with_origin(
    objects: Vec<GraphObject>,
    origin: Option<&EncodedHistory>,
    edits: HistoryEdits,
    state: &crate::State,
    maximum: usize,
) -> Result<PreparedUpdate> {
    let root = objects
        .last()
        .expect("invariant: a resolved history graph ends with its root");
    let mut decoded = decode_history_root(root)?;
    // Raw graph admission alone never promotes a descriptor. An existing origin
    // binds the exact authenticated preimage; replacement rows are new encoder
    // input, and every unchanged field retains its actual prior descriptor.
    let mut maps = origin.map(|origin| {
        assert_eq!(
            origin.hash,
            root.hash(),
            "invariant: encoder origin binds the exact source root"
        );
        origin.maps.clone()
    });
    update_header(&mut decoded, &edits, state);
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
        let MapUpdate { root, reused_roots } =
            encoder.edit_map_values(&decoded.maps[field.name()], replacements, load)?;
        decoded
            .maps
            .insert(field.name().into(), root.reference().clone());
        if let Some(maps) = &mut maps {
            maps.insert(field, root);
        }
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
    let encoder = maps.map(|maps| {
        Arc::new(EncodedHistory {
            hash: hash.clone(),
            root: decoded,
            maps,
        })
    });
    Ok(PreparedUpdate {
        hash,
        created,
        retained,
        maximum_object_bytes,
        encoder,
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
