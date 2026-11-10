//! Explicit service-manager handoff after installation.
//! Systemd and the daemon own their observable runtime states.
mod health;
mod paths;
mod samba;
mod units;

use super::{model, render};
use crate::{
    Result,
    account::{LocalAccount, LocalGroup},
    installation::InstalledProgram,
    private_io::{ConfigurationFiles, SYSTEMD_PATH},
    system_program::{Executable, Program},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use platform_io::service_lease::ServiceLease;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub deployment: super::Input,
    pub enable_on_boot: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Observation {
    pub input: Input,
    pub request_digest: String,
}

struct Prepared {
    input: Input,
    account: LocalAccount,
    group: LocalGroup,
    smb_account: Option<LocalAccount>,
    program: InstalledProgram,
    paths: paths::Paths,
    configuration: ConfigurationFiles,
    unit: render::ConfigurationFile,
    smb_configuration: Option<render::ConfigurationFile>,
    systemctl: Executable,
    analyzer: Executable,
    samba: Option<samba::Samba>,
    digest: String,
}
impl Prepared {
    fn open(input: Input) -> Result<Self> {
        if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
            return Err(
                "service activation requires the manually run real/effective root helper".into(),
            );
        }
        let settings = model::Settings::try_from(input.deployment.clone())?;
        let account = LocalAccount::resolve(settings.user.clone())?;
        let group = LocalGroup::resolve(settings.group.clone())?;
        if group.gid() != account.owner().1 {
            return Err("selected service group is not the account's primary group".into());
        }
        let smb_account = settings
            .smb
            .as_ref()
            .map(|smb| LocalAccount::resolve(smb.account.clone()))
            .transpose()?;
        if smb_account
            .as_ref()
            .is_some_and(|smb| smb.owner().0 == account.owner().0)
        {
            return Err(
                "Samba authentication and private service accounts must be distinct".into(),
            );
        }
        let program = InstalledProgram::open()?;
        let paths = paths::Paths::open(account.owner(), settings.smb.is_some())?;
        let configuration = ConfigurationFiles::open(settings.smb.is_some())?;
        let (unit, smb_configuration) = render::files(&settings);
        if configuration
            .inspect(
                unit.content.as_bytes(),
                smb_configuration
                    .as_ref()
                    .map(|value| value.content.as_bytes()),
            )?
            .iter()
            .any(|exists| !exists)
        {
            return Err("install the exact existing deployment configuration first; activation does not replace or repair it".into());
        }
        let systemctl = Executable::open(Program::Systemctl)?;
        let analyzer = Executable::open(Program::SystemdAnalyze)?;
        let samba = match (&settings.smb, &smb_account) {
            (Some(settings), Some(account)) => {
                Some(samba::Samba::open(account, settings.peer_uid)?)
            }
            (None, None) => None,
            _ => return Err("Samba activation context is incomplete".into()),
        };
        // Current runtime state is intentionally not in the digest. Start and
        // enable are separately observable, idempotent service-manager actions.
        let plan = json!({"format":"zxs-explicit-service-activation","revision":1,
            "input":input,"host_device":paths.identity().device.to_string(),
            "host_inode":paths.identity().inode.to_string(),"payload":program.metadata(),
            "service_account":account.snapshot(),"service_group":group.snapshot(),
            "samba_account":smb_account.as_ref().map(LocalAccount::snapshot),
            "configuration_parents":configuration.parents(),"unit":unit,
            "samba_configuration":smb_configuration,"samba_unit":samba.as_ref().map(|_| units::SMB_BYTES),
            "directories":paths.snapshot(),"systemctl":systemctl.snapshot(),
            "systemd_analyze":analyzer.snapshot(),"samba":samba.as_ref().map(samba::Samba::snapshot)});
        let digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
        let result = Self {
            input,
            account,
            group,
            smb_account,
            program,
            paths,
            configuration,
            unit,
            smb_configuration,
            systemctl,
            analyzer,
            samba,
            digest,
        };
        result.recheck()?;
        Ok(result)
    }
    fn recheck(&self) -> Result<()> {
        self.account.recheck()?;
        self.group.recheck()?;
        self.program.recheck()?;
        self.paths.recheck()?;
        self.configuration.recheck()?;
        if let Some(account) = &self.smb_account {
            account.recheck()?;
        }
        if self
            .configuration
            .inspect(
                self.unit.content.as_bytes(),
                self.smb_configuration
                    .as_ref()
                    .map(|value| value.content.as_bytes()),
            )?
            .iter()
            .any(|exists| !exists)
        {
            return Err(
                "the exact installed configuration disappeared; it was not recreated".into(),
            );
        }
        self.systemctl.recheck()?;
        self.analyzer.recheck()?;
        if let Some(samba) = &self.samba {
            samba.recheck()?;
        }
        Ok(())
    }
    fn require_digest(&self, expected: &str) -> Result<()> {
        if self.digest != expected {
            return Err("activation inputs, program/module bytes, accounts or directory bindings changed since preview; inspect current state before a new explicit approval".into());
        }
        Ok(())
    }
    fn zxs_command(&self) -> Result<&str> {
        self.unit
            .content
            .lines()
            .find_map(|line| line.strip_prefix("ExecStart="))
            .ok_or_else(|| "generated ZXS unit has no start command".into())
    }
    fn require_zxs(&self, unit: &units::Unit) -> Result<()> {
        unit.require_loaded(
            SYSTEMD_PATH,
            self.zxs_command()?,
            &self.input.deployment.service_user,
            &self.input.deployment.service_group,
        )
    }
    fn require_smb(&self, unit: &units::Unit) -> Result<()> {
        unit.require_loaded(
            units::SMB_PATH,
            "/usr/sbin/smbd --foreground --no-process-group --configfile=/etc/samba/zxs.conf",
            "",
            "",
        )
    }
    fn observe(&self) -> Result<Value> {
        self.recheck()?;
        let zxs = units::read(&self.systemctl, units::ZXS)?;
        let zxs_loaded = self.require_zxs(&zxs).is_ok();
        let zxs_active = zxs_loaded && zxs.active()?;
        let management = if zxs_active {
            health::management(self.input.deployment.listen)
                .map(|value| json!({"observed":value}))
                .unwrap_or_else(|error| json!({"error":crate::diagnostic_chain(error.as_ref())}))
        } else {
            json!({"not_observed":"exact ZXS service is not running"})
        };
        let http_ready = zxs_active && health::ready(&management["observed"]);
        let smb = if self.samba.is_some() {
            let unit = units::read(&self.systemctl, units::SMB)?;
            let exact = self
                .configuration
                .inspect_smb_unit(units::SMB_BYTES.as_bytes())?
                && self.require_smb(&unit).is_ok();
            let active = exact && unit.active()?;
            let bridge = if http_ready && active {
                health::bridge_socket(self.account.owner().0)
                    .map(|value| json!({"observed":value}))
                    .unwrap_or_else(
                        |error| json!({"error":crate::diagnostic_chain(error.as_ref())}),
                    )
            } else {
                json!({"not_observed":"normal management or exact Samba service is not running"})
            };
            Some(json!({"unit":unit,"exact_loaded_configuration":exact,
                "active":active,"enabled":unit.enabled()?,"bridge":bridge}))
        } else {
            None
        };
        let smb_ready = smb.as_ref().is_none_or(|value| {
            value["active"] == true && value["bridge"]["observed"]["socket_published"] == true
        });
        let enabled = zxs.enabled()? && smb.as_ref().is_none_or(|value| value["enabled"] == true);
        let ready = http_ready && smb_ready && (!self.input.enable_on_boot || enabled);
        self.recheck()?;
        Ok(
            json!({"state":if ready {"services_active_and_management_ready"} else {"not_ready"},
            "request_digest":self.digest, "zxs":zxs,"exact_loaded_configuration":zxs_loaded,
            "management":management,"samba":smb,"autostart_enabled":enabled,"autostart_requested":self.input.enable_on_boot,
            "file_write_test_performed":false,"bridge_protocol_session_verified":false,"network_smb_password_login_verified":false,"samba_vfs_network_session_verified":false,
            "package_or_account_installation_performed":false,"data_initialized_by_activation":false,
            "automatic_retry":false}),
        )
    }
}
pub(crate) fn samba_module_host() -> Result<Executable> {
    samba::module_host()
}

