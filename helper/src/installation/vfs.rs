//! Explicit no-overwrite publication of the sealed same-source VFS module.
//! Native packages, existing libraries, services and passwords are not modified.
use super::{InstalledProgram, PayloadMetadata, vfs_payload};
use crate::{
    Result,
    deployment::activation,
    private_io::{BundleDestination, VfsDestination},
    system_program::{Executable, Program, Snapshot},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

const RECORD_LIMIT: usize = 64 * 1024;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Original {
    pub request_digest: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}
#[derive(Serialize)]
struct Plan {
    format: &'static str,
    destination: &'static str,
    new_file_mode: &'static str,
    host_device: String,
    host_inode: String,
    parent_device: String,
    parent_inode: String,
    program: PayloadMetadata,
    module: vfs_payload::Metadata,
    daemon: Snapshot,
    systemctl: Snapshot,
}
#[derive(Serialize)]
pub(crate) struct Observation {
    state: &'static str,
    request_digest: String,
    intent_readable: bool,
    completion_readable: bool,
    current_module_matches: bool,
    receipt: Option<Value>,
    service_state_verified: bool,
    native_module_loaded: bool,
    automatic_retry: bool,
    complete_installation_ready: bool,
}
struct Prepared {
    host: BundleDestination,
    program: InstalledProgram,
    destination: VfsDestination,
    payload: vfs_payload::Verified,
    daemon: Executable,
    systemctl: Executable,
    plan: Plan,
    digest: String,
}
impl Prepared {
    fn open() -> Result<Self> {
        if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
            return Err("VFS installation requires the manually run real/effective root helper".into());
        }
        let payload = vfs_payload::required()?;
        let host = BundleDestination::open(Path::new("/zxs"))?;
        let program = InstalledProgram::open()?;
        let daemon = activation::samba_module_host()?;
        let systemctl = Executable::open(Program::Systemctl)?;
        let destination = VfsDestination::open()?;
        destination.matches_or_absent(payload.bytes)?;
        let parent = destination.parent_identity();
        let plan = Plan {
            format: "zxs-helper-sealed-vfs-installation",
            destination: vfs_payload::MODULE_PATH,
            new_file_mode: "0555",
            host_device: host.identity().device.to_string(),
            host_inode: host.identity().inode.to_string(),
            parent_device: parent.device.to_string(),
            parent_inode: parent.inode.to_string(),
            program: program.metadata(),
            module: payload.metadata(),
            daemon: daemon.snapshot(),
            systemctl: systemctl.snapshot(),
        };
        let digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
        let result = Self {
            host,
            program,
            destination,
            payload,
            daemon,
            systemctl,
            plan,
            digest,
        };
        result.recheck()?;
        Ok(result)
    }
    fn recheck(&self) -> Result<()> {
        self.host.recheck()?;
        self.program.recheck()?;
        self.daemon.recheck()?;
        self.systemctl.recheck()?;
        self.destination.recheck()
    }
    fn require_digest(&self, expected: &str) -> Result<()> {
        if self.digest != expected {
            return Err("VFS source, installed program, fixed Samba tools or directory bindings changed; the original request was not replaced".into());
        }
        Ok(())
    }
    fn name(&self, suffix: &str) -> String {
        format!(".zxs-helper-vfs-{suffix}-{}", self.digest)
    }
    fn receipt(&self) -> Value {
        json!({"format":"zxs-helper-vfs-installed","request_digest":self.digest,
            "destination":vfs_payload::MODULE_PATH,"module":self.payload.metadata(),
            "durable_exact_bytes":true,"different_module_replaced":false,
            "packages_installed":false,"service_reloaded":false,"service_started":false,
            "passwords_changed":false,"native_module_loaded":false,
            "network_smb_login_verified":false,"complete_installation_ready":false})
    }
    fn records(&self) -> Result<(bool, bool)> {
        let plan = json_codec::to_vec_pretty(&self.plan)?;
        let receipt = json_codec::to_vec_pretty(&self.receipt())?;
        let original = self.host.read_operation(&self.name("intent"), "plan.json", RECORD_LIMIT)?;
        if original.as_ref().is_some_and(|bytes| bytes != &plan) {
            return Err("retained VFS intent differs from this exact original request; preserve it".into());
        }
        let complete = self.host.read_operation(
            &self.name("installed"), "receipt.json", RECORD_LIMIT,
        )?;
        if complete.as_ref().is_some_and(|bytes| bytes != &receipt)
            || (complete.is_some() && original.is_none())
        {
            return Err("retained VFS completion is inconsistent or lacks its original intent".into());
        }
        Ok((original.is_some(), complete.is_some()))
    }
    fn observation(&self) -> Result<Observation> {
        self.recheck()?;
        let (intent, complete) = self.records()?;
        let exact = self.destination.matches_or_absent(self.payload.bytes)?;
        self.recheck()?;
        let state = if complete {
            if exact {
                "completed_historical_receipt_and_current_bytes_match"
            } else {
                "completed_module_missing_do_not_reinstall"
            }
        } else if intent {
            "original_installation_pending"
        } else {
            "no_original_intent"
        };
        Ok(Observation {
            state,
            request_digest: self.digest.clone(),
            intent_readable: intent,
            completion_readable: complete,
            current_module_matches: exact,
            receipt: complete.then(|| self.receipt()),
            service_state_verified: false,
            native_module_loaded: false,
            automatic_retry: false,
            complete_installation_ready: false,
        })
    }
}
pub(crate) fn preview() -> Result<Value> {
    let prepared = Prepared::open()?;
    activation::require_samba_stopped_for_vfs(&prepared.systemctl)?;
    let current = prepared.observation()?;
    Ok(json!({"request_digest":prepared.digest,"plan":prepared.plan,"current":current,
        "source":"sealed matching helper release; no runtime path, URL or upload",
        "will_replace_existing_bytes":false,"will_install_os_packages":false,
        "will_stop_or_start_services":false,"all_external_writers_excluded":false,
        "confirmation":"install-sealed-samba-vfs-only"}))
}
pub(crate) fn status(input: Original) -> Result<Observation> {
    let prepared = Prepared::open()?;
    prepared.require_digest(&input.request_digest)?;
    prepared.observation()
}
#[derive(Debug)]
struct Pending {
    digest: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VFS installation {} may have published exact module bytes; preserve all material, query this original request and explicitly resume only it; no rollback or service activation is claimed", self.digest)
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
pub(crate) fn commit(input: Commit) -> Result<Value> {
    if input.confirmation != "install-sealed-samba-vfs-only"
        || !input.external_administration_stopped
    {
        return Err("explicit sealed VFS-only installation and continuously stopped external Samba/package/service administrators are required".into());
    }
    let prepared = Prepared::open()?;
    prepared.require_digest(&input.request_digest)?;
    activation::require_samba_stopped_for_vfs(&prepared.systemctl)?;
    publish(&prepared).map_err(|source| {
        Box::new(Pending {
            digest: prepared.digest.clone(),
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}
fn publish(prepared: &Prepared) -> Result<Value> {
    prepared.recheck()?;
    let (_, complete) = prepared.records()?;
    let matches = prepared.destination.matches_or_absent(prepared.payload.bytes)?;
    if complete && !matches {
        return Err("a completed VFS module is missing; installation does not recreate it or discard the original completion".into());
    }
    if !complete {
        let plan = json_codec::to_vec_pretty(&prepared.plan)?;
        prepared.host.publish(&prepared.name("intent"), &[("plan.json", &plan)])?;
    }
    prepared.recheck()?;
    activation::require_samba_stopped_for_vfs(&prepared.systemctl)?;
    let created = prepared.destination.install(prepared.payload.bytes)?;
    prepared.recheck()?;
    activation::require_samba_stopped_for_vfs(&prepared.systemctl)?;
    let receipt = json_codec::to_vec_pretty(&prepared.receipt())?;
    prepared.host.publish(
        &prepared.name("installed"), &[("receipt.json", &receipt)],
    )?;
    let current = prepared.observation()?;
    if !current.current_module_matches {
        return Err("VFS module changed after receipt publication; no current success is claimed".into());
    }
    Ok(json!({"state":"vfs_module_installed_not_activated","request_digest":prepared.digest,
        "created_in_this_attempt":created,"current":current,"receipt":prepared.receipt()}))
}
