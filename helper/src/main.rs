//! Offline auxiliary utility. Stop the daemon before using it.
mod account;
mod authority;
mod configuration;
mod console;
mod deployment;
mod disk_database_rescue;
mod disk_databases;
mod host;
mod initialization;
mod installation;
mod maintenance;
mod object_databases;
mod private_io;
mod scrub;
mod system_program;
use command_line::{Parser, Subcommand};
use json_codec::json;
use std::path::PathBuf;
use storage_engine::{Engine, OperationId};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Parser)]
#[command(
    name = "zxs-helper",
    version,
    about = "Offline auxiliary tool; stop ZXS before use",
    after_help = "restore-system requires a majority of the configured original metadata members (one for a single-disk pool).
Recovery preview/commit only accepts decided, unpublished operations with all original disks accessible. It requires a reviewed digest and explicit permanent-loss acknowledgement; it is not a reset or missing-disk replacement.
Offline export validates one published version, not the entire pool or current permissions.
Disk-databases reads selected local SQLite inventories without SYSTEM; local consistency is not publication authority or content health.
Object-database exports object-local metadata and reconstructs standalone rescue SQLite from that archive; it never installs an active database or grants authority.
With no command, start the root-only, unauthenticated maintenance Web console on 0.0.0.0:9040. Use a trusted isolated network and exit after use.
The matching release helper can install its embedded ZXS executable at /usr/local/bin/zxs and explicitly install its sealed same-source VFS at the fixed Debian Samba path, without overwriting different bytes or starting services.
Engine recovery commands require the current-daemon /zxs lifetime lease; host preview/prepare can create that root without initializing the system.
A missing nonlogin Unix service account and same-name primary group can be explicitly prepared through the fixed systemd-sysusers program. Existing accounts are never modified. A selected non-root local account can receive an empty private data layout; this creates no administrator, database or disk.
Systemd/Samba configuration can be prepared as an inactive bundle or installed at fixed paths without overwriting different files, reloading, starting or enabling services.
The independent Web console can create the first management-only administrator in the exact new empty private layout; no storage is initialized.
The independent Web console can reset an existing ZXS administrator password with exact confirmation; it revokes ZXS sessions but does not update disaster-recovery credential replicas or Samba passwords.
The Web console can initialize the exact first production batch of existing empty primary roots through a non-root worker; it neither mounts/formats devices nor starts a service.
The independent Web console can add and initialize an exact empty directory batch in an existing production pool, retaining original membership intents and native receipts; this is not foreign import or an independent-authority migration.
The independent Web console can copy an exact foreign level 2/3 source into all explicitly selected lower roots as one recoverable material batch; it does not admit the source or any lower member.
The Web console can explicitly enroll a missing local tdbsam Samba account for an existing independent non-root Unix user, including native creation of a missing passdb in its existing private directory. Existing-account password resets remain separate; Separate Samba Unix account and OS dependency installation and other backends are not implemented. VFS installation is a separate exact offline-payload confirmation, not package installation or SMB login validation.
The independent Web console can explicitly deregister an exact non-final-voter member with original D3/receipt checks; disk bytes remain, but current pool visibility/content access may be lost.
Complete installation readiness/all-path clean-start gating, final-voter terminal removal and foreign admission remain incomplete. Explicit activation starts only verified installed service units; it does not install dependencies or attest network SMB/file I/O. No planned-offline action is offered."
)]
struct Args {
    /// Host retained-workspace record limit; supply all four limits or none.
    #[arg(long, global = true, requires_all = ["max_retained_workspaces_per_principal", "max_retained_workspace_metadata_bytes", "max_retained_workspace_metadata_bytes_per_principal"])]
    max_retained_workspaces: Option<std::num::NonZeroUsize>,
    /// Host per-principal retained-workspace record limit.
    #[arg(long, global = true, requires_all = ["max_retained_workspaces", "max_retained_workspace_metadata_bytes", "max_retained_workspace_metadata_bytes_per_principal"])]
    max_retained_workspaces_per_principal: Option<std::num::NonZeroUsize>,
    /// Host retained-workspace metadata byte limit.
    #[arg(long, global = true, requires_all = ["max_retained_workspaces", "max_retained_workspaces_per_principal", "max_retained_workspace_metadata_bytes_per_principal"])]
    max_retained_workspace_metadata_bytes: Option<std::num::NonZeroU64>,
    /// Host per-principal retained-workspace metadata byte limit.
    #[arg(long, global = true, requires_all = ["max_retained_workspaces", "max_retained_workspaces_per_principal", "max_retained_workspace_metadata_bytes"])]
    max_retained_workspace_metadata_bytes_per_principal: Option<std::num::NonZeroU64>,
    #[arg(long, default_value = "./zxs-data", global = true)]
    root: PathBuf,
    #[arg(long, default_value = "admin", global = true)]
    principal: String,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Manually serve the independent maintenance console; no login or autostart.
    Serve(console::Options),
    /// Inspect or install the matching embedded ZXS/VFS bytes; no implicit service setup.
    Install {
        #[command(subcommand)]
        command: installation::Command,
    },
    /// Prepare a nonlogin service account, private layout and runtime directories.
    Initialization {
        #[command(subcommand)]
        command: initialization::Command,
    },
    /// Prepare the root-owned host lease directory only, not accounts or storage.
    Host {
        #[command(subcommand)]
        command: host::Command,
    },
    /// Prepare/install fixed files or explicitly activate verified installed ZXS services.
    Deployment {
        #[command(subcommand)]
        command: deployment::Command,
    },
    /// Explicit independent disk journal, separate from online pools. Stop all writers first.
    Authority {
        #[command(subcommand)]
        command: authority::Command,
    },
    /// Prepare or safely edit uninitialized topology drafts; never mutates a live pool.
    Configuration {
        #[command(subcommand)]
        command: configuration::Command,
    },
    RestoreSystem {
        #[arg(long,num_args=1..,required=true)]
        disks: Vec<PathBuf>,
    },
    RecoveryPreview {
        operation: OperationId,
        #[arg(long)]
        reason: String,
    },
    RecoveryCommit {
        recovery: String,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        accept_permanent_loss: bool,
    },
    /// Export, verify or restore a complete independently validated history package.
    Package {
        #[command(subcommand)]
        command: PackageCommand,
    },
    /// Scan retained publication metadata and required full copies; stop ZXS first.
    Scrub,
    /// Stream provisional physical workspace manifests, even when engine quotas are exhausted.
    WorkspaceInventory,
    OfflineCheck {
        disk: PathBuf,
    },
    /// Inspect current/history SQLite with explicit disk identity; never restores authority.
    DiskDatabases(disk_databases::Options),
    /// Inspect, export, reconstruct or verify isolated file/directory SQLite material.
    ObjectDatabase {
        #[command(subcommand)]
        command: object_databases::Command,
    },
    /// Export, rebuild or verify standalone disk SQLite rescue material; no active installation.
    DiskDatabaseRescue {
        #[command(subcommand)]
        command: disk_database_rescue::Command,
    },
    OfflineExport {
        disk: PathBuf,
        version: String,
        output: PathBuf,
    },
}
#[derive(Subcommand)]
enum PackageCommand {
    /// Destination must not exist; parent must be owned by this user and mode 0700.
    Export {
        history: String,
        destination: PathBuf,
    },
    Check {
        package: PathBuf,
    },
    Restore {
        package: PathBuf,
        #[arg(long)]
        primary_slot: String,
        #[arg(long, default_value_t = 1, value_parser = command_line::value_parser!(u32).range(1..))]
        copies: u32,
        #[arg(long)]
        operation_id: Option<OperationId>,
    },
}
fn print(value: impl data_serialization::Serialize) -> Result<()> {
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    writeln!(&mut output, "{}", json_codec::to_string_pretty(&value)?)?;
    output.flush()?;
    Ok(())
}
impl Args {
    fn workspace_limits(&self) -> Result<Option<storage_engine::RetainedWorkspaceLimits>> {
        match (
            self.max_retained_workspaces,
            self.max_retained_workspaces_per_principal,
            self.max_retained_workspace_metadata_bytes,
            self.max_retained_workspace_metadata_bytes_per_principal,
        ) {
            (None, None, None, None) => Ok(None),
            (Some(records), Some(principal_records), Some(bytes), Some(principal_bytes)) => {
                Ok(Some(storage_engine::RetainedWorkspaceLimits {
                    max_retained_workspaces: records.get(),
                    max_retained_workspaces_per_principal: principal_records.get(),
                    max_retained_workspace_metadata_bytes: bytes.get(),
                    max_retained_workspace_metadata_bytes_per_principal: principal_bytes.get(),
                }))
            }
            _ => Err("all four retained-workspace limits must be supplied together".into()),
        }
    }
}

