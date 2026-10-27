//! Page-at-a-time encoding and exact unchanged-field references.
use super::*;

// One schema drives preparation and fallback field dispatch. Exhaustive State
// matching below makes new fields a compile-time classification obligation.
macro_rules! state_map_fields {
    ($consumer:ident) => {
        $consumer! {
            required {
                repositories, histories, branches, commits, versions, placements,
                merge_links, history_boundaries, content_states, operations, plans,
                placement_rules, protection_policies, operation_principals,
            }
            optional {
                file_selections, directory_branches, directory_trees, file_write_origins,
                file_contexts, file_path_nodes, file_metadata_nodes, file_branch_merges,
                file_branch_aliases, file_node_parents, file_path_notes,
                directory_tree_selections, file_locations, file_backup_paths,
                file_backup_policies, directory_selections, file_notes, version_notes, file_names,
            }
        }
    };
}

#[cfg(test)]
struct CountedPage<'a, K, V> {
    field: &'a str,
    page: &'a BTreeMap<K, V>,
}
#[cfg(test)]
impl<K: Serialize, V: Serialize> Serialize for CountedPage<'_, K, V> {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        SNAPSHOT_FIELD_SERIALIZATIONS.with(|counts| {
            *counts
                .borrow_mut()
                .entry(self.field.to_owned())
                .or_default() += 1;
        });
        self.page.serialize(serializer)
    }
}

trait SnapshotKey {
    fn partition_hash(&self) -> blake3::Hash;
}
impl SnapshotKey for String {
    fn partition_hash(&self) -> blake3::Hash {
        blake3::hash(self.as_bytes())
    }
}
impl SnapshotKey for OperationId {
    fn partition_hash(&self) -> blake3::Hash {
        blake3::hash(self.to_string().as_bytes())
    }
}
impl<T: SnapshotKey> SnapshotKey for &T {
    fn partition_hash(&self) -> blake3::Hash {
        (*self).partition_hash()
    }
}

/// The sole typed map partitioner. The callback never retains the borrowed
/// entry map, and fallback installation can discard each encoded page in turn.
fn encode_map<K: SnapshotKey + Ord + Serialize, V: Serialize>(
    field: &str,
    entries: impl IntoIterator<Item = (K, V)>,
    maximum: usize,
    mut emit: impl FnMut(PreparedMetadataObject, usize) -> Result<()>,
) -> Result<()> {
    let mut page = BTreeMap::new();
    let mut flush = |page: &mut BTreeMap<K, V>| -> Result<()> {
        #[cfg(test)]
        let value = CountedPage { field, page };
        #[cfg(not(test))]
        let value = &*page;
        #[cfg(not(test))]
        let _ = field;
        emit(
            PreparedMetadataObject::new("state-page", &value, maximum)?,
            page.len(),
        )?;
        page.clear();
        Ok(())
    };
    for (key, value) in entries {
        let boundary = key.partition_hash().as_bytes()[0] & 31 == 0;
        if !page.is_empty() && (page.len() == STATE_PAGE_ENTRIES || boundary) {
            flush(&mut page)?;
        }
        page.insert(key, value);
        #[cfg(test)]
        SNAPSHOT_PEAK_PAGE_ENTRIES.with(|peak| peak.set(peak.get().max(page.len())));
    }
    if !page.is_empty() {
        flush(&mut page)?;
    }
    Ok(())
}

