//! Deliberately small offline rescue utility. Stop the daemon before using it.
mod configuration;
mod private_io;
mod scrub;
use command_line::{Parser, Subcommand};
use json_codec::json;
use std::path::PathBuf;
use storage_engine::{Engine, OperationId};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Parser)]
#[command(
    name = "zxs-rescue",
    version,
    about = "Offline verified recovery; stop ZXS before use",
    after_help = "restore-system requires a majority of the configured original metadata members (one for a single-disk pool).
Recovery preview/commit only accepts decided, unpublished operations with all original disks accessible. It requires a reviewed digest and explicit permanent-loss acknowledgement; it is not a reset or missing-disk replacement.
Offline export validates one published version, not the entire pool or current permissions."
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
    command: Command,
}
#[derive(Subcommand)]
enum Command {
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
    OfflineCheck {
        disk: PathBuf,
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
        eprintln!("zxs-rescue: {error}");
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
            let mut message = error.to_string();
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                use std::fmt::Write;
                write!(&mut message, "; caused by: {cause}")
                    .expect("String formatting is infallible");
                source = cause.source();
            }
            eprintln!("zxs-rescue: {action}: {message}");
        }
    }
}

fn run(args: Args) -> Result<()> {
    let retained_workspace_limits = args.workspace_limits()?;
    let root = args.root;
    match args.command {
        Command::Configuration { command } => configuration::run(command),
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
                let operation = operation_id.unwrap_or_else(OperationId::generate);
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
            let pool = &engine.config().pool_id;
            let repositories = engine.state().repositories.len();
            let credentials_available = engine.auth_state().is_some();
            print(
                json!({"restored":true,"pool_id":pool,"repositories":repositories,"credential_recovery_snapshot_available":credentials_available}),
            )
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
            storage_engine::offline_export(&disk, &version, &output)?;
            print(json!({"version_id":version,"exported":output,"mode":"offline-isolated"}))
        }
    }
}

#[cfg(test)]
#[path = "../../../test/rescue/unit/main/args_tests.rs"]
mod args_tests;
