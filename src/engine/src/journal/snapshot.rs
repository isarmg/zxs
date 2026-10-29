//! Typed dirty keys feed the shared canonical radix. Every map has a fixed root.
use super::*;
use crate::metadata_map::{Encoder, MapRoot, ProofMeasure, RootFormat};
use std::{borrow::Cow, collections::BTreeSet};
use storage_contract::{EVENT_SEGMENT_KIND, MapChanges, OPERATION_SEGMENT_KIND};

macro_rules! state_map_fields {
    ($consumer:ident) => {
        $consumer! {
            repositories,histories,branches,commits,versions,placements,merge_links,
            history_boundaries,content_states,plans,placement_rules,
            protection_policies,file_selections,directory_branches,
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
    Segments(MapRoot<u64>),
}
macro_rules! field_root {
    ($value:expr, $root:ident => $expression:expr) => {
        match $value {
            FieldRoot::Text($root) => $expression,
            FieldRoot::Operations($root) => $expression,
            FieldRoot::Segments($root) => $expression,
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
    fn segments(&self) -> &MapRoot<u64> {
        let Self::Segments(root) = self else {
            unreachable!("invariant: segment directories preserve typed ordinals")
        };
        root
    }
}
/// Directory leaf values are typed wire records carrying each segment's
/// placement facts (first sequence / ordinal / record count). They are
/// recorded values, re-verified against decoded bodies by the disk-read
/// entry; readers never trust them as allocation lengths.
fn segment_ordinal_text(index: &u64) -> Cow<'_, str> {
    Cow::Owned(format!("{index:020}"))
}
fn event_dir_entry(hash: &str, first_sequence: u64) -> json_codec::Value {
    count_segment_directory_records("events");
    json_codec::to_value(storage_contract::EventDirectoryEntry {
        kind: Default::default(),
        hash: hash.into(),
        first_sequence,
    })
    .expect("invariant: directory entries are plain JSON records")
}
fn op_dir_entry(
    hash: &str,
    first_sequence: u64,
    first_ordinal: u64,
    records: u64,
) -> json_codec::Value {
    count_segment_directory_records("operation_log");
    json_codec::to_value(storage_contract::OperationDirectoryEntry {
        kind: Default::default(),
        hash: hash.into(),
        first_sequence,
        first_ordinal,
        records,
    })
    .expect("invariant: directory entries are plain JSON records")
}
type LogDirectoryEntry = (u64, json_codec::Value);
type EncodedLog = (Vec<PreparedMetadataObject>, Vec<LogDirectoryEntry>);
type EncodedLogBodies = (Vec<PreparedMetadataObject>, Option<LogDirectoryEntry>);

/// Objects to install plus the complete directory for one segmented log:
/// every sealed reference, the open tail (when non-empty) as its own object,
/// and the retained fresh-seal body so a seal decided this transaction is
/// actually installed before PREPARE.
fn events_log_encoding(state: &State, maximum: usize) -> Result<EncodedLog> {
    let (objects, tail) = events_log_objects(state, maximum)?;
    let mut entries = state
        .events
        .sealed()
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            (
                index as u64,
                event_dir_entry(&segment.hash, segment.first_sequence),
            )
        })
        .collect::<Vec<_>>();
    entries.extend(tail);
    Ok((objects, entries))
}
/// Prepare only resident bodies. Incremental directory encoding resolves each
/// requested key from the resident directory instead of serializing its prefix.
fn events_log_objects(state: &State, maximum: usize) -> Result<EncodedLogBodies> {
    let mut objects = Vec::new();
    let mut tail = None;
    if let Some((_, segment)) = state.events.fresh_seal() {
        objects.push(PreparedMetadataObject::new(
            EVENT_SEGMENT_KIND,
            segment,
            maximum.min(MAX_METADATA),
        )?);
    }
    if let Some(first) = state.events.tail().first() {
        let segment = storage_contract::EventSegment {
            format: Default::default(),
            events: state.events.tail().to_vec(),
        };
        let object =
            PreparedMetadataObject::new(EVENT_SEGMENT_KIND, &segment, maximum.min(MAX_METADATA))?;
        tail = Some((
            state.events.tail_ordinal(),
            event_dir_entry(&object.hash, first.sequence),
        ));
        objects.push(object);
    }
    Ok((objects, tail))
}
fn operations_log_encoding(state: &State, maximum: usize) -> Result<EncodedLog> {
    let (objects, tail) = operations_log_objects(state, maximum)?;
    let mut entries = state
        .operations
        .sealed()
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            (
                index as u64,
                op_dir_entry(
                    &segment.hash,
                    segment.first_sequence,
                    segment.first_ordinal,
                    segment.records,
                ),
            )
        })
        .collect::<Vec<_>>();
    entries.extend(tail);
    Ok((objects, entries))
}
fn operations_log_objects(state: &State, maximum: usize) -> Result<EncodedLogBodies> {
    let mut objects = Vec::new();
    let mut tail = None;
    if let Some((_, segment)) = state.operations.fresh_seal() {
        objects.push(PreparedMetadataObject::new(
            OPERATION_SEGMENT_KIND,
            segment,
            maximum.min(MAX_METADATA),
        )?);
    }
    if let Some(first) = state.operations.tail().first() {
        let segment = storage_contract::OperationSegment {
            format: Default::default(),
            records: state.operations.tail().to_vec(),
        };
        let object = PreparedMetadataObject::new(
            OPERATION_SEGMENT_KIND,
            &segment,
            maximum.min(MAX_METADATA),
        )?;
        tail = Some((
            state.operations.tail_ordinal(),
            op_dir_entry(
                &object.hash,
                first.result.sequence,
                state.operations.tail_first_ordinal(),
                state.operations.tail().len() as u64,
            ),
        ));
        objects.push(object);
    }
    Ok((objects, tail))
}
/// Complete locator encoding requires the one-shot cold index that only the
/// disk-read entry attaches. Normal publications extend the previous basis
/// instead; reaching this without either is a lineage-handling bug upstream.
fn operation_key_text(key: &OperationId) -> Cow<'_, str> {
    Cow::Owned(key.to_string())
}
fn full_locator_root(encoder: &mut Encoder, state: &State) -> Result<MapRoot<OperationId>> {
    let index = state.operations.cold_index().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "operation locator lineage is unavailable; recover before publishing",
        )
    })?;
    count_field_records("operation_locators", index.len());
    encoder.map_typed(
        index
            .iter()
            .map(|(id, ordinal)| (*id, json_codec::Value::from(ordinal.to_string())))
            .collect::<Vec<_>>(),
        ProofMeasure::Plain,
        &operation_key_text,
    )
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
    prepare_inner(state, maximum, None, &mut no_reused_sources)
}
pub(super) fn prepare_incremental(
    before: &State,
    state: &State,
    basis: &SnapshotReuse,
    maximum: usize,
    reused: &mut dyn FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<PreparedSnapshot> {
    if basis.descriptor.sequence != before.sequence {
        return prepare(state, maximum);
    }
    prepare_inner(state, maximum, Some((before, basis)), reused)
}
/// A full encoding never touches reused leaf sources; reaching for one there
/// is a lineage-handling bug, reported rather than silently re-read.
pub(crate) fn no_reused_sources(
    _kind: &'static str,
    _hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    Err(Error::new(
        storage_contract::ErrorCode::StateInvalid,
        "a full snapshot encoding has no reused leaf sources",
    ))
}
fn prepare_inner(
    state: &State,
    maximum: usize,
    previous: Option<(&State, &SnapshotReuse)>,
    reused: &mut dyn FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
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
        ($field:ident) => {
            typed_field!($field, Text, text, &|key: &String| Cow::Borrowed(
                key.as_str()
            ))
        };
    }
    macro_rules! fields {
        ($($field:ident,)*) => {{
            let State { sequence:_, business_generation:_, topology_generation:_, control_auth:_, events:_, operations:_, directory_selection_generations:_, file_selection_generations:_, $($field:_,)* } = state;
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
    // Segment directories map 20-digit ordinals to typed sealed/tail segment
    // references. Sealed bodies are byte-stable; only dirty ordinals re-encode,
    // and the one open tail object per log is re-prepared from resident records.
    let events = {
        let changes = previous
            .map(|(before, _)| state.events.changes_since(&before.events))
            .unwrap_or(storage_contract::SegmentChanges::All);
        match (changes, previous) {
            (storage_contract::SegmentChanges::Segments(dirty), Some((_, basis))) => {
                let (objects, tail) = events_log_objects(state, maximum)?;
                for object in objects {
                    builder.encoder.prepared(object);
                }
                builder.encoder.update_typed(
                    basis.fields["events"].segments(),
                    (*dirty).clone(),
                    ProofMeasure::Plain,
                    &segment_ordinal_text,
                    |index| {
                        if let Some(segment) = usize::try_from(*index)
                            .ok()
                            .and_then(|index| state.events.sealed().get(index))
                        {
                            return Ok(Some(event_dir_entry(
                                &segment.hash,
                                segment.first_sequence,
                            )));
                        }
                        Ok(tail
                            .as_ref()
                            .filter(|(ordinal, _)| ordinal == index)
                            .map(|(_, value)| value.clone()))
                    },
                )?
            }
            _ => {
                let (objects, entries) = events_log_encoding(state, maximum)?;
                for object in objects {
                    builder.encoder.prepared(object);
                }
                builder
                    .encoder
                    .map_typed(entries, ProofMeasure::Plain, &segment_ordinal_text)?
            }
        }
    };
    // Receipts: the segment directory plus the id→ordinal locator map.
    {
        let changes = previous
            .map(|(before, _)| state.operations.changes_since(&before.operations))
            .unwrap_or(storage_contract::SegmentChanges::All);
        let log_root = match (changes, previous) {
            (storage_contract::SegmentChanges::Segments(dirty), Some((_, basis))) => {
                let (objects, tail) = operations_log_objects(state, maximum)?;
                for object in objects {
                    builder.encoder.prepared(object);
                }
                builder.encoder.update_typed(
                    basis.fields["operation_log"].segments(),
                    (*dirty).clone(),
                    ProofMeasure::Plain,
                    &segment_ordinal_text,
                    |index| {
                        if let Some(segment) = usize::try_from(*index)
                            .ok()
                            .and_then(|index| state.operations.sealed().get(index))
                        {
                            return Ok(Some(op_dir_entry(
                                &segment.hash,
                                segment.first_sequence,
                                segment.first_ordinal,
                                segment.records,
                            )));
                        }
                        Ok(tail
                            .as_ref()
                            .filter(|(ordinal, _)| ordinal == index)
                            .map(|(_, value)| value.clone()))
                    },
                )?
            }
            _ => {
                let (objects, entries) = operations_log_encoding(state, maximum)?;
                for object in objects {
                    builder.encoder.prepared(object);
                }
                count_field_records("operation_log", entries.len());
                builder
                    .encoder
                    .map_typed(entries, ProofMeasure::Plain, &segment_ordinal_text)?
            }
        };
        builder
            .fields
            .insert("operation_log".into(), FieldRoot::Segments(log_root));
        let locators = match previous {
            Some((before, basis)) => {
                match state.operations.appended_ids_since(&before.operations) {
                    Some(appended) => {
                        let appended: BTreeMap<OperationId, u64> = appended.into_iter().collect();
                        let basis_root = basis.fields["operation_locators"].operations();
                        let basis_reference = basis_root.reference().clone();
                        // Re-encoding a dirty leaf also re-serializes its
                        // untouched co-resident rows: their ordinals come
                        // from exact point reads of the published basis
                        // pages, memoized so each page is read once.
                        let mut pages: BTreeMap<String, json_codec::Value> = BTreeMap::new();
                        builder.encoder.update_typed(
                            basis_root,
                            appended.keys().cloned().collect::<BTreeSet<_>>(),
                            ProofMeasure::Plain,
                            &operation_key_text,
                            |key| {
                                if let Some(ordinal) = appended.get(key) {
                                    return Ok(Some(json_codec::Value::from(ordinal.to_string())));
                                }
                                crate::metadata_map::lookup(
                                    &basis_reference,
                                    &key.to_string(),
                                    |kind, hash| {
                                        let address = format!("{kind}/{hash}");
                                        if let Some(value) = pages.get(&address) {
                                            return AuthenticatedMetadataObject::authenticate(
                                                kind,
                                                hash,
                                                value.clone(),
                                            );
                                        }
                                        let object = reused(kind, hash)?;
                                        pages.insert(address, object.value().clone());
                                        Ok(object)
                                    },
                                )
                            },
                        )?
                    }
                    None => full_locator_root(&mut builder.encoder, state)?,
                }
            }
            None => full_locator_root(&mut builder.encoder, state)?,
        };
        builder
            .fields
            .insert("operation_locators".into(), FieldRoot::Operations(locators));
    }
    let maps = builder
        .fields
        .iter()
        .map(|(field, root)| (field.clone(), root.reference().clone()))
        .collect::<BTreeMap<_, _>>();
    let events_reference = events.reference().clone();
    builder
        .fields
        .insert("events".into(), FieldRoot::Segments(events));
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
    count_field_records(field, 1);
    Ok(json_codec::to_value(value)?)
}
/// Test observability only: a complete re-encode of a segment-directory or
/// locator field records its entry count, exactly like per-value map encodes.
fn count_field_records(field: &str, records: usize) {
    #[cfg(test)]
    SNAPSHOT_FIELD_SERIALIZATIONS.with(|counts| {
        *counts.borrow_mut().entry(field.into()).or_default() += records;
    });
    #[cfg(not(test))]
    let _ = (field, records);
}
fn count_segment_directory_records(field: &'static str) {
    #[cfg(test)]
    SEGMENT_DIRECTORY_SERIALIZATIONS.with(|counts| {
        *counts.borrow_mut().entry(field).or_default() += 1;
    });
    #[cfg(not(test))]
    let _ = field;
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
        "events"=>{
            let (objects,entries)=events_log_encoding(state,maximum)?;
            for object in objects{encoder.prepared(object);}
            entries.into_iter().map(|(k,v)|(format!("{k:020}"),v)).collect()
        },
        "operation_log"=>{
            let (objects,entries)=operations_log_encoding(state,maximum)?;
            for object in objects{encoder.prepared(object);}
            entries.into_iter().map(|(k,v)|(format!("{k:020}"),v)).collect()
        },
        "operation_locators"=>state.operations.cold_index().ok_or_else(||Error::new(storage_contract::ErrorCode::StateInvalid,"operation locator lineage is unavailable; recover before reconstructing"))?.iter().map(|(id,ordinal)|(id.to_string(),json_codec::Value::from(ordinal.to_string()))).collect(),
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
        let fields=[$(stringify!($field),)*"directory_selection_generations","file_selection_generations","operation_log","operation_locators"];
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
        let json_codec::Value::Object(map) = value else { panic!("State field is a map") };
        let measure = if field == "content_states" { ProofMeasure::ContentStates } else { ProofMeasure::Plain };
        let root = FieldRoot::Text(builder.encoder.map_root(map, measure)?);
        builder.fields.insert(field.into(), root);
    }};}
    state_map_fields!(fields);
    logical.remove("events");
    logical.remove("operations");
    // The oracle reads the typed logs directly: the logical JSON omits the
    // one-shot cold index and the retained fresh-seal body on purpose.
    let (objects, entries) = events_log_encoding(state, maximum)?;
    for object in objects {
        builder.encoder.prepared(object);
    }
    let events = builder
        .encoder
        .map_typed(entries, ProofMeasure::Plain, &segment_ordinal_text)?;
    let (objects, entries) = operations_log_encoding(state, maximum)?;
    for object in objects {
        builder.encoder.prepared(object);
    }
    let log_root =
        builder
            .encoder
            .map_typed(entries, ProofMeasure::Plain, &segment_ordinal_text)?;
    builder
        .fields
        .insert("operation_log".into(), FieldRoot::Segments(log_root));
    let locators = full_locator_root(&mut builder.encoder, state)?;
    builder
        .fields
        .insert("operation_locators".into(), FieldRoot::Operations(locators));
    let events_reference = events.reference().clone();
    let maps = builder
        .fields
        .iter()
        .map(|(f, r)| (f.clone(), r.reference().clone()))
        .collect();
    builder
        .fields
        .insert("events".into(), FieldRoot::Segments(events));
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
    static SEGMENT_DIRECTORY_SERIALIZATIONS:std::cell::RefCell<BTreeMap<&'static str,usize>>=const{std::cell::RefCell::new(BTreeMap::new())};
}
#[cfg(test)]
pub(super) fn take_encoding_counts() -> BTreeMap<String, usize> {
    SNAPSHOT_FIELD_SERIALIZATIONS.with(|counts| std::mem::take(&mut *counts.borrow_mut()))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/event_append_tests.rs"]
mod event_append_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/segment_directory_cost_tests.rs"]
mod segment_directory_cost_tests;
