mod offline;
use command_line::Parser;
use std::{net::SocketAddr, path::PathBuf};
use storage_server::ServerConfig;
use tracing::collector::{EnvFilter, filter::LevelFilter};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, error_definition::Error)]
enum LogFilterError {
    #[error("RUST_LOG contains an invalid filter directive")]
    InvalidDirective(#[source] tracing::collector::filter::ParseError),
    #[error("RUST_LOG must contain valid Unicode")]
    NotUnicode(#[source] std::env::VarError),
}

fn log_filter_from_env() -> std::result::Result<EnvFilter, LogFilterError> {
    match std::env::var(EnvFilter::DEFAULT_ENV) {
        Ok(directives) => EnvFilter::try_new(directives).map_err(LogFilterError::InvalidDirective),
        Err(std::env::VarError::NotPresent) => {
            Ok(EnvFilter::default().add_directive(LevelFilter::WARN.into()))
        }
        Err(error @ std::env::VarError::NotUnicode(_)) => Err(LogFilterError::NotUnicode(error)),
    }
}

/// Direct service entry. Offline preparation belongs to the independent helper.
#[derive(Parser)]
#[command(
    name = "zxs",
    version,
    about = "ZXS storage and Web management daemon",
    after_help = "The current daemon requires an existing root-owned /zxs maintenance root and holds its exclusive OS lease until all runtime workers exit. Prepare the root with zxs-helper host preview/prepare. The root is not proof of completed system initialization. Before any listener or automatic directory creation, normal startup requires existing private SYSTEM/management directories, storage configuration and completed initial activation; finish first preparation with the independent zxs-helper. Existing storage/authentication faults remain faults, never a fresh account setup. Stop older or nonparticipating writers separately; no service manager is invoked."
)]
struct Args {
    #[command(subcommand)]
    command: Option<offline::Command>,
    /// Host retained-workspace record limit; supply all four limits or none.
    #[arg(long, requires_all = ["max_retained_workspaces_per_principal", "max_retained_workspace_metadata_bytes", "max_retained_workspace_metadata_bytes_per_principal"])]
    max_retained_workspaces: Option<std::num::NonZeroUsize>,
    /// Host per-principal retained-workspace record limit.
    #[arg(long, requires_all = ["max_retained_workspaces", "max_retained_workspace_metadata_bytes", "max_retained_workspace_metadata_bytes_per_principal"])]
    max_retained_workspaces_per_principal: Option<std::num::NonZeroUsize>,
    /// Host retained-workspace metadata byte limit.
    #[arg(long, requires_all = ["max_retained_workspaces", "max_retained_workspaces_per_principal", "max_retained_workspace_metadata_bytes_per_principal"])]
    max_retained_workspace_metadata_bytes: Option<std::num::NonZeroU64>,
    /// Host per-principal retained-workspace metadata byte limit.
    #[arg(long, requires_all = ["max_retained_workspaces", "max_retained_workspaces_per_principal", "max_retained_workspace_metadata_bytes"])]
    max_retained_workspace_metadata_bytes_per_principal: Option<std::num::NonZeroU64>,
    /// Non-overlapping automatic scrub interval, measured after each completed attempt.
    #[arg(long, default_value = "86400")]
    scrub_interval_seconds: std::num::NonZeroU64,
    /// Expire full-integrity observations from scan start, not ordinary writes.
    #[arg(long, default_value = "172800")]
    scrub_max_age_seconds: std::num::NonZeroU64,
    /// Maximum background read rate; one bounded read may burst before accounting.
    #[arg(long, default_value = "67108864")]
    scrub_bytes_per_second: std::num::NonZeroU64,
    #[arg(long, default_value = "./zxs-data")]
    root: PathBuf,
    /// System-disk management state; defaults to ROOT/management, never a data member.
    #[arg(long)]
    management_root: Option<PathBuf>,
    /// Discover only immediate disk directories under this service-managed parent.
    #[arg(long, default_value = "/zxs/data/disks")]
    storage_discovery_root: storage_server::storage_directories::DiscoveryRoot,
    /// Locally approved existing disk root, repeated as opaque ID=/absolute/path.
    #[arg(long = "setup-disk")]
    setup_disks: Vec<String>,
    /// Locally reviewed private configuration preset, repeated as ID=/absolute/file.
    #[arg(long = "setup-config")]
    setup_configs: Vec<String>,
    /// Test-only: allow development-mode pool setup; production also accepts managed directories.
    #[arg(long)]
    allow_development_setup: bool,