/// Final local diagnostic boundary. Escape controls so untrusted paths and
/// nested causes cannot inject terminal commands or forge additional lines.
fn diagnostic_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = String::new();
    let mut current = Some(error);
    while let Some(cause) = current {
        if !message.is_empty() {
            message.push_str("; caused by: ");
        }
        for character in cause.to_string().chars() {
            if character.is_control() {
                message.extend(character.escape_default());
            } else {
                message.push(character);
            }
        }
        current = cause.source();
    }
    message
}

fn main() {
    let args = Args::parse();
    tracing::collector::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        // Stderr is the final diagnostic sink; do not recursively report its write failures.
        .log_internal_errors(false)
        .init();
    if let Err(error) = run(args) {
        eprintln!("zxs-helper: {}", diagnostic_chain(error.as_ref()));
        std::process::exit(1);
    }
}
fn report_recovery_warnings(engine: &mut Engine) {
    let storage_engine::RecoveryWarnings {
        seals,
        checkpoint_load,
        checkpoint_save,
    } = engine.take_recovery_warnings();
    for failure in seals {
        tracing::warn!(
            context = ?failure.context(),
            error = ?failure.cause(),
            "recovery seal verification failed; recovery evidence remains restricted"
        );
    }
    for (action, warning) in [
        ("checkpoint load failed; used full replay", checkpoint_load),
        ("checkpoint save skipped", checkpoint_save),
    ] {
        if let Some(error) = warning {
            let message = diagnostic_chain(&error);
            eprintln!("zxs-helper: {action}: {message}");
        }
    }
}

