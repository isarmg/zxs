//! Held deployment resources shared by configuration installation and handoff.
use super::{Input, model, render};
use crate::{
    Result,
    account::{LocalAccount, LocalGroup},
    installation::InstalledProgram,
    private_io::ConfigurationFiles,
};

pub(super) struct Context {
    pub account: LocalAccount,
    pub group: LocalGroup,
    pub samba_account: Option<LocalAccount>,
    pub program: InstalledProgram,
    pub configuration: ConfigurationFiles,
}
impl Context {
    pub fn open(
        input: &Input,
    ) -> Result<(
        Self,
        render::ConfigurationFile,
        Option<render::ConfigurationFile>,
    )> {
        let settings = model::Settings::try_from(input.clone())?;
        let account = LocalAccount::resolve(settings.user.clone())?;
        let group = LocalGroup::resolve(settings.group.clone())?;
        if group.gid() != account.owner().1 {
            return Err("service group must resolve to the selected account's primary GID; supplementary or mismatched group overrides are not supported".into());
        }
        let samba_account = match &settings.smb {
            Some(smb) => {
                let resolved = LocalAccount::resolve(smb.account.clone())?;
                if resolved.owner().0 == account.owner().0 {
                    return Err("Samba authentication UID must be distinct from the private ZXS service UID".into());
                }
                Some(resolved)
            }
            None => None,
        };
        let program = InstalledProgram::open()?;
        let configuration = ConfigurationFiles::open(settings.smb.is_some())?;
        let (unit, samba) = render::files(&settings);
        Ok((
            Self {
                account,
                group,
                samba_account,
                program,
                configuration,
            },
            unit,
            samba,
        ))
    }
    pub fn recheck(&self) -> Result<()> {
        self.program.recheck()?;
        self.account.recheck()?;
        self.group.recheck()?;
        if let Some(account) = &self.samba_account {
            account.recheck()?;
        }
        self.configuration.recheck()
    }
    pub fn require_existing(
        &self,
        unit: &render::ConfigurationFile,
        samba: Option<&render::ConfigurationFile>,
    ) -> Result<()> {
        if self
            .configuration
            .inspect(
                unit.content.as_bytes(),
                samba.map(|file| file.content.as_bytes()),
            )?
            .iter()
            .any(|present| !present)
        {
            return Err("exact installed configuration is required; missing files are not installed by this operation".into());
        }
        Ok(())
    }
}