    #[arg(long, default_value = "0.0.0.0:7400")]
    pub(crate) listen: SocketAddr,
    #[arg(long)]
    pub(crate) external_origin: Option<String>,
    /// Trust this exact proxy peer; it must overwrite X-Forwarded-For with one IP.
    #[arg(long = "trusted-proxy-ip", requires = "external_origin")]
    pub(crate) trusted_proxy_ips: Vec<std::net::IpAddr>,
    #[arg(long)]
    pub(crate) web_dir: Option<PathBuf>,
    #[arg(long, default_value_t = 1_073_741_824)]
    pub(crate) max_upload_bytes: u64,
    /// HTTP spool quota only, including retained generations and metadata allowance.
    #[arg(long, default_value_t = 107_374_182_400)]
    pub(crate) http_spool_bytes_per_account: u64,
    #[arg(long, default_value_t = 536_870_912_000)]
    pub(crate) http_spool_bytes_total: u64,
    /// Keep this filesystem headroom outside new upload reservations.
    #[arg(long, default_value_t = 1_073_741_824)]
    pub(crate) http_spool_recovery_reserve_bytes: u64,
    /// Independent capacity for each HTTP upload/read pool.
    #[arg(long, default_value_t = 2)]
    pub(crate) http_transfer_slots: usize,
    #[arg(long, default_value_t = 60)]
    pub(crate) http_transfer_idle_seconds: u64,
    #[arg(long, default_value_t = 3600)]
    pub(crate) http_transfer_total_seconds: u64,
    #[arg(long, requires_all = ["smb_peer_uid", "smb_user"])]
    pub(crate) smb_socket: Option<PathBuf>,
    #[arg(long, requires = "smb_socket")]
    pub(crate) smb_peer_uid: Option<u32>,
    /// Initial authenticated Samba account; saved selection takes precedence on restart.
    #[arg(long, requires = "smb_socket", value_parser = parse_smb_user)]
    pub(crate) smb_user: Option<String>,
    /// Additional existing Samba accounts provisioned in passdb and every share valid users.
    #[arg(long = "smb-allowed-user", requires = "smb_socket", value_parser = parse_smb_user)]
    pub(crate) smb_allowed_users: Vec<String>,
    /// SMB admitted private-copy extents, including retained disconnected descriptors.
    #[arg(long, default_value_t = 107_374_182_400)]
    pub(crate) smb_spool_bytes_per_account: u64,
    #[arg(long, default_value_t = 536_870_912_000)]
    pub(crate) smb_spool_bytes_total: u64,
    #[arg(long, default_value_t = 1_073_741_824)]
    pub(crate) smb_spool_recovery_reserve_bytes: u64,
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

fn main() -> std::process::ExitCode {
    let mut args = Args::parse();
    // This branch must precede logging, host startup admission, Tokio and listeners.
    if let Some(command) = args.command.take() {
        return match offline::run(command) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("zxs: offline maintenance failed: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    let log_filter = match log_filter_from_env() {
        Ok(filter) => filter,
        Err(error) => {
            // Sources may retain operator input; do not expose it through Debug or
            // a source-chain formatter at the process diagnostic boundary.
            eprintln!("zxs: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    tracing::collector::fmt()
        .with_env_filter(log_filter)
        .with_writer(std::io::stderr)
        .log_internal_errors(false)
        .init();
    // Keep this owner outside #[tokio::main]: its return includes runtime
    // destruction and the join of blocking work, including startup failures.
    let lease = match platform_io::service_lease::ServiceLease::host() {
        Ok(lease) => lease,
        Err(error) => {
            eprintln!(
                "zxs: service exclusion unavailable: {error}; use the root helper host preview/prepare when /zxs is absent. Stop and drain a participating daemon/helper before retrying. No runtime or pool was opened."
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let prepared = match prepare_normal_start(&mut args) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("zxs: {error}; no runtime, listener or initialization was started");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut result = run_with_runtime(args, &prepared);
    if let Err(error) = prepared.recheck() {
        eprintln!(
            "zxs: prepared system path changed: {error}; preserve state and inspect before restarting"
        );
        result = std::process::ExitCode::FAILURE;
    }
    drop(prepared);
    if let Err(error) = lease.recheck() {
        eprintln!("zxs: service lease root changed: {error}; inspect the host before restarting");
        result = std::process::ExitCode::FAILURE;
    }
    drop(lease);
    // This point is after Tokio has joined its blocking pool, not merely after
    // the HTTP connection futures stopped. Do not log completion earlier.
    tracing::info!("daemon runtime stopped; admitted blocking work joined");
    result
}

/// Resolve only host configuration and admit existing preparation before Tokio
/// can spawn management work or before any listener/spool directory is created.
fn prepare_normal_start(
    args: &mut Args,
) -> Result<storage_server::startup_admission::PreparedService> {
    let current = std::env::current_dir()?;
    if !args.root.is_absolute() {
        args.root = current.join(&args.root);
    }
    let management = args
        .management_root
        .take()
        .unwrap_or_else(|| args.root.join("management"));
    let management = if management.is_absolute() {
        management
    } else {
        current.join(management)
    };
    let prepared =
        storage_server::startup_admission::PreparedService::open(&args.root, &management)?;
    args.management_root = Some(management);
    prepared.recheck()?;
    Ok(prepared)
}

#[tokio::main]
async fn run_with_runtime(
    args: Args,
    prepared: &storage_server::startup_admission::PreparedService,
) -> std::process::ExitCode {
    // Let runtime teardown join admitted blocking work, including on errors.
    // Force-exiting here would skip its durability and recovery cleanup.
    let shutdown = match storage_server::ShutdownSignal::install() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            eprintln!("zxs: cannot install shutdown handlers: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match run(args, shutdown, prepared).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // Report the error before runtime teardown, which may need to wait.
            eprintln!("zxs: {error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(
    args: Args,
    shutdown: storage_server::ShutdownSignal,
    prepared: &storage_server::startup_admission::PreparedService,
) -> Result<()> {
    let retained_workspace_limits = args.workspace_limits()?;
    let scrub_policy = storage_server::integrity_scrub::IntegrityScrubPolicy::periodic(
        args.scrub_interval_seconds,
        args.scrub_max_age_seconds,
        args.scrub_bytes_per_second,
    )?;
    let Args {
        root,
        management_root,
        storage_discovery_root,
        setup_disks,
        setup_configs,
        allow_development_setup,
        listen,
        external_origin,
        trusted_proxy_ips,
        web_dir,
        max_upload_bytes,
        http_spool_bytes_per_account,
        http_spool_bytes_total,
        http_spool_recovery_reserve_bytes,
        http_transfer_slots,
        http_transfer_idle_seconds,
        http_transfer_total_seconds,
        smb_socket,
        smb_peer_uid,
        smb_user,
        smb_allowed_users,
        smb_spool_bytes_per_account,
        smb_spool_bytes_total,
        smb_spool_recovery_reserve_bytes,
        ..
    } = args;
    // Resolve operator-supplied relative paths without touching unavailable data members.
    let current = std::env::current_dir()?;
    let root = if root.is_absolute() {
        root
    } else {
        current.join(root)
    };
    let management_root = management_root.unwrap_or_else(|| root.join("management"));
    let management_root = if management_root.is_absolute() {
        management_root
    } else {
        current.join(management_root)
    };
    let mut config = ServerConfig::new(&management_root);
    config.listen = listen;
    config.external_origin = external_origin;
    config.trusted_proxy_ips = trusted_proxy_ips;
    config.web_dir = web_dir;
    config.max_upload_bytes = max_upload_bytes;
    config.http_spool_limits = storage_server::SpoolLimits {
        bytes_per_account: http_spool_bytes_per_account,
        bytes_total: http_spool_bytes_total,
        recovery_reserve_bytes: http_spool_recovery_reserve_bytes,
    };
    config.http_transfer_limits = storage_server::TransferLimits {
        slots: http_transfer_slots,
        idle_timeout: std::time::Duration::from_secs(http_transfer_idle_seconds),
        total_timeout: std::time::Duration::from_secs(http_transfer_total_seconds),
    };
    if let Some(socket) = smb_socket {
        let allowed_uid = smb_peer_uid.ok_or("--smb-peer-uid is required with --smb-socket")?;
        let username = smb_user.ok_or("--smb-user is required with --smb-socket")?;
        config.smb = Some(storage_server::SmbConfig {
            spool_dir: management_root.join("smb-spool"),
            spool_limits: storage_server::SpoolLimits {
                bytes_per_account: smb_spool_bytes_per_account,
                bytes_total: smb_spool_bytes_total,
                recovery_reserve_bytes: smb_spool_recovery_reserve_bytes,
            },
            socket,
            allowed_uid,
            username,
            additional_users: smb_allowed_users,
        });
    }
    let (backend, auth) = storage_server::management::open_prepared_service_with_options(
        prepared,
        storage_server::management::ManagementOptions {
            scrub_policy,
            setup_disks: parse_setup_map(setup_disks, "--setup-disk")?,
            discovery_root: storage_discovery_root,
            setup_configs: parse_setup_map(setup_configs, "--setup-config")?,
            allow_development_setup,
            retained_workspace_limits,
        },
    )?;
    storage_server::serve_with_shutdown(backend, auth, config, shutdown).await?;
    Ok(())
}

fn parse_smb_user(value: &str) -> std::io::Result<String> {
    smb_protocol::protocol::validate_username(value).map(|()| value.to_owned())
}

fn parse_setup_map(
    values: Vec<String>,
    flag: &str,
) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let mut paths = std::collections::BTreeMap::new();
    for mapping in values {
        let (id, path) = mapping
            .split_once('=')
            .ok_or_else(|| format!("{flag} must be ID=/absolute/path"))?;
        if !storage_server::setup_recovery::valid_local_setup_id(id) {
            return Err(
                format!("{flag} ID must contain 1–64 ASCII letters, digits, '_' or '-'").into(),
            );
        }
        let path = PathBuf::from(path);
        if !path.is_absolute() || paths.insert(id.to_owned(), path).is_some() {
            return Err(
                format!("{flag} IDs must be nonempty and unique, with absolute paths").into(),
            );
        }
    }
    Ok(paths)
}

#[cfg(test)]
#[path = "../../../test/server/unit/main_tests.rs"]
mod tests;
