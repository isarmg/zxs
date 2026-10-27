//! Typed dirty keys feed the shared canonical radix. Every map has a fixed root.
use super::*;
use crate::metadata_map::{Encoder, MapRoot, ProofMeasure, RootFormat};
use std::{borrow::Cow, collections::BTreeSet};
use storage_contract::{MapChanges, SequenceChanges};

macro_rules! state_map_fields {
    ($consumer:ident) => {
        $consumer! {
            repositories,histories,branches,commits,versions,placements,merge_links,
            history_boundaries,content_states,operations,plans,placement_rules,
            protection_policies,operation_principals,file_selections,directory_branches,
            directory_trees,file_write_origins,file_contexts,file_path_nodes,file_metadata_nodes,
            file_branch_merges,file_branch_aliases,file_node_parents,file_path_notes,
            directory_tree_selections,file_locations,file_backup_paths,file_backup_policies,
            directory_selections,file_notes,version_notes,file_names,
        }
    };
}
/// Heterogeneous State key domains share the same radix implementation. The
/// aggregate retains their types instead of parsing serialized leaf keys on reuse.
#[derive(Clone)]
pub(super) enum FieldRoot {
    Text(MapRoot),
    Operations(MapRoot<OperationId>),
    Events(MapRoot<usize>),
}
macro_rules! field_root {
    ($value:expr, $root:ident => $expression:expr) => {
        match $value {
            FieldRoot::Text($root) => $expression,
            FieldRoot::Operations($root) => $expression,
            FieldRoot::Events($root) => $expression,
        }
    };
}
impl FieldRoot {
    pub(super) fn reference(&self) -> &crate::metadata_map::Reference {
        field_root!(self, root => root.reference())
    }
    pub(super) fn maximum_object_bytes(&self) -> usize {
        field_root!(self, root => root.maximum_object_bytes())
    }
    pub(super) fn maximum_tombstone_tail(&self) -> Option<usize> {
        field_root!(self, root => root.maximum_tombstone_tail())
    }
    pub(super) fn visit(
        &self,
        visit: &mut impl FnMut(&crate::metadata_map::Reference, usize) -> Result<()>,
    ) -> Result<()> {
        field_root!(self, root => root.visit(visit))
    }
    fn text(&self) -> &MapRoot {
        let Self::Text(root) = self else {
            unreachable!("invariant: field declaration preserves its String key domain")
        };
        root
    }
    fn operations(&self) -> &MapRoot<OperationId> {
        let Self::Operations(root) = self else {
            unreachable!("invariant: operation fields preserve typed identifiers")
        };
        root
    }
    fn events(&self) -> &MapRoot<usize> {
        let Self::Events(root) = self else {
            unreachable!("invariant: event field preserves typed ordinals")
        };
        root
    }
}
struct Builder {
    encoder: Encoder,
    fields: BTreeMap<String, FieldRoot>,
}
impl Builder {
    fn map<'a, K: Ord + Clone + 'a, V: 'a>(
        &mut self,
        field: &str,
        entries: impl Iterator<Item = (&'a K, &'a V)>,
        previous: Option<(MapChanges<K>, &MapRoot<K>)>,
        get: impl Fn(&K) -> Option<&'a V>,
        encode: impl Fn(&V) -> Result<json_codec::Value>,
        key_text: &impl Fn(&K) -> Cow<'_, str>,
    ) -> Result<MapRoot<K>> {
        let measure = if field == "content_states" {
            ProofMeasure::ContentStates
        } else {
            ProofMeasure::Plain
        };
        match previous {
            Some((MapChanges::Keys(keys), basis)) => {
                self.encoder
                    .update_typed(basis, (*keys).clone(), measure, key_text, |key| {
                        get(key).map(&encode).transpose()
                    })
            }
            Some((MapChanges::All, _)) | None => {
                let entries = entries
                    .map(|(key, value)| Ok((key.clone(), encode(value)?)))
                    .collect::<Result<Vec<_>>>()?;
                self.encoder.map_typed(entries, measure, key_text)
            }
        }
    }
}
pub(super) fn prepare(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    prepare_inner(state, maximum, None)
}
pub(super) fn prepare_incremental(
    before: &State,
    state: &State,
    basis: &SnapshotReuse,
    maximum: usize,
) -> Result<PreparedSnapshot> {
    if basis.descriptor.sequence != before.sequence {
        return prepare(state, maximum);
    }
    prepare_inner(state, maximum, Some((before, basis)))
}
fn prepare_inner(
    state: &State,
    maximum: usize,
    previous: Option<(&State, &SnapshotReuse)>,
) -> Result<PreparedSnapshot> {
    let mut builder = Builder {
        encoder: Encoder::new(maximum),
        fields: BTreeMap::new(),
    };
    macro_rules! typed_field {
        ($field:ident, $variant:ident, $access:ident, $text:expr) => {{
            let root = builder.map(
                stringify!($field),
                state.$field.iter(),
                previous.map(|(before, basis)| {
                    (
                        state.$field.changes_since(&before.$field),
                        basis.fields[stringify!($field)].$access(),
                    )
                }),
                |key| state.$field.get(key),
                |value| encode_value(stringify!($field), value),
                $text,
            )?;
            builder
                .fields
                .insert(stringify!($field).into(), FieldRoot::$variant(root));
        }};
    }
    macro_rules! field {
        (operations) => {
            typed_field!(operations, Operations, operations, &|key: &OperationId| {
                Cow::Owned(key.to_string())
            })
        };
        (operation_principals) => {
            typed_field!(
                operation_principals,
                Operations,
                operations,
                &|key: &OperationId| Cow::Owned(key.to_string())
            )
        };
        ($field:ident) => {
            typed_field!($field, Text, text, &|key: &String| Cow::Borrowed(
                key.as_str()
            ))
        };
    }
    macro_rules! fields {
        ($($field:ident,)*) => {{
            let State { sequence:_, business_generation:_, topology_generation:_, control_auth:_, events:_, directory_selection_generations:_, file_selection_generations:_, $($field:_,)* } = state;
            $(field!($field);)*
        }};
    }
    state_map_fields!(fields);
    macro_rules! counters { ($($field:ident),*) => { $(
        let root = builder.map(stringify!($field), state.$field.iter(),
            previous.map(|(before,basis)| (state.$field.changes_since(&before.$field), basis.fields[stringify!($field)].text())),
            |key| state.$field.get(key), |value| encode_value(stringify!($field), &value.to_string()),
            &|key: &String| Cow::Borrowed(key.as_str()))?;
        builder.fields.insert(stringify!($field).into(), FieldRoot::Text(root));
    )* }; }
    counters!(directory_selection_generations, file_selection_generations);
    fn ordinal(index: &usize) -> Cow<'_, str> {
        Cow::Owned(format!("{index:020}"))
    }
    let events =
        match previous.map(|(before, basis)| (state.events.changes_since(&before.events), basis)) {
            Some((SequenceChanges::Append { start }, basis)) => builder.encoder.update_typed(
                basis.fields["events"].events(),
                (start..state.events.len()).collect::<BTreeSet<_>>(),
                ProofMeasure::Plain,
                &ordinal,
                |index| {
                    state
                        .events
                        .get(*index)
                        .map(|event| encode_value("events", event))
                        .transpose()
                },
            )?,
            Some((SequenceChanges::All, _)) | None => builder.encoder.map_typed(
                state
                    .events
                    .iter()
                    .enumerate()
                    .map(|(index, event)| Ok((index, encode_value("events", event)?)))
                    .collect::<Result<Vec<_>>>()?,
                ProofMeasure::Plain,
                &ordinal,
            )?,
        };
    let maps = builder
        .fields
        .iter()
        .map(|(field, root)| (field.clone(), root.reference().clone()))
        .collect::<BTreeMap<_, _>>();
    let events_reference = events.reference().clone();
    builder
        .fields
        .insert("events".into(), FieldRoot::Events(events));
    let (control_auth, auth_bytes) = match previous {
        Some((before, basis)) if state.control_auth == before.control_auth => {
            (basis.descriptor.control_auth.clone(), basis.auth_bytes)
        }
        _ => {
            let object = PreparedMetadataObject::new(
                "state-control-auth",
                &state.control_auth,
                maximum.min(MAX_METADATA),
            )?;
            let reference = ControlAuthReference {
                kind: ControlAuthKind::StateControlAuth,
                hash: object.hash.clone(),
            };
            let bytes = object.bytes.len();
            builder.encoder.prepared(object);
            (reference, bytes)
        }
    };
    let descriptor = Snapshot {
        format: RootFormat::RadixMapRoots,
        sequence: state.sequence,
        business_generation: state.business_generation,
        topology_generation: state.topology_generation,
        maps,
        events: events_reference,
        control_auth,
    };
    finish(builder, descriptor, auth_bytes, maximum)
}
fn finish(
    mut builder: Builder,
    descriptor: Snapshot,
    auth_bytes: usize,
    maximum: usize,
) -> Result<PreparedSnapshot> {
    let largest = builder
        .fields
        .values()
        .map(FieldRoot::maximum_object_bytes)
        .max()
        .unwrap_or(0)
        .max(auth_bytes);
    if largest > maximum {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "reused metadata exceeds the reader budget",
        ));
    }
    let hash = builder.encoder.object("snapshot", &descriptor)?;
    Ok(PreparedSnapshot {
        metadata_integrity: None,
        descriptor,
        hash,
        objects: builder.encoder.finish(),
        fields: builder.fields,
        auth_bytes,
        maximum,
    })
}
fn encode_value<T: Serialize>(field: &str, value: &T) -> Result<json_codec::Value> {
    #[cfg(test)]
    SNAPSHOT_FIELD_SERIALIZATIONS.with(|counts| {
        *counts.borrow_mut().entry(field.into()).or_default() += 1;
    });
    #[cfg(not(test))]
    let _ = field;
    Ok(json_codec::to_value(value)?)
}
/// Rare source-loss fallback: reconstruct one exact bound field, never substitute
/// another graph or catch an installation/collision failure as a source failure.
pub(super) fn reconstruct_field(
    state: &State,
    field: &str,
    maximum: usize,
) -> Result<(MapRoot, Vec<PreparedMetadataObject>)> {
    let mut encoder = Encoder::new(maximum);
    macro_rules! dispatch {($($name:ident,)*)=>{match field {
        $(stringify!($name)=>state.$name.iter().map(|(k,v)|Ok((k.to_string(),encode_value(field,v)?))).collect::<Result<json_codec::Map<_,_>>>()?,)*
        "directory_selection_generations"=>state.directory_selection_generations.iter().map(|(k,v)|Ok((k.clone(),encode_value(field,&v.to_string())?))).collect::<Result<_>>()?,
        "file_selection_generations"=>state.file_selection_generations.iter().map(|(k,v)|Ok((k.clone(),encode_value(field,&v.to_string())?))).collect::<Result<_>>()?,
        "events"=>state.events.iter().enumerate().map(|(i,event)|Ok((format!("{i:020}"),encode_value(field,event)?))).collect::<Result<_>>()?,
        _=>unreachable!("invariant: snapshot fields use the exhaustive State schema"),
    }};}
    let map = state_map_fields!(dispatch);
    let measure = if field == "content_states" {
        ProofMeasure::ContentStates
    } else {
        ProofMeasure::Plain
    };
    let root = encoder.map_root(map, measure)?;
    Ok((root, encoder.finish()))
}
pub(crate) fn validate_schema(snapshot: &Snapshot) -> Result<()> {
    macro_rules! check {($($field:ident,)*)=>{{
        let fields=[$(stringify!($field),)*"directory_selection_generations","file_selection_generations"];
        if snapshot.maps.len()!=fields.len() || fields.iter().any(|field|!snapshot.maps.contains_key(*field)) {
            return Err(Error::new(ErrorCode::ObjectCorrupt,"snapshot map schema differs"));
        }
    }};}
    state_map_fields!(check);
    for reference in snapshot
        .maps
        .values()
        .chain(std::iter::once(&snapshot.events))
    {
        reference.validate()?;
    }
    snapshot.control_auth.validate()
}
/// Independent logical serializer oracle, not the typed dirty-field adapter.
#[cfg(test)]
pub(super) fn prepare_reference(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    let json_codec::Value::Object(mut logical) = json_codec::to_value(state)? else {
        panic!("State is a map")
    };
    let mut builder = Builder {
        encoder: Encoder::new(maximum),
        fields: BTreeMap::new(),
    };
    macro_rules! fields {($($field:ident,)*) => {for field in [$(stringify!($field),)* "directory_selection_generations", "file_selection_generations"] {
        let value = logical.remove(field).unwrap_or_else(|| json_codec::json!({}));
        let root = if matches!(field, "operations" | "operation_principals") {
            let map: BTreeMap<OperationId,json_codec::Value> = json_codec::from_value(value)?;
            FieldRoot::Operations(builder.encoder.map_typed(map, ProofMeasure::Plain, &|key: &OperationId| Cow::Owned(key.to_string()))?)
        } else {
            let json_codec::Value::Object(map) = value else { panic!("State field is a map") };
            let measure = if field == "content_states" { ProofMeasure::ContentStates } else { ProofMeasure::Plain };
            FieldRoot::Text(builder.encoder.map_root(map, measure)?)
        };
        builder.fields.insert(field.into(), root);
    }};}
    state_map_fields!(fields);
    let events = logical
        .remove("events")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.clone()))
        .collect::<Vec<_>>();
    let events = builder
        .encoder
        .map_typed(events, ProofMeasure::Plain, &|index: &usize| {
            Cow::Owned(format!("{index:020}"))
        })?;
    let events_reference = events.reference().clone();
    let maps = builder
        .fields
        .iter()
        .map(|(f, r)| (f.clone(), r.reference().clone()))
        .collect();
    builder
        .fields
        .insert("events".into(), FieldRoot::Events(events));
    let auth = logical.remove("control_auth").unwrap();
    let auth_bytes = canonical_json_value(&auth)?.len();
    let hash = builder.encoder.object("state-control-auth", &auth)?;
    for field in ["sequence", "business_generation", "topology_generation"] {
        logical.remove(field);
    }
    assert!(logical.is_empty());
    finish(
        builder,
        Snapshot {
            format: RootFormat::RadixMapRoots,
            sequence: state.sequence,
            business_generation: state.business_generation,
            topology_generation: state.topology_generation,
            maps,
            events: events_reference,
            control_auth: ControlAuthReference {
                kind: ControlAuthKind::StateControlAuth,
                hash,
            },
        },
        auth_bytes,
        maximum,
    )
}
#[cfg(test)]
thread_local! {
    pub(super) static SNAPSHOT_FIELD_SERIALIZATIONS:std::cell::RefCell<BTreeMap<String,usize>>=const{std::cell::RefCell::new(BTreeMap::new())};
}
#[cfg(test)]
pub(super) fn take_encoding_counts() -> BTreeMap<String, usize> {
    SNAPSHOT_FIELD_SERIALIZATIONS.with(|counts| std::mem::take(&mut *counts.borrow_mut()))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/event_append_tests.rs"]
mod event_append_tests;
