//! Persistent registry and occurrence-mask pages carried by control envelopes.
use crate::{Error, Result, decode_strict};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, btree_map::Entry};
use storage_contract::ErrorCode;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct GraphObject {
    kind: String,
    value: json_codec::Value,
}

pub(in crate::engine) type GraphObjects = BTreeMap<String, GraphObject>;

/// Read the envelope representation. Registry and mask loaders apply the byte
/// budget and authenticate each page against its referenced domain digest.
pub(in crate::engine) fn read_objects(
    objects: &GraphObjects,
    kind: &'static str,
    hash: &str,
) -> Result<Vec<u8>> {
    let object = objects
        .get(hash)
        .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "registry graph object is missing"))?;
    if object.kind != kind {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "registry graph object kind differs",
        ));
    }
    Ok(json_codec::to_vec(&object.value)?)
}

/// Collect the unique object set emitted by one prepared graph.
pub(in crate::engine) fn collect_objects<'a>(
    values: impl Iterator<Item = (&'static str, &'a str, &'a [u8])>,
) -> Result<GraphObjects> {
    values
        .map(|(kind, hash, bytes)| {
            Ok((
                hash.to_owned(),
                GraphObject {
                    kind: kind.to_owned(),
                    value: decode_strict(bytes)?,
                },
            ))
        })
        .collect()
}

/// Merge separately prepared graphs without cloning their decoded value trees.
pub(in crate::engine) fn add_objects<'a>(
    objects: &mut GraphObjects,
    values: impl Iterator<Item = (&'static str, &'a str, &'a [u8])>,
    collision: ErrorCode,
) -> Result<()> {
    for (kind, hash, bytes) in values {
        let object = GraphObject {
            kind: kind.to_owned(),
            value: decode_strict(bytes)?,
        };
        match objects.entry(hash.to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(object);
            }
            Entry::Occupied(entry) if entry.get() != &object => {
                return Err(Error::new(collision, "registry graph hash collision"));
            }
            Entry::Occupied(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/registry_graph_objects_tests.rs"]
mod tests;