struct SnapshotBuilder {
    maximum: usize,
    pages: BTreeMap<String, Vec<String>>,
    objects: Vec<PreparedMetadataObject>,
    fields: BTreeMap<String, SnapshotFieldPlan>,
}
impl SnapshotBuilder {
    fn map<K: SnapshotKey + Ord + Serialize, V: Serialize>(
        &mut self,
        field: &str,
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> Result<()> {
        let start = self.objects.len();
        let mut max_page_bytes = 0;
        let mut hashes = Vec::new();
        encode_map(field, entries, self.maximum, |object, _| {
            max_page_bytes = max_page_bytes.max(object.bytes.len());
            hashes.push(object.hash.clone());
            self.objects.push(object);
            Ok(())
        })?;
        self.fields.insert(
            field.to_owned(),
            SnapshotFieldPlan {
                reused_pages: 0,
                objects: start..self.objects.len(),
                max_page_bytes,
            },
        );
        self.pages.insert(field.to_owned(), hashes);
        Ok(())
    }
    /// Append preserves every completed page. Re-encode the former tail page
    /// with the suffix so hash-anchor and forced-size boundaries stay identical.
    fn events(
        &mut self,
        state: &State,
        previous: Option<(&State, &SnapshotReuse)>,
    ) -> Result<usize> {
        let previous =
            previous.map(|(before, basis)| (basis, state.events.changes_since(&before.events)));
        let (start, mut max_page_bytes) = match previous {
            Some((basis, storage_contract::SequenceChanges::Append { start })) => {
                assert!(
                    self.reuse("events", basis)?,
                    "invariant: snapshot basis contains events"
                );
                if state.events.len() == start {
                    return Ok(basis.event_tail_start);
                }
                self.pages
                    .get_mut("events")
                    .expect("invariant: reused events have pages")
                    .pop();
                (basis.event_tail_start, basis.page_maxima["events"])
            }
            Some((_, storage_contract::SequenceChanges::All)) | None => {
                self.pages.insert("events".into(), Vec::new());
                (0, 0)
            }
        };
        let object_start = self.objects.len();
        let hashes = self
            .pages
            .get_mut("events")
            .expect("invariant: events have a page list");
        let reused_pages = hashes.len();
        let mut offset = start;
        let mut tail_start = start;
        encode_map(
            "events",
            state.events[start..]
                .iter()
                .enumerate()
                .map(|(index, event)| (format!("{:020}", start + index), event)),
            self.maximum,
            |object, count| {
                tail_start = offset;
                offset += count;
                max_page_bytes = max_page_bytes.max(object.bytes.len());
                hashes.push(object.hash.clone());
                self.objects.push(object);
                Ok(())
            },
        )?;
        self.fields.insert(
            "events".into(),
            SnapshotFieldPlan {
                reused_pages,
                objects: object_start..self.objects.len(),
                max_page_bytes,
            },
        );
        Ok(tail_start)
    }
    fn reuse(&mut self, field: &str, basis: &SnapshotReuse) -> Result<bool> {
        let (Some(pages), Some(&max_page_bytes)) = (
            basis.descriptor.pages.get(field),
            basis.page_maxima.get(field),
        ) else {
            return Ok(false);
        };
        if max_page_bytes > self.maximum {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                format!(
                    "serialized metadata exceeds {} byte reader limit",
                    self.maximum
                ),
            ));
        }
        self.pages.insert(field.to_owned(), pages.clone());
        self.fields.insert(
            field.to_owned(),
            SnapshotFieldPlan {
                reused_pages: pages.len(),
                objects: self.objects.len()..self.objects.len(),
                max_page_bytes,
            },
        );
        Ok(true)
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
    #[cfg(test)]
    SNAPSHOT_PEAK_PAGE_ENTRIES.with(|peak| peak.set(0));
    let mut builder = SnapshotBuilder {
        maximum,
        pages: BTreeMap::new(),
        objects: Vec::new(),
        fields: BTreeMap::new(),
    };
    macro_rules! map_fields {
        (required { $($required:ident,)* } optional { $($optional:ident,)* }) => {{
            let State {
                sequence: _, business_generation: _, topology_generation: _, control_auth: _, events: _,
                directory_selection_generations: _, file_selection_generations: _,
                $($required,)* $($optional,)*
            } = state;
            $(if !match previous {
                Some((before, basis)) if $required.changes_since(&before.$required).is_empty() => builder.reuse(stringify!($required), basis)?,
                _ => false,
            } { builder.map(stringify!($required), $required.iter())?; })*
            $(if !$optional.is_empty() && !match previous {
                Some((before, basis)) if $optional.changes_since(&before.$optional).is_empty() => builder.reuse(stringify!($optional), basis)?,
                _ => false,
            } { builder.map(stringify!($optional), $optional.iter())?; })*
        }};
    }
    state_map_fields!(map_fields);
    macro_rules! string_counter_map {
        ($field:ident) => {
            if !state.$field.is_empty()
                && !match previous {
                    Some((before, basis))
                        if state.$field.changes_since(&before.$field).is_empty() =>
                    {
                        builder.reuse(stringify!($field), basis)?
                    }
                    _ => false,
                }
            {
                builder.map(
                    stringify!($field),
                    state
                        .$field
                        .iter()
                        .map(|(key, value)| (key, value.to_string())),
                )?;
            }
        };
    }
    string_counter_map!(directory_selection_generations);
    string_counter_map!(file_selection_generations);
    let event_tail_start = builder.events(state, previous)?;
    let mut scalars = json_codec::Map::new();
    scalars.insert("sequence".into(), state.sequence.to_string().into());
    scalars.insert(
        "business_generation".into(),
        state.business_generation.to_string().into(),
    );
    scalars.insert(
        "topology_generation".into(),
        state.topology_generation.to_string().into(),
    );
    if let Some(json_codec::Value::Object(map)) = &state.control_auth {
        builder.map("control_auth", map.iter())?;
    } else {
        scalars.insert(
            "control_auth".into(),
            json_codec::to_value(&state.control_auth)?,
        );
    }
    let descriptor = Snapshot {
        sequence: state.sequence,
        pages: builder.pages,
        scalars,
    };
    let root = PreparedMetadataObject::new("snapshot", &descriptor, maximum)?;
    let hash = root.hash.clone();
    builder.objects.push(root);
    Ok(PreparedSnapshot {
        descriptor,
        hash,
        objects: builder.objects,
        fields: builder.fields,
        event_tail_start,
        maximum,
    })
}

