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
