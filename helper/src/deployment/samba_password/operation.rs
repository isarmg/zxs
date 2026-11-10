//! Shared command prerequisites and one-shot intent, not passdb authority.
use super::{MAX_RECORD_BYTES, state};
use crate::{
    Result,
    account::Snapshot as AccountSnapshot,
    deployment::{Input, install},
    private_io::{BundleDestination, BundlePublication},
    system_program::{Executable, Program, Snapshot as ProgramSnapshot},
};
use data_serialization::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) struct Context {
    pub(super) configuration: install::Prepared,
    testparm: Executable,
    pub(super) pdbedit: Executable,
    pub(super) smbpasswd: Executable,
}
impl Context {
    pub(super) fn open(input: Input) -> Result<(Self, AccountSnapshot)> {
        let configuration = install::existing(input)?;
        let account = configuration.samba_account()?.snapshot();
        let testparm = Executable::open(Program::Testparm)?;
        let pdbedit = Executable::open(Program::Pdbedit)?;
        let smbpasswd = Executable::open(Program::Smbpasswd)?;
        state::require_local_configuration(&testparm)?;
        configuration.require_existing()?;
        Ok((
            Self {
                configuration,
                testparm,
                pdbedit,
                smbpasswd,
            },
            account,
        ))
    }
    pub(super) fn recheck(&self) -> Result<()> {
        self.configuration.require_existing()?;
        self.testparm.recheck()?;
        self.pdbedit.recheck()?;
        self.smbpasswd.recheck()
    }
    pub(super) fn programs(&self) -> Vec<ProgramSnapshot> {
        vec![
            self.testparm.snapshot(),
            self.pdbedit.snapshot(),
            self.smbpasswd.snapshot(),
        ]
    }
}

// Each operation keeps its own format and database proof type. Field order is
// part of the existing compact digest and pretty retained-plan encoding.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Plan<Format, Database> {
    pub(super) format: Format,
    pub(super) operation_id: String,
    pub(super) installed_configuration_digest: String,
    pub(super) account: AccountSnapshot,
    pub(super) database: Database,
    pub(super) programs: Vec<ProgramSnapshot>,
}
pub(super) fn digest(plan: &impl Serialize) -> Result<String> {
    Ok(platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(
        plan,
    )?)))
}

#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub(crate) enum Status<Receipt> {
    NoCompletePlanReadable {
        request_digest: String,
    },
    OutcomeUnknownDoNotReplay {
        request_digest: String,
        account: String,
    },
    CommandCompletedHistoricalReceipt {
        receipt: Box<Receipt>,
        current_password_or_service_state_verified: bool,
    },
}

/// Only a newly created durable intent admits an external command. A resumed
/// bundle, including a previously empty staging directory, never admits replay.
pub(super) fn admit(host: &BundleDestination, name: &str, plan: &impl Serialize) -> Result<()> {
    if host
        .read_operation(name, "plan.json", MAX_RECORD_BYTES)?
        .is_some()
    {
        return Err(format!("Samba operation {name} was already admitted; query its retained outcome, never automatically repeat it").into());
    }
    let bytes = json_codec::to_vec_pretty(plan)?;
    if !matches!(
        host.publish(name, &[("plan.json", bytes.as_slice())])?,
        BundlePublication::Created
    ) {
        return Err(format!(
            "Samba operation staging {name} already existed; command was not repeated"
        )
        .into());
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct Pending {
    pub(super) operation: &'static str,
    pub(super) request_digest: String,
    pub(super) source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Samba {} {} has a retained intent and may have changed the native account or password; query this exact request, preserve all evidence, never automatically replay or restore an old database",
            self.operation, self.request_digest
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/samba_operation_tests.rs"]
mod tests;