fn run(args: Args) -> Result<()> {
    let retained_workspace_limits = args.workspace_limits()?;
    let root = args.root;
    let command = args
        .command
        .unwrap_or_else(|| Command::Serve(console::Options::default()));
    let _lease = if requires_service_exclusion(&command) {
        Some(platform_io::service_lease::ServiceLease::host()?)
    } else {
        None
    };
    match command {
        Command::Serve(options) => console::run(options),
        Command::Install { command } => installation::run(command),
        Command::Initialization { command } => initialization::run(command),
        Command::Host { command } => host::run(command),
        Command::Deployment { command } => deployment::run(command),
        Command::Configuration { command } => configuration::run(command),
        Command::Authority { command } => authority::run(command),
        Command::DiskDatabases(options) => disk_databases::run(options),
        Command::ObjectDatabase { command } => object_databases::run(command),
        Command::DiskDatabaseRescue { command } => disk_database_rescue::run(command),
        Command::Package { command } => match command {
            PackageCommand::Check { package } => {
                print(storage_engine::offline_check_history_package(&package)?)
            }
            PackageCommand::Export {
                history,
                destination,
            } => {
                let mut engine =
                    Engine::open_with_workspace_limits(&root, retained_workspace_limits)?;
                report_recovery_warnings(&mut engine);
                print(engine.export_history_package(&history, &destination)?)
            }
            PackageCommand::Restore {
                package,
                primary_slot,
                copies,
                operation_id,
            } => {
                let mut engine =
                    Engine::open_with_workspace_limits(&root, retained_workspace_limits)?;
                report_recovery_warnings(&mut engine);
                let operation = match operation_id {
                    Some(id) => id,
                    None => OperationId::generate()?,
                };
                let restored = engine.restore_history_package(
                    &package,
                    &primary_slot,
                    copies,
                    &args.principal,
                    operation,
                );
                report_recovery_warnings(&mut engine);
                print(restored?)
            }
        },
        Command::RestoreSystem { disks } => {
            let mut engine = Engine::restore_from_disks_with_workspace_limits(
                &root,
                &disks,
                retained_workspace_limits,
            )?;
            report_recovery_warnings(&mut engine);
            disk_databases::report_restored(&engine)
        }
        Command::RecoveryPreview { operation, reason } => {
            let mut engine =
                Engine::open_for_recovery_with_workspace_limits(&root, retained_workspace_limits)?;
            report_recovery_warnings(&mut engine);
            let result = engine.prepare_degraded_recovery(operation, &args.principal, &reason);
            report_recovery_warnings(&mut engine);
            print(result?)
        }
        Command::RecoveryCommit {
            recovery,
            digest,
            accept_permanent_loss,
        } => {
            if !accept_permanent_loss {
                return Err(
                    "review the recovery preview and explicitly pass --accept-permanent-loss"
                        .into(),
                );
            }
            let mut engine =
                Engine::open_for_recovery_with_workspace_limits(&root, retained_workspace_limits)?;
            report_recovery_warnings(&mut engine);
            let result = engine.commit_degraded_recovery(&recovery, &digest, &args.principal, true);
            report_recovery_warnings(&mut engine);
            print(result?)
        }
        Command::Scrub => scrub::run(root, retained_workspace_limits),
        Command::WorkspaceInventory => {
            use std::io::Write;
            let inventory = storage_engine::WorkspaceInventory::open(&root)?;
            let mut output = std::io::stdout().lock();
            let mut mirrors = 0u64;
            for record in inventory {
                let record = record?;
                mirrors = mirrors
                    .checked_add(1)
                    .ok_or("workspace inventory count overflow")?;
                writeln!(output, "{}", json_codec::to_string(&record)?)?;
            }
            writeln!(
                output,
                "{}",
                json!({"complete":true,"physical_manifests":mirrors.to_string(),"authoritative":false})
            )?;
            output.flush()?;
            Ok(())
        }
        Command::OfflineCheck { disk } => {
            let report = storage_engine::offline_check(&disk)?;
            let healthy = report.issues.is_empty();
            print(report)?;
            if healthy {
                Ok(())
            } else {
                Err("offline integrity check reported issues".into())
            }
        }
        Command::OfflineExport {
            disk,
            version,
            output,
        } => {
            // The receipt represents this path as JSON text. Reject an
            // unrepresentable destination before exporting any verified bytes.
            let output_text = output
                .to_str()
                .ok_or("offline export output path must be valid UTF-8 for JSON output")?;
            storage_engine::offline_export(&disk, &version, &output)?;
            print(json!({"version_id":version,"exported":output_text,"mode":"offline-isolated"}))
        }
    }
}

