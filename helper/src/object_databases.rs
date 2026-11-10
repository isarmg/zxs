//! Export and reconstruction of standalone object database rescue artifacts.
use crate::{Result, print, private_io};
use command_line::Subcommand;
use data_serialization::Deserialize;
use json_codec::json;
use platform_io::fs_safety::{
    PrivateDirectory,
    linux::{MountPolicy, OpenAt2Root},
};
use std::path::{Path, PathBuf};
use storage_engine::{
    MAX_OBJECT_DATABASE_ARCHIVE_BYTES, ObjectDatabaseMaterial, ObjectDatabaseScope,
};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Inspect one exact surviving object directory without SYSTEM or disk roots.
    Inspect { selection: PathBuf },
    /// Save complete object-local records to a new .object-db.json rescue file.
    Export { selection: PathBuf, output: PathBuf },
    /// Recreate standalone SQLite from a rescue archive and explicit scope JSON.
    Rebuild {
        archive: PathBuf,
        scope: PathBuf,
        output: PathBuf,
    },
    /// Read back an existing rescue image against the exact archive; never repair it.
    Verify {
        archive: PathBuf,
        scope: PathBuf,
        image: PathBuf,
    },
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    entity_directory: PathBuf,
    expected_scope: ObjectDatabaseScope,
}

fn read(selection: &Path) -> Result<ObjectDatabaseMaterial> {
    let selection: Selection =
        storage_engine::decode_strict(&private_io::read_owned(selection, 64 * 1024)?)?;
    let directory = PrivateDirectory::open_existing(&selection.entity_directory)?;
    let path = directory.path().to_owned();
    let directory = OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)?;
    Ok(storage_engine::inspect_object_database_material(
        directory.into_directory(),
        path,
        &selection.expected_scope,
    )?)
}

fn summary(material: &ObjectDatabaseMaterial) -> json_codec::Value {
    let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
    for record in material.records() {
        *kinds.entry(record.kind()).or_default() += 1;
    }
    json!({
        "scope": material.scope(), "source_sequence": material.source_sequence().to_string(),
        "records_hash": material.records_hash(), "records": material.records().len(), "record_kinds": kinds,
        "system_root_opened": false, "disk_registry_opened": false,
        "source_publications_verified": false, "external_references_verified": false,
        "content_availability_verified": false, "authority_restored": false,
        "active_database_installed": false,
    })
}

/// The suffix excludes all normal managed database/authority names. This is
/// not a serving install entrypoint, even if the chosen parent is on a data disk.
fn artifact_path(path: &Path, suffix: &str) -> Result<()> {
    if !path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(suffix))
    {
        return Err(format!(
            "rescue output must end in {suffix}; active database installation is not supported"
        )
        .into());
    }
    Ok(())
}

fn reconstruct(
    archive: &Path,
    scope: &Path,
) -> Result<(storage_engine::ObjectDatabaseReconstruction, String)> {
    let expected: ObjectDatabaseScope =
        storage_engine::decode_strict(&private_io::read_owned(scope, 64 * 1024)?)?;
    let archive = private_io::read_owned(archive, MAX_OBJECT_DATABASE_ARCHIVE_BYTES)?;
    let digest = private_io::rescue_digest(&archive);
    Ok((
        storage_engine::reconstruct_object_database_material(&archive, &expected)?,
        digest,
    ))
}

pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::Inspect { selection } => print(summary(&read(&selection)?)),
        Command::Export { selection, output } => {
            artifact_path(&output, ".object-db.json")?;
            let material = read(&selection)?;
            let archive = material.archive_bytes()?;
            private_io::write_new(&output, &archive)?;
            print(
                json!({"exported": output, "archive_bytes": archive.len(), "material": summary(&material)}),
            )
        }
        Command::Rebuild {
            archive,
            scope,
            output,
        } => {
            artifact_path(&output, ".recovered.sqlite")?;
            let (rebuilt, archive_sha256) = reconstruct(&archive, &scope)?;
            let readback = private_io::publish_rescue_sqlite(
                &output,
                rebuilt.database_bytes(),
                rebuilt.maximum_database_bytes(),
                |bytes| Ok(rebuilt.verify_database_bytes(bytes)?),
            )?;
            print(
                json!({"reconstructed": output, "database_bytes": rebuilt.database_bytes().len(),
                "archive_sha256":archive_sha256, "file_readback_verified":true, "readback":readback,
                "material": summary(rebuilt.material())}),
            )
        }
        Command::Verify {
            archive,
            scope,
            image,
        } => {
            artifact_path(&image, ".recovered.sqlite")?;
            let (rebuilt, archive_sha256) = reconstruct(&archive, &scope)?;
            let readback = private_io::verify_rescue_sqlite(
                &image,
                rebuilt.maximum_database_bytes(),
                |bytes| Ok(rebuilt.verify_database_bytes(bytes)?),
            )?;
            print(json!({"verified":image, "archive_sha256":archive_sha256,
                "file_readback_verified":true, "readback":readback, "files_modified":false,
                "material":summary(rebuilt.material())}))
        }
    }
}

#[cfg(test)]
#[path = "../../test/helper/unit/object_databases_tests.rs"]
mod tests;
