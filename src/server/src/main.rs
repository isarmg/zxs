use command_line::Parser;
use std::{net::SocketAddr, path::PathBuf};
use storage_server::ServerConfig;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Direct service entry. Daily administration and initialization belong in Web.
#[derive(Parser)]
#[command(name = "zxs", version, about = "ZXS storage and Web management daemon")]
struct Args {
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
    #[arg(long, default_value = "./zxs-data")]
    root: PathBuf,
    /// System-disk management state; defaults to ROOT/management, never a data member.
    #[arg(long)]
    management_root: Option<PathBuf>,
    /// Locally approved existing disk root, repeated as opaque ID=/absolute/path.
    #[arg(long = "setup-disk")]
    setup_disks: Vec<String>,
    /// Locally reviewed private configuration preset, repeated as ID=/absolute/file.
    #[arg(long = "setup-config")]
    setup_configs: Vec<String>,
    /// Test-only: allow directory-backed development pool setup, never physical disk validation.
    #[arg(long)]
    allow_development_setup: bool,

    #[arg(long, default_value = "127.0.0.1:7400")]
    pub(crate) listen: SocketAddr,
    #[arg(long)]
    pub(crate) external_origin: Option<String>,
    /// Trust this exact loopback proxy peer; it must overwrite X-Forwarded-For with one IP.
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
    /// Exact authenticated Samba account bound to the sole current ZXS account.
    #[arg(long, requires = "smb_socket", value_parser = parse_smb_user)]
    pub(crate) smb_user: Option<String>,
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
    let args = Args::parse();
    tracing::collector::fmt()
        .with_env_filter(
            tracing::collector::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let result = run_with_runtime(args);
    // This point is after Tokio has joined its blocking pool, not merely after
    // the HTTP connection futures stopped. Do not log completion earlier.
    tracing::info!("daemon runtime stopped; admitted blocking work joined");
    result
}

#[tokio::main]
async fn run_with_runtime(args: Args) -> std::process::ExitCode {
    // Let runtime teardown join admitted blocking work, including on errors.
    // Force-exiting here would skip its durability and recovery cleanup.
    let shutdown = match storage_server::ShutdownSignal::install() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            eprintln!("zxs: cannot install shutdown handlers: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match run(args, shutdown).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // Report the error before runtime teardown, which may need to wait.
            eprintln!("zxs: {error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args, shutdown: storage_server::ShutdownSignal) -> Result<()> {
    let retained_workspace_limits = args.workspace_limits()?;
    let Args {
        root,
        management_root,
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
    let mut config = ServerConfig::loopback(&management_root);
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
        });
    }
    let (backend, auth) = storage_server::management::open_service_with_options(
        &root,
        &management_root,
        storage_server::management::ManagementOptions {
            setup_disks: parse_setup_map(setup_disks, "--setup-disk")?,
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
