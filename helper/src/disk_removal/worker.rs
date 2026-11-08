//! Native active-Deregister worker. In particular, status and completion never
//! enumerate or re-open the retired disk merely to see whether it still exists.
use super::*;
use crate::managed_pool::ManagedPool;
use storage_contract::{
    storage_lifecycle::{ExitCommit, ExitRequest},
    storage_readmission::{RegistrationEntry, RegistrationSnapshot},
};
use storage_server::management::storage_lifecycle::ActiveExit;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Inventory {
    pool_id: String,
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
fn original(target: &ManagedPool, plan: &Plan) -> Result<()> {
    target.recheck()?;
    supported(&plan.exit)?;
    if target.context() != plan.context || target.auth().credential_revision()? != plan.credential {
        return Err("original deregistration no longer matches current roots/account or primary credentials; no new authority was invented".into());
    }
    Ok(())
}
fn inventory(control: &ActiveExit<'_>) -> Result<Inventory> {
    let config = control.current_configuration()?;
    let RegistrationSnapshot::ActiveMembership { registrations } = control.registrations()? else {
        return Err(
            "no admitted active registration authority is available; this is not an empty pool"
                .into(),
        );
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
        registrations: entries,
        separate_target_availability_probe_performed: false,
        existing_engine_recovery_may_run: true,
    })
}
pub(super) fn execute(request: WorkerRequest) -> Result<Output> {
    let target = ManagedPool::open(&request.context)?;
    let identity = target.auth().sole_identity()?;
    let control = ActiveExit::open(target.backend(), &identity, target.layout().management()?)?;
    match request.action {
        Action::Inventory => {
            let inventory = inventory(&control)?;
            target.recheck()?;
            Ok(Output::Inventory { inventory })
        }
        Action::Preview {
            operation_id,
            slot_id,
            expected_disk_id,
        } => {
            let exit = control.preview(ExitRequest {
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
                    credential: target.auth().credential_revision()?,
                    exit,
                },
            })
        }
        Action::Original { plan } => {
            original(&target, &plan)?;
            let status = match control.status(plan.exit.request.operation_id) {
                Ok(status) => {
                    if status.preview != plan.exit {
                        return Err(
                            "helper plan differs from the original native deregistration record"
                                .into(),
                        );
                    }
                    Some(status)
                }
                Err(error) if error.code == storage_contract::ErrorCode::OperationNotFound => {
                    if control.preview(plan.exit.request.clone())? != plan.exit {
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
            let status = control.commit(ExitCommit { preview: plan.exit })?;
            target.recheck()?;
            Ok(Output::Status { status })
        }
    }
}