/// Reconstruct only one field, keeping the original typed wire rules. The
/// caller binds every emitted page to the already published reference sequence.
pub(super) fn reconstruct_field(
    state: &State,
    field: &str,
    maximum: usize,
    mut emit: impl FnMut(PreparedMetadataObject) -> Result<()>,
) -> Result<()> {
    let emit = |object, _| emit(object);
    macro_rules! dispatch {
        (required { $($required:ident,)* } optional { $($optional:ident,)* }) => {
            match field {
                $(stringify!($required) => encode_map(field,state.$required.iter(),maximum,emit),)*
                $(stringify!($optional) => encode_map(field,state.$optional.iter(),maximum,emit),)*
                "directory_selection_generations" => encode_map(field,state.directory_selection_generations.iter().map(|(k,v)|(k,v.to_string())),maximum,emit),
                "file_selection_generations" => encode_map(field,state.file_selection_generations.iter().map(|(k,v)|(k,v.to_string())),maximum,emit),
                "events" => encode_map(field, state.events.iter().enumerate().map(|(index, event)| (format!("{index:020}"), event)), maximum, emit),
                _ => Err(Error::new(storage_contract::ErrorCode::StateInvalid,"snapshot field is not eligible for reference reconstruction")),
            }
        };
    }
    state_map_fields!(dispatch)
}

#[cfg(test)]
thread_local! {
    pub(super) static SNAPSHOT_PEAK_PAGE_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static SNAPSHOT_FIELD_SERIALIZATIONS: std::cell::RefCell<BTreeMap<String,usize>> = const { std::cell::RefCell::new(BTreeMap::new()) };
}
#[cfg(test)]
pub(super) fn take_encoding_counts() -> BTreeMap<String, usize> {
    SNAPSHOT_FIELD_SERIALIZATIONS.with(|counts| std::mem::take(&mut *counts.borrow_mut()))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/event_append_tests.rs"]
mod event_append_tests;
/// Check the current writer schema without reconstructing every historical State.
/// The caller still authenticates the exact root and every referenced page.
pub(crate) fn validate_schema(snapshot: &Snapshot) -> Result<()> {
    #[derive(Deserialize)]
    #[serialization(deny_unknown_fields)]
    struct Header {
        #[serialization(with = "storage_contract::u64_string")]
        sequence: u64,
        #[serialization(rename = "business_generation", with = "storage_contract::u64_string")]
        _business_generation: u64,
        #[serialization(rename = "topology_generation", with = "storage_contract::u64_string")]
        _topology_generation: u64,
        #[serialization(rename = "control_auth")]
        _control_auth: Option<json_codec::Value>,
    }
    let Header {
        sequence,
        _business_generation: _,
        _topology_generation: _,
        _control_auth: _,
    } = json_codec::from_value(json_codec::Value::Object(snapshot.scalars.clone()))?;
    if sequence != snapshot.sequence
        || snapshot.pages.contains_key("control_auth")
            == snapshot.scalars.contains_key("control_auth")
        || !snapshot.pages.contains_key("events")
    {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "snapshot scalar/page layout differs",
        ));
    }
    macro_rules! check_fields {
        (required { $($required:ident,)* } optional { $($optional:ident,)* }) => {
            if [$(stringify!($required),)*].iter().any(|field| !snapshot.pages.contains_key(*field)) {
                return Err(Error::new(ErrorCode::ObjectCorrupt, "snapshot required field is absent"));
            }
            for field in snapshot.pages.keys() {
                match field.as_str() {
                    $(stringify!($required) |)* $(stringify!($optional) |)*
                    "directory_selection_generations" | "file_selection_generations" |
                    "events" | "control_auth" => (),
                    _ => return Err(Error::new(ErrorCode::ObjectCorrupt, "snapshot contains an unknown field")),
                }
            }
        };
    }
    state_map_fields!(check_fields);
    Ok(())
}
