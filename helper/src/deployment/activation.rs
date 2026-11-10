//! Explicit service-manager handoff after installation.
//! Systemd and the daemon own their observable runtime states.
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
use sha2::{Digest, Sha256};

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
        let plan = json!({"format":"zxs-explicit-service-activation","revision":2,
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
        zxs.require_stopped()?;
        let smb = units::read(&self.systemctl, units::SMB)?;
        smb.require_stopped()?;
        Ok(
            json!({"state":"offline_ready_for_handoff", "request_digest":self.digest,
            "web_port":self.input.deployment.listen.port(), "zxs":zxs,"samba":smb,
            "autostart_requested":self.input.enable_on_boot,
            "services_started":false,"service_readiness_verified":false}),
        )
    }
}

pub(crate) struct Handoff {
    systemctl: Executable,
    samba: bool,
}
impl Handoff {
    pub(crate) fn execute(self) -> Result<()> {
        let mut args = vec!["--no-pager", "start", "--", units::ZXS];
        if self.samba {
            args.push(units::SMB);
        }
        self.systemctl.replace_process(&args)
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
        units::read(program, name)?.require_stopped()?;
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
        json!({"state":"offline_ready_for_handoff","request_digest":prepared.digest,"input":prepared.input,"current":observation,
        "dedicated_samba_unit_present":smb_unit_present,
        "web_port":prepared.input.deployment.listen.port(),
        "commands":["systemd-analyze verify exact units","systemctl daemon-reload",
            "if selected: systemctl enable the exact installed units without starting",
            "stop helper admission and wait for all requests and workers",
            "replace the exited helper with systemctl start for the exact installed units"],
        "will_stop_existing_services":false,"will_replace_configuration":false,
        "will_initialize_data_or_change_passwords":false,
        "service_readiness_verified":false,
        "confirmation":"exit-helper-and-start-installed-zxs-services"}),
    )
}
pub(crate) fn status(input: Observation) -> Result<Value> {
    let prepared = Prepared::open(input.input)?;
    prepared.require_digest(&input.request_digest)?;
    prepared.observe()
}
pub(crate) fn activate(input: Commit) -> Result<(Value, Handoff)> {
    if input.confirmation != "exit-helper-and-start-installed-zxs-services"
        || !input.external_administration_stopped
    {
        return Err("confirm helper exit and installed service startup after stopping external service administration".into());
    }
    let prepared = Prepared::open(input.input)?;
    prepared.require_digest(&input.request_digest)?;
    prepare_start(&prepared)?;
    let response = json!({"state":"helper_exit_then_service_start",
        "request_digest":prepared.digest,"web_port":prepared.input.deployment.listen.port(),
        "helper_shutdown_requested":true,"services_started":false,
        "service_readiness_verified":false,"autostart_requested":prepared.input.enable_on_boot});
    Ok((
        response,
        Handoff {
            systemctl: prepared.systemctl,
            samba: prepared.samba.is_some(),
        },
    ))
}
fn prepare_start(prepared: &Prepared) -> Result<()> {
    units::read(&prepared.systemctl, units::ZXS)?.require_stopped()?;
    units::read(&prepared.systemctl, units::SMB)?.require_stopped()?;
    if prepared.samba.is_some() {
        for name in [
            "smbd.service",
            "samba.service",
            "samba-ad-dc.service",
            "nmbd.service",
        ] {
            units::conflict(&prepared.systemctl, name)?;
        }
        prepared
            .configuration
            .install_smb_unit(units::SMB_BYTES.as_bytes())?;
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
    if prepared.input.enable_on_boot {
        let mut args = vec!["--no-pager", "enable", "--", units::ZXS];
        if prepared.samba.is_some() {
            args.push(units::SMB);
        }
        prepared.systemctl.capture(&args)?;
    }
    prepared.recheck()
}
