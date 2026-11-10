//! Standalone typed local compositions. Child/file references remain explicit
//! unresolved references, never fetched from external paths or mutable HEADs.
use super::*;
use crate::container_storage::object_database::{
    ObjectDatabaseMaterial, ObjectDatabaseRecord, ObjectDatabaseScope,
};
use data_serialization::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{
    Commit, DirectoryTree, Entry, NamespaceEntryKind, directory_git::DirectoryGitRepository,
};

mod rows;
pub(super) use rows::validate_rows;

pub(in crate::container_storage) fn read_material(bytes: &[u8]) -> Result<ObjectDatabaseMaterial> {
    let captured = verify(bytes)?;
    Ok(ObjectDatabaseMaterial::new(
        ObjectDatabaseScope::Directory {
            repository_id: captured.repository,
            directory_id: captured.directory.as_str().into(),
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
    repository: &str,
    directory: &str,
) -> Result<Vec<u8>> {
    let mut records = Vec::with_capacity(material.records().len());
    for row in material.records() {
        let kind = match row.kind() {
            "repository" => "repository",
            "branch" => "branch",
            "tree" => "tree",
            "commit" => "commit",
            _ => {
                return Err(invalid(
                    "directory archive contains an unsupported local record kind",
                ));
            }
        };
        records.push(Record {
            kind,
            id: row.id().into(),
            body: row.body_json().into(),
        });
    }
    validate_rows(&records, repository, directory, material.source_sequence())?;
    Ok(SCHEMA
        .encode(
            records,
            [repository, directory],
            material.source_sequence(),
            material.records_hash(),
        )?
        .bytes)
}

fn decode<T: for<'de> Deserialize<'de> + Serialize>(body: &str) -> Result<T> {
    let value: T = storage_contract::decode_strict(body.as_bytes())?;
    if crate::disk::encode_metadata(&value, projection::MAX_RECORD_BYTES)? != body.as_bytes() {
        return Err(invalid(
            "directory material row is not canonical typed metadata",
        ));
    }
    Ok(value)
}

fn sequence(value: u64, through: u64) -> Result<()> {
    if value == 0 || value > through {
        return Err(invalid(
            "directory material lies outside its captured sequence",
        ));
    }
    Ok(())
}

fn entries(entries: &[Entry]) -> Result<()> {
    let mut paths = BTreeSet::new();
    let mut identities = BTreeMap::<&str, &Entry>::new();
    for entry in entries {
        storage_contract::validate_id(&entry.entry_id, "entry")?;
        if !paths.insert(entry.lookup_key()) {
            return Err(invalid("directory local entries repeat a normalized path"));
        }
        if let Some(previous) = identities.insert(&entry.entry_id, entry)
            && (entry.kind != NamespaceEntryKind::File
                || previous.kind != NamespaceEntryKind::File
                || previous.file_id != entry.file_id)
        {
            return Err(invalid(
                "directory local entry identity is reused by different entities",
            ));
        }
        match entry.kind {
            NamespaceEntryKind::File => {
                storage_contract::validate_id(&entry.file_id, "file")?;
                storage_contract::validate_id(&entry.version_id, "ver")?;
            }
            NamespaceEntryKind::Directory => {
                if !entry.file_id.is_empty() || !entry.version_id.is_empty() || entry.size != 0 {
                    return Err(invalid("directory entry contains a fake file payload"));
                }
            }
        }
    }
    Ok(())
}

/// Linear traversal of each retained parent edge; unknown/missing parents and
/// cycles fail instead of dropping an inaccessible part of the local history.
fn parents_closed(parents: BTreeMap<&str, Option<&str>>) -> Result<()> {
    let mut resolved = BTreeSet::new();
    for id in parents.keys() {
        let mut visiting = BTreeSet::new();
        let mut next = Some(*id);
        while let Some(id) = next {
            if resolved.contains(id) {
                break;
            }
            if !visiting.insert(id) {
                return Err(invalid("directory material contains a parent cycle"));
            }
            next = *parents
                .get(id)
                .ok_or_else(|| invalid("directory material has a missing local parent"))?;
        }
        resolved.extend(visiting);
    }
    Ok(())
}
