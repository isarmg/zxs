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

/// The sole typed map partitioner. The callback never retains the borrowed
/// entry map, and fallback installation can discard each encoded page in turn.
fn encode_map<K: AsRef<str> + Ord + Serialize, V: Serialize>(
    field: &str,
    entries: impl IntoIterator<Item = (K, V)>,
    maximum: usize,
    mut emit: impl FnMut(PreparedMetadataObject) -> Result<()>,
) -> Result<()> {
    let mut page = BTreeMap::new();
    let mut flush = |page: &mut BTreeMap<K, V>| -> Result<()> {
        #[cfg(test)]
        let value = CountedPage { field, page };
        #[cfg(not(test))]
        let value = &*page;
        #[cfg(not(test))]
        let _ = field;
        emit(PreparedMetadataObject::new("state-page", &value, maximum)?)?;
        page.clear();
        Ok(())
    };
    for (key, value) in entries {
        let boundary = blake3::hash(key.as_ref().as_bytes()).as_bytes()[0] & 31 == 0;
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
    fn map<K: AsRef<str> + Ord + Serialize, V: Serialize>(
        &mut self,
        field: &str,
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> Result<()> {
        let start = self.objects.len();
        let mut max_page_bytes = 0;
        let mut hashes = Vec::new();
        encode_map(field, entries, self.maximum, |object| {
            max_page_bytes = max_page_bytes.max(object.bytes.len());
            hashes.push(object.hash.clone());
            self.objects.push(object);
            Ok(())
        })?;
        self.fields.insert(
            field.to_owned(),
            SnapshotFieldPlan {
                objects: Some(start..self.objects.len()),
                max_page_bytes,
            },
        );
        self.pages.insert(field.to_owned(), hashes);
        Ok(())
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
                "RESOURCE_LIMIT",
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
                objects: None,
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
    builder.map(
        "events",
        state
            .events
            .iter()
            .enumerate()
            .map(|(index, event)| (format!("{index:020}"), event)),
    )?;
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
        maximum,
    })
}

/// Reconstruct only one field, keeping the original typed wire rules. The
/// caller binds every emitted page to the already published reference sequence.
pub(super) fn reconstruct_field(
    state: &State,
    field: &str,
    maximum: usize,
    emit: impl FnMut(PreparedMetadataObject) -> Result<()>,
) -> Result<()> {
    macro_rules! dispatch {
        (required { $($required:ident,)* } optional { $($optional:ident,)* }) => {
            match field {
                $(stringify!($required) => encode_map(field,state.$required.iter(),maximum,emit),)*
                $(stringify!($optional) => encode_map(field,state.$optional.iter(),maximum,emit),)*
                "directory_selection_generations" => encode_map(field,state.directory_selection_generations.iter().map(|(k,v)|(k,v.to_string())),maximum,emit),
                "file_selection_generations" => encode_map(field,state.file_selection_generations.iter().map(|(k,v)|(k,v.to_string())),maximum,emit),
                _ => Err(Error::new("STATE_INVALID","snapshot field is not eligible for reference reconstruction")),
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