/// Engine open can replay durable work even for a command named preview or
/// export. Keep its host lease until the synchronous command and all owners end.
fn requires_service_exclusion(command: &Command) -> bool {
    match command {
        Command::ObjectDatabase { command } => {
            object_databases::requires_service_exclusion(command)
        }
        Command::DiskDatabaseRescue { command } => {
            disk_database_rescue::requires_service_exclusion(command)
        }
        Command::RestoreSystem { .. }
        | Command::DiskDatabases(_)
        | Command::RecoveryPreview { .. }
        | Command::Scrub
        | Command::Package {
            command: PackageCommand::Export { .. } | PackageCommand::Restore { .. },
        } => true,
        Command::RecoveryCommit {
            accept_permanent_loss,
            ..
        } => *accept_permanent_loss,
        Command::Serve(_)
        | Command::Install { .. }
        | Command::Host { .. }
        | Command::Initialization { .. }
        | Command::Deployment { .. }
        | Command::Authority { .. }
        | Command::Configuration { .. }
        | Command::Package {
            command: PackageCommand::Check { .. },
        }
        | Command::WorkspaceInventory
        | Command::OfflineCheck { .. }
        | Command::OfflineExport { .. } => false,
    }
}

#[cfg(test)]
#[path = "../../test/helper/unit/main/args_tests.rs"]
mod args_tests;
