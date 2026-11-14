//! Lossless local records for rescue archives; not the full publication graph.
use super::*;
use crate::container_storage::object_database::{
    ObjectDatabaseMaterial, ObjectDatabaseRecord, ObjectDatabaseScope,
};

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
    Ok(CapturedMaterial {
        records,
        file: file.to_owned(),
        history: history.to_owned(),
        sequence: material.source_sequence(),
        records_hash: material.records_hash().to_owned(),
    }
    .encode()?
    .bytes)
}
