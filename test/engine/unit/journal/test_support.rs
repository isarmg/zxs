use super::*;

pub(crate) fn append(
    d: &DiskConfig,
    kind: RecordType,
    sequence: u64,
    candidate: &str,
    object: &str,
) -> Result<Receipt> {
    private_dir(log_path(d).parent().expect("journal parent"))?;
    let mut writer = OpenJournal::scan(d)?.into_writer(d)?;
    writer.repair_torn_tail()?;
    writer.append(d, kind, sequence, candidate, object)
}

/// Encode an immutable object once for all replicas, retaining the same hash
/// function, reader bounds, atomic publication and existing-byte checks.
pub(crate) fn store_object_replicas_bounded<T: Serialize + ?Sized>(
    members: &[DiskConfig],
    kind: &'static str,
    value: &T,
    maximum: usize,
) -> Result<String> {
    PreparedMetadataObject::new(kind, value, maximum)?.store(members)
}

pub(crate) fn map_addresses(
    disk: &DiskConfig,
    reference: &crate::metadata_map::Reference,
) -> Vec<(&'static str, String)> {
    let mut addresses = Vec::new();
    crate::metadata_map::visit_objects(
        reference,
        |kind, hash| load_authenticated_object(disk, kind, hash),
        |object| {
            addresses.push((object.kind(), object.hash().to_owned()));
            Ok(())
        },
    )
    .unwrap();
    addresses
}
pub(crate) fn snapshot_addresses(
    disk: &DiskConfig,
    snapshot: &Snapshot,
) -> Vec<(&'static str, String)> {
    let mut addresses = BTreeSet::new();
    for reference in snapshot
        .maps
        .values()
        .chain(std::iter::once(&snapshot.events))
    {
        addresses.extend(map_addresses(disk, reference));
    }
    addresses.insert((
        "state-control-auth",
        snapshot.control_auth.hash().to_owned(),
    ));
    addresses.into_iter().collect()
}
pub(crate) fn map_pages(
    disk: &DiskConfig,
    reference: &crate::metadata_map::Reference,
) -> Vec<String> {
    map_addresses(disk, reference)
        .into_iter()
        .filter_map(|(kind, hash)| (kind == "state-page").then_some(hash))
        .collect()
}
pub(crate) fn stored_map(
    members: &[DiskConfig],
    value: json_codec::Map<String, json_codec::Value>,
) -> crate::metadata_map::Reference {
    let mut encoder = crate::metadata_map::Encoder::new(MAX_METADATA);
    let reference = encoder.map(value).unwrap();
    for object in encoder.finish() {
        object.store(members).unwrap();
    }
    reference
}
pub(crate) fn candidate_catalog(
    disk: &DiskConfig,
    candidate: &Candidate,
) -> BTreeMap<String, String> {
    crate::history_package::resolve_catalog(&candidate.history_roots, |kind, hash| {
        load_authenticated_object(disk, kind, hash)
    })
    .unwrap()
    .0
}
pub(crate) fn set_candidate_catalog(
    members: &[DiskConfig],
    candidate: &mut Candidate,
    entries: BTreeMap<String, String>,
) {
    candidate.history_roots = stored_map(
        members,
        entries
            .into_iter()
            .map(|(id, hash)| (id, json_codec::Value::String(hash)))
            .collect(),
    );
}
