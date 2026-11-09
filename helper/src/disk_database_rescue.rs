//! Standalone rescue files, never active disk database installation.
use crate::{Result, disk_databases, print, private_io};
use command_line::{Subcommand, ValueEnum};
use json_codec::json;
use std::path::{Path, PathBuf};
use storage_engine::{
    DiskDatabaseRebuildSource, DiskRegistrationIdentity, MAX_DISK_DATABASE_ARCHIVE_BYTES,
    reconstruct_disk_database_material,
};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Freeze selected local databases into a new .disk-db.json rescue archive.
    Export {
        selection: PathBuf,
        #[arg(long, value_enum, default_value = "both")]
        scope: disk_databases::Scope,
        output: PathBuf,
    },
    /// Reconstruct one new .recovered.sqlite; no SYSTEM or original disk is opened.
    Rebuild {
        archive: PathBuf,
        /// Owned JSON containing the explicit DiskRegistrationIdentity.
        identity: PathBuf,
        #[arg(long, value_enum)]
        source: Source,
        output: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Source {
    StoredCurrent,
    History,
    CurrentFromHistory,
}
impl Source {
    fn engine(self) -> DiskDatabaseRebuildSource {
        match self {
            Self::StoredCurrent => DiskDatabaseRebuildSource::StoredCurrent,
            Self::History => DiskDatabaseRebuildSource::History,
            Self::CurrentFromHistory => DiskDatabaseRebuildSource::CurrentFromHistory,
        }
    }
}

pub(crate) fn requires_service_exclusion(command: &Command) -> bool {
    matches!(command, Command::Export { .. })
}

fn require_suffix(path: &Path, suffix: &str) -> Result<()> {
    if !path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(suffix)) {
        return Err(format!("rescue output must end with {suffix}; active database names are forbidden").into());
    }
    Ok(())
}

pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::Export { selection, scope, output } => {
            require_suffix(&output, ".disk-db.json")?;
            let material = disk_databases::inspect(&selection, scope)?;
            let bytes = material.archive_bytes()?;
            private_io::write_new(&output, &bytes)?;
            print(json!({
                "format":"disk-database-rescue-export",
                "output":output,
                "archive_bytes":bytes.len(),
                "identity":material.identity(),
                "scope":material.scope(),
                "relation":material.relation(),
                "stored_current_sequence":material.current().map(|row| row.source_sequence.to_string()),
                "history_through_sequence":material.history().map(|row| row.header.through_sequence.to_string()),
                "system_root_opened":false,
                "source_publications_verified":false,
                "content_availability_verified":false,
                "authority_restored":false,
                "active_database_installed":false,
            }))
        }
        Command::Rebuild { archive, identity, source, output } => {
            require_suffix(&output, ".recovered.sqlite")?;
            let expected: DiskRegistrationIdentity = storage_engine::decode_strict(
                &private_io::read_owned(&identity, 64 * 1024)?)?;
            let bytes = private_io::read_owned(&archive, MAX_DISK_DATABASE_ARCHIVE_BYTES)?;
            let image = reconstruct_disk_database_material(&bytes, &expected, source.engine())?;
            private_io::write_new(&output, image.database_bytes())?;
            print(json!({
                "format":"disk-database-rescue-reconstruction",
                "output":output,
                "identity":image.identity(),
                "source":image.source(),
                "source_sequence":image.source_sequence().to_string(),
                "records":image.records(),
                "records_hash":image.records_hash(),
                "database_bytes":image.database_bytes().len(),
                "system_root_opened":false,
                "source_disk_opened":false,
                "source_publications_verified":false,
                "content_availability_verified":false,
                "authority_restored":false,
                "active_database_installed":false,
            }))
        }
    }
}

#[cfg(test)]
#[path = "../../test/helper/unit/disk_database_rescue_tests.rs"]
mod tests;
