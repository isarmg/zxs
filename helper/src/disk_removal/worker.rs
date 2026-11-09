//! Native Deregister worker, including the last metadata voter. Status and
//! completion never re-open a retired disk merely to see whether it exists.
use super::*;
use platform_io::service_lease::ServiceLease;
use storage_engine::{
    storage_lifecycle::{ExitCommit, ExitRequest},
    storage_readmission::{RegistrationEntry, RegistrationSnapshot},
};
use storage_engine::Mode;
use storage_server::management::storage_lifecycle::ExclusiveExit;
use storage_server::{auth::AuthStore, startup_admission::PreparedService};

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Inventory {
    pool_id: String,
    authority: ExitAuthority,
    registrations: Vec<Registration>,
    separate_target_availability_probe_performed: bool,
    existing_engine_recovery_may_run: bool,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Registration {
    registration: RegistrationEntry,
    root_path: String,
    level: u32,
    upstream_slot_id: Option<String>,
}
pub(super) struct Target {
    pub(super) control: ExclusiveExit,
    auth: AuthStore,
    prepared: PreparedService,
    layout: Layout,
    lease: ServiceLease,
}
impl Target {
    pub(super) fn credential_revision(&self) -> Result<CredentialRevision> {
        Ok(self.auth.credential_revision()?)
    }
    pub(super) fn open(context: &layout::Context) -> Result<Self> {
        let lease = ServiceLease::host()?;
        let layout = Layout::open(context.account.name.as_str().into())?;
        if layout.context(lease.identity()) != *context {
            return Err(
                "deregistration worker rejected changed host/account/layout identity".into(),
            );
        }
        let prepared =
            PreparedService::open(Path::new(layout::STATE), Path::new(layout::MANAGEMENT))?;
        let auth = AuthStore::open_system_management_directory(layout.management()?)?;
        let control = ExclusiveExit::open(
            layout.state()?,
            layout.management()?,
            auth.reader(),
            auth.sole_identity()?,
        )?;
        if control.current_configuration()?.mode != Mode::Production {
            return Err("helper deregistration requires an existing production pool".into());
        }
        let target = Self {
            control,
            auth,
            prepared,
            layout,
            lease,
        };
        target.recheck()?;
        Ok(target)
    }
    pub(super) fn recheck(&self) -> Result<()> {
        self.lease.recheck()?;
        self.layout.recheck()?;
        self.prepared.recheck()?;
        Ok(())
    }
}
fn original(target: &Target, plan: &Plan) -> Result<()> {
    target.recheck()?;
    supported(&plan.exit)?;
    if target.layout.context(target.lease.identity()) != plan.context
        || target.auth.credential_revision()? != plan.credential
    {
        return Err("original deregistration no longer matches current roots/account or primary credentials; no new authority was invented".into());
    }
    Ok(())
}
fn inventory(control: &ExclusiveExit) -> Result<Inventory> {
    let config = control.current_configuration()?;
    let (authority, registrations) = match control.registrations()? {
        RegistrationSnapshot::ActiveMembership { registrations } => {
            (ExitAuthority::ActiveMembership, registrations)
        }
        RegistrationSnapshot::TerminalManagement { registrations } => {
            (ExitAuthority::TerminalManagement, registrations)
        }
        RegistrationSnapshot::Unadmitted => {
            return Err("no admitted registration authority; this is not an empty pool".into());
        }
    };
    let mut entries = Vec::with_capacity(registrations.len());
    for registration in registrations {
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.disk_id == registration.disk_id)
            .ok_or(
                "current registration has no matching authenticated definition; no root was opened",
            )?;
        entries.push(Registration {
            registration,
            root_path: disk
                .root
                .to_str()
                .ok_or("registered root is not UTF-8")?
                .into(),
            level: disk.level,
            upstream_slot_id: disk.upstream_slot_id.clone(),
        });
    }
    Ok(Inventory {
        pool_id: config.pool_id,
        authority,
        registrations: entries,
        separate_target_availability_probe_performed: false,
        existing_engine_recovery_may_run: true,
    })
}
pub(super) fn execute(request: WorkerRequest) -> Result<Output> {
    let mut target = Target::open(&request.context)?;
    match request.action {
        Action::TerminalPreview { request: selection } => {
            let retirement = target.control.preview_terminal_retirement(selection)?;
            target.recheck()?;
            Ok(Output::TerminalPreview {
                plan: terminal_action::plan(
                    request.context,
                    target.auth.credential_revision()?,
                    retirement,
                ),
            })
        }
        Action::TerminalOriginal { plan } => {
            terminal_original(&target, &plan)?;
            let result = target
                .control
                .terminal_retirement_result(&plan.retirement)?;
            target.recheck()?;
            Ok(Output::TerminalResult { result })
        }
        Action::TerminalCommit { plan } => {
            terminal_original(&target, &plan)?;
            let result = target
                .control
                .complete_terminal_retirement(&plan.retirement)?;
            target.recheck()?;
            Ok(Output::TerminalResult {
                result: Some(result),
            })
        }
        Action::InspectTerminal {
            slot_id,
            expected_disk_id,
        } => {
            let inspection = terminal::capture(&target.control, &slot_id, &expected_disk_id)?;
            target.recheck()?;
            Ok(Output::TerminalInspection { inspection })
        }
        Action::Inventory => {
            let inventory = inventory(&target.control)?;
            target.recheck()?;
            Ok(Output::Inventory { inventory })
        }
        Action::Preview {
            operation_id,
            slot_id,
            expected_disk_id,
        } => {
            let exit = target.control.preview(ExitRequest {
                operation_id,
                slot_id,
                expected_disk_id,
                action: ExitAction::Deregister,
            })?;
            supported(&exit)?;
            target.recheck()?;
            Ok(Output::Preview {
                plan: Plan {
                    format: Format::Deregistration,
                    context: request.context,
                    credential: target.auth.credential_revision()?,
                    exit,
                },
            })
        }
        Action::Original { plan } => {
            original(&target, &plan)?;
            let status = match target.control.status(plan.exit.request.operation_id) {
                Ok(status) => {
                    if status.preview != plan.exit {
                        return Err(
                            "helper plan differs from the original native deregistration record"
                                .into(),
                        );
                    }
                    Some(status)
                }
                Err(error) if error.code == storage_engine::ErrorCode::OperationNotFound => {
                    if target.control.preview(plan.exit.request.clone())? != plan.exit {
                        return Err("unstarted deregistration no longer has its exact original published source boundary".into());
                    }
                    None
                }
                Err(error) => return Err(error.into()),
            };
            target.recheck()?;
            Ok(Output::Original { status })
        }
        Action::Commit { plan } => {
            original(&target, &plan)?;
            let status = target.control.commit(ExitCommit { preview: plan.exit })?;
            target.recheck()?;
            Ok(Output::Status { status })
        }
    }
}

fn terminal_original(target: &Target, plan: &terminal_action::Plan) -> Result<()> {
    target.recheck()?;
    if target.layout.context(target.lease.identity()) != plan.context
        || target.auth.credential_revision()? != plan.credential
    {
        return Err(
            "terminal retirement original roots/account/primary credentials changed".into(),
        );
    }
    Ok(())
}