pub(crate) fn require_samba_stopped_for_vfs(program: &Executable) -> Result<()> {
    for name in [
        units::SMB,
        "smbd.service",
        "samba.service",
        "samba-ad-dc.service",
        "nmbd.service",
    ] {
        units::read(program, name)?.require_stopped_for_vfs()?;
    }
    Ok(())
}

pub(crate) fn preview(input: Input) -> Result<Value> {
    let prepared = Prepared::open(input)?;
    let smb_unit_present = if prepared.samba.is_some() {
        Some(
            prepared
                .configuration
                .inspect_smb_unit(units::SMB_BYTES.as_bytes())?,
        )
    } else {
        None
    };
    let observation = prepared.observe()?;
    Ok(
        json!({"request_digest":prepared.digest,"input":prepared.input,"current":observation,
        "dedicated_samba_unit_present":smb_unit_present,
        "web_port":prepared.input.deployment.listen.port(),
        "commands":["systemd-analyze verify exact units","systemctl daemon-reload",
            "systemctl start zxs.service","wait for normal authentication/storage/Web readiness",
            "if selected: systemctl start zxs-smb.service and inspect the protected bridge socket",
            "only if explicitly selected and ready: systemctl enable these exact units"],
        "will_stop_existing_services":false,"will_replace_configuration":false,
        "will_initialize_data_or_change_passwords":false,"confirmation":"activate-installed-zxs-services"}),
    )
}
pub(crate) fn status(input: Observation) -> Result<Value> {
    let prepared = Prepared::open(input.input)?;
    prepared.require_digest(&input.request_digest)?;
    prepared.observe()
}
pub(crate) fn activate(input: Commit) -> Result<Value> {
    if input.confirmation != "activate-installed-zxs-services"
        || !input.external_administration_stopped
    {
        return Err("explicit activation confirmation and stopped external configuration/service administration are required; no service action was attempted".into());
    }
    // Establish lock order before opening the installation/configuration
    // parents. Read-only unit observation does not acquire a filesystem lock.
    let observer = Executable::open(Program::Systemctl)?;
    let active = units::read(&observer, units::ZXS)?.active()?;
    let lease = if active {
        None
    } else {
        Some(ServiceLease::host()?)
    };
    drop(observer);
    let prepared = Prepared::open(input.input)?;
    prepared.require_digest(&input.request_digest)?;
    let result = start(&prepared, lease);
    // systemctl may have queued a job before failing. Retain the original
    // request correlation and observe the current state for explicit recovery.
    match result {
        Ok(()) => prepared.observe(),
        Err(error) => Ok(json!({"state":"activation_incomplete_or_unknown",
            "request_digest":prepared.digest,"error":crate::diagnostic_chain(error.as_ref()),
            "current":prepared.observe().ok(),"automatic_retry":false,
            "instruction":"query this exact request; explicitly retry only after inspecting the current service state"})),
    }
}
fn start(prepared: &Prepared, lease: Option<ServiceLease>) -> Result<()> {
    let initial = units::read(&prepared.systemctl, units::ZXS)?;
    let active = initial.active()?;
    if active == lease.is_some() {
        return Err("ZXS changed activity during activation admission; inspect the current service, no new service command was sent".into());
    }
    if active {
        prepared.require_zxs(&initial)?;
    } else if !initial.absent()? && !initial.may_start()? {
        return Err("ZXS is already starting/stopping; observe the original service job instead of starting another".into());
    }
    let current_smb = units::read(&prepared.systemctl, units::SMB)?;
    if current_smb.active()? {
        if prepared.samba.is_none() {
            return Err("a dedicated Samba service is already active but was not selected; stop or select it explicitly".into());
        }
        prepared.require_smb(&current_smb)?;
    } else if !current_smb.absent()? && !current_smb.may_start()? {
        return Err(
            "dedicated Samba is already changing state; observe that job before activation".into(),
        );
    }
    if prepared.samba.is_some() {
        for name in [
            "smbd.service",
            "samba.service",
            "samba-ad-dc.service",
            "nmbd.service",
        ] {
            units::conflict(&prepared.systemctl, name)?;
        }
    }
    // An inactive daemon's host lease was acquired before the program and
    // configuration parents. An already active exact daemon retains its owner.
    if let Some(lease) = &lease {
        lease.recheck()?;
        if lease.identity() != prepared.paths.identity() {
            return Err("service handoff host identity changed".into());
        }
    }
    prepared.recheck()?;
    if prepared.samba.is_some() {
        if active
            && !prepared
                .configuration
                .inspect_smb_unit(units::SMB_BYTES.as_bytes())?
        {
            return Err("ZXS is already active but the dedicated Samba unit is missing; stop ZXS explicitly before installing that unit".into());
        }
        if !active {
            prepared
                .configuration
                .install_smb_unit(units::SMB_BYTES.as_bytes())?;
        }
    }
    let mut verify = vec!["verify", SYSTEMD_PATH];
    if prepared.samba.is_some() {
        verify.push(units::SMB_PATH);
    }
    prepared.analyzer.capture(&verify)?;
    prepared.recheck()?;
    prepared
        .systemctl
        .capture(&["--no-pager", "daemon-reload"])?;
    prepared.require_zxs(&units::read(&prepared.systemctl, units::ZXS)?)?;
    if prepared.samba.is_some() {
        prepared.require_smb(&units::read(&prepared.systemctl, units::SMB)?)?;
    }
    prepared.recheck()?;
    // Transfer the host lease to the daemon's normal startup path.
    drop(lease);
    if !active {
        prepared
            .systemctl
            .capture(&["--no-pager", "start", "--", units::ZXS])?;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let unit = units::read(&prepared.systemctl, units::ZXS)?;
        prepared.require_zxs(&unit)?;
        if unit.active()?
            && health::management(prepared.input.deployment.listen)
                .is_ok_and(|value| health::ready(&value))
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err("ZXS readiness wait expired before authentication, writable storage and Web assets were ready; service state was preserved".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    prepared.recheck()?;
    if prepared.input.deployment.smb.is_some() {
        let unit = units::read(&prepared.systemctl, units::SMB)?;
        prepared.require_smb(&unit)?;
        if !unit.active()? {
            if !unit.may_start()? {
                return Err("dedicated Samba is changing state; inspect its current job".into());
            }
            prepared
                .systemctl
                .capture(&["--no-pager", "start", "--", units::SMB])?;
        }
        let unit = units::read(&prepared.systemctl, units::SMB)?;
        prepared.require_smb(&unit)?;
        if !unit.active()? {
            return Err("dedicated Samba did not remain running".into());
        }
        health::bridge_socket(prepared.account.owner().0)?;
    }
    if prepared.input.enable_on_boot {
        let mut args = vec!["--no-pager", "enable", "--", units::ZXS];
        if prepared.samba.is_some() {
            args.push(units::SMB);
        }
        prepared.systemctl.capture(&args)?;
    }
    Ok(())
}
