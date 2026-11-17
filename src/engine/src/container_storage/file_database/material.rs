//! Lossless local records for rescue archives; not the full publication graph.
use super::*;
use crate::container_storage::object_database::{
    ObjectDatabaseMaterial, ObjectDatabaseRecord, ObjectDatabaseScope,
};

/// Re-admit a physical FILE predecessor and compare all its local rows with
/// the frozen original. An unrelated publication can advance State without
/// rewriting this FILE; the scalar bound alone is not a native FILE frontier.
pub(in crate::container_storage) fn require_backup_predecessor(
    actual: &[u8], view: StateProjection<'_>, file: &str, history: &str,
) -> Result<()> {
    let observed = verify_snapshot(actual)?;
    if observed.file != file || observed.history != history || observed.sequence == 0
        || observed.sequence > view.state().sequence
    {
        return Err(invalid("physical Backup FILE is not a predecessor in the original scope"));
    }
    let original = CapturedMaterial::from_projection(view, file, history)?;
    if original.records_hash != observed.records_hash {
        return Err(invalid("physical Backup FILE differs from the original published rows"));
    }
    Ok(())
}

pub(in crate::container_storage) fn read_material(bytes: &[u8]) -> Result<ObjectDatabaseMaterial> {
    let captured = verify_snapshot(bytes)?;
    Ok(ObjectDatabaseMaterial::new(
        ObjectDatabaseScope::File {
            file_id: captured.file,
            history_tree_id: captured.history,
        },
        captured.sequence,
        captured.records_hash,
        captured
            .records
            .into_iter()
            .map(|row| ObjectDatabaseRecord::new(row.kind, row.id, row.body))
            .collect(),
    ))
}

pub(in crate::container_storage) fn reconstruct_material(
    material: &ObjectDatabaseMaterial,
    file: &str,
    history: &str,
) -> Result<Vec<u8>> {
    let mut records = Vec::with_capacity(material.records().len());
    for row in material.records() {
        let kind = match row.kind() {
            "file" => "file",
            "version" => "version",
            "branch" => "branch",
            "path_change" => "path_change",
            "metadata" => "metadata",
            "deletion" => "deletion",
            "lifecycle" => "lifecycle",
            "merge" => "merge",
            "merge_link" => "merge_link",
            _ => {
                return Err(invalid(
                    "file archive contains an unsupported local record kind",
                ));
            }
        };
        records.push(Record {
            kind,
            id: row.id().into(),
            body: row.body_json().into(),
        });
    }
    validate_records(&records, file, history, material.source_sequence())?;
    // The archive envelope has already checked this digest. Preserve its exact
    // bytes and hash; local row admission above does not grant publication.
    let encoded = CapturedMaterial {
        records,
        file: file.to_owned(),
        history: history.to_owned(),
        sequence: material.source_sequence(),
        records_hash: material.records_hash().to_owned(),
    }
    .encode()?;
    Ok(Arc::unwrap_or_clone(encoded.bytes))
}
