use crate::{Result, diagnostic_chain, print, private_io};
use command_line::{Args, ValueEnum};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use platform_io::fs_safety::{
    PrivateDirectory,
    linux::{MountPolicy, OpenAt2Root},
};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use storage_engine::{
    DiskCatalogMaintenanceState, DiskDatabaseReadScope, DiskHistoryMaintenanceState,
    DiskRegistrationIdentity, Engine, IndexHealth,
};

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Scope {
    Both,
    Current,
    History,
}

#[derive(Args)]
pub(crate) struct Options {
    /// Owned JSON file: explicit root plus expected_identity, not a pool Config.
    selection: PathBuf,
    /// Select one intact file when the other is damaged; omitted files stay uninspected.
    #[arg(long, value_enum, default_value = "both")]
    scope: Scope,
    /// Emit bounded record lines to stdout after the complete frozen read succeeds.
    #[arg(long)]
    records: bool,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    root: PathBuf,
    expected_identity: DiskRegistrationIdentity,
}

#[derive(Serialize)]
#[serialization(tag = "record", content = "value", rename_all = "kebab-case")]
enum Record<'a> {
    CurrentVersion(&'a storage_engine::DiskCatalogVersion),
    CurrentBackupDirectory(&'a storage_engine::DiskCatalogDirectory),
    HistoryChange(&'a storage_engine::DiskHistoryRecord),
    HistoryInventoryVersion(&'a storage_engine::DiskCatalogVersion),
    HistoryInventoryBackupDirectory(&'a storage_engine::DiskCatalogDirectory),
}

pub(crate) fn inspect(path: &Path, scope: Scope) -> Result<storage_engine::DiskDatabaseMaterial> {
    let selection: Selection =
        storage_engine::decode_strict(&private_io::read_owned(path, 64 * 1024)?)?;
    let directory = PrivateDirectory::open_existing(&selection.root)?;
    let path = directory.path().to_path_buf();
    let directory = OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)?;
    let scope = match scope {
        Scope::Both => DiskDatabaseReadScope::Both,
        Scope::Current => DiskDatabaseReadScope::Current,
        Scope::History => DiskDatabaseReadScope::History,
    };
    Ok(storage_engine::inspect_disk_database_material(
        directory.into_directory(),
        path,
        &selection.expected_identity,
        scope,
    )?)
}

pub(crate) fn run(options: Options) -> Result<()> {
    let material = inspect(&options.selection, options.scope)?;
    let summary = json!({
        "format":"disk-database-material",
        "scope":material.scope(),
        "identity":material.identity(),
        "relation":material.relation(),
        "current":material.current().map(|current| json!({
            "source_sequence":current.source_sequence.to_string(),
            "versions":current.versions.len(),
            "backup_directories":current.backup_directories.len(),
        })),
        "history":material.history().map(|history| json!({
            "header":history.header,
            "changes":history.changes.len(),
        })),
        "history_inventory":material.history_inventory().map(|inventory| json!({
            "source_sequence":inventory.source_sequence.to_string(),
            "versions":inventory.versions.len(),
            "backup_directories":inventory.backup_directories.len(),
        })),
        "system_root_opened":false,
        "source_publications_verified":false,
        "content_availability_verified":false,
        "authority_restored":false,
        "disk_files_modified":false,
        "unselected_files":"not_inspected",
    });
    if !options.records {
        return print(summary);
    }
    let mut output = std::io::stdout().lock();
    writeln!(
        output,
        "{}",
        json_codec::to_string(&json!({"summary":summary}))?
    )?;
    if let Some(current) = material.current() {
        for row in &current.versions {
            writeln!(
                output,
                "{}",
                json_codec::to_string(&Record::CurrentVersion(row))?
            )?;
        }
    }
    if let Some(current) = material.current() {
        for row in &current.backup_directories {
            writeln!(
                output,
                "{}",
                json_codec::to_string(&Record::CurrentBackupDirectory(row))?
            )?;
        }
    }
    if let Some(history) = material.history() {
        for row in &history.changes {
            writeln!(
                output,
                "{}",
                json_codec::to_string(&Record::HistoryChange(row))?
            )?;
        }
    }
    if let Some(inventory) = material.history_inventory() {
        for row in &inventory.versions {
            writeln!(
                output,
                "{}",
                json_codec::to_string(&Record::HistoryInventoryVersion(row))?
            )?;
        }
    }
    if let Some(inventory) = material.history_inventory() {
        for row in &inventory.backup_directories {
            writeln!(
                output,
                "{}",
                json_codec::to_string(&Record::HistoryInventoryBackupDirectory(row))?
            )?;
        }
    }
    writeln!(
        output,
        "{}",
        json!({"complete":true,"authority_restored":false})
    )?;
    output.flush()?;
    Ok(())
}

pub(crate) fn report_restored(engine: &Engine) -> Result<()> {
    let current: Vec<_> = engine.disk_catalog_statuses().into_iter().map(|disk| {
        let state = match disk.state {
            DiskCatalogMaintenanceState::Unbuilt => json!({"kind":"unbuilt"}),
            DiskCatalogMaintenanceState::Excluded => json!({"kind":"excluded"}),
            DiskCatalogMaintenanceState::Pending { last_materialized_sequence } => json!({
                "kind":"pending", "last_materialized_sequence":last_materialized_sequence.map(|n| n.to_string()),
            }),
            DiskCatalogMaintenanceState::Installed { materialized_sequence, local_state_sequence, records_hash } => json!({
                "kind":"installed", "materialized_sequence":materialized_sequence.to_string(),
                "local_state_sequence":local_state_sequence.to_string(), "records_hash":records_hash,
            }),
            DiskCatalogMaintenanceState::Unavailable(failure) => json!({
                "kind":"unavailable", "sequence":failure.sequence().to_string(),
                "error":diagnostic_chain(failure.as_ref()),
            }),
        };
        json!({"identity":disk.identity,"observed_sequence":disk.observed_sequence.to_string(),"state":state})
    }).collect();
    let history: Vec<_> = engine.disk_history_statuses().into_iter().map(|disk| {
        let state = match disk.state {
            DiskHistoryMaintenanceState::Unbuilt => json!({"kind":"unbuilt"}),
            DiskHistoryMaintenanceState::Excluded => json!({"kind":"excluded"}),
            DiskHistoryMaintenanceState::Pending { persisted_through_sequence } => json!({
                "kind":"pending", "persisted_through_sequence":persisted_through_sequence.map(|n| n.to_string()),
            }),
            DiskHistoryMaintenanceState::Tracked { persisted_through_sequence, observed_through_sequence, observed_publications, records_hash } => json!({
                "kind":"tracked", "persisted_through_sequence":persisted_through_sequence.to_string(),
                "observed_through_sequence":observed_through_sequence.to_string(),
                "observed_publications":observed_publications,"records_hash":records_hash,
            }),
            DiskHistoryMaintenanceState::Unavailable(failure) => json!({
                "kind":"unavailable", "sequence":failure.sequence().to_string(),
                "error":diagnostic_chain(failure.as_ref()),
            }),
        };
        json!({"identity":disk.identity,"observed_sequence":disk.observed_sequence.to_string(),"state":state})
    }).collect();
    let system_index = match engine.index_health() {
        IndexHealth::Unbuilt => json!({"kind":"unbuilt"}),
        IndexHealth::Ready { sequence } => json!({"kind":"ready","sequence":sequence.to_string()}),
        IndexHealth::Unavailable(failure) => json!({
            "kind":"unavailable", "sequence":failure.sequence().to_string(),
            "error":diagnostic_chain(failure.as_ref()),
        }),
    };
    print(json!({
        "restored":true,
        "restored_scope":"authoritative-system-state",
        "pool_id":engine.config().pool_id,
        "repositories":engine.state().repositories.len(),
        "credential_recovery_snapshot_available":engine.auth_state().is_some(),
        "all_database_scopes_restored":false,
        "system_derived_index":system_index,
        "disk_current_catalogs":current,
        "disk_history_catalogs":history,
    }))
}
