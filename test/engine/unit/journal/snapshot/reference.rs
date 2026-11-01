use super::*;

/// Independent logical serializer oracle, not the typed dirty-field adapter.
pub(in crate::journal) fn prepare_reference(
    state: &State,
    maximum: usize,
) -> Result<PreparedSnapshot> {
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
