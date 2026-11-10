//! Only the non-root service UID touches existing Engine/native membership data.
use super::*;
use crate::offline::managed_pool::ManagedPool;
use std::path::PathBuf;
use storage_engine::storage_addition::{AdditionCommit, AdditionRequest, DirectoryAddition};
use storage_server::{management::storage_addition::ActiveAddition, storage_directories};
fn original(target: &crate::offline::managed_pool::ManagedPool, plan: &Plan) -> Result<()> {
    target.recheck()?;
    if target.context() != plan.context || target.auth().credential_revision()? != plan.credential {
        return Err("original addition no longer matches current roots or primary credentials; no fresh identity/session was invented".into());
    }
    Ok(())
}
pub(super) fn execute(request: WorkerRequest) -> Result<Output> {
    let target = ManagedPool::open(&request.context)?;
    let identity = target.auth().sole_identity()?;
    let control = ActiveAddition::open(
        target.backend(),
        &identity,
        target.layout().management()?,
        layout::DISKS,
    )?;
    // Historical configuration is retained. Only the admitted registration
    // projection decides which roots may be opened by this maintenance owner.
    let (view, stopped) = control.current_directory_scope()?;
    match request.action {
        Action::Preview {
            operation_id,
            additions,
        } => {
            // Snapshot comes from the admitted Engine, not from client-supplied
            // JSON or a copied SQLite index. No stopped root is implicitly reused.
            let registered_roots = root_paths(&view)?;
            let all: Vec<_> = registered_roots
                .iter()
                .cloned()
                .chain(additions.iter().map(|a| a.root_path.clone()))
                .collect();
            target.layout().inventory_addition(&all, &stopped)?;
            let mut excluded: Vec<_> = vec![
                PathBuf::from(layout::STATE),
                PathBuf::from(layout::MANAGEMENT),
            ];
            excluded.extend(
                view.disks
                    .iter()
                    .filter(|disk| !stopped.contains(&disk.root))
                    .map(|disk| disk.root.clone()),
            );
            let mut selected = Vec::with_capacity(additions.len());
            for item in additions {
                let selection = storage_directories::inspect_fresh_path_within(
                    layout::DISKS,
                    &item.root_path,
                    &excluded,
                    &stopped,
                )?;
                excluded.push(PathBuf::from(&item.root_path));
                selected.push(DirectoryAddition {
                    selection,
                    slot_id: item.slot_id,
                    level: item.level,
                    upstream_slot_id: item.upstream_slot_id,
                    metadata_member: item.metadata_member,
                });
            }
            let addition = control.preview(AdditionRequest {
                operation_id,
                additions: selected,
            })?;
            target.recheck()?;
            let plan = Plan {
                format: Format::Addition,
                context: request.context,
                credential: target.auth().credential_revision()?,
                registered_roots,
                addition,
            };
            Ok(Output::Preview { plan })
        }
        Action::Status { plan } => {
            original(&target, &plan)?;
            current_roots_match(&plan, &root_paths(&view)?)?;
            target.layout().inventory_addition(&plan.roots(), &stopped)?;
            let status = match control.status(plan.addition.request.operation_id) {
                Ok(status) => {
                    if status.preview != plan.addition {
                        return Err(
                            "saved helper plan differs from the original native control record"
                                .into(),
                        );
                    }
                    Some(status)
                }
                Err(error) if error.code == storage_engine::ErrorCode::OperationNotFound => {
                    // Root intent may precede the child/native intent. Only an
                    // exactly unchanged fresh preview may be explicitly retried.
                    if control.preview(plan.addition.request.clone())? != plan.addition {
                        return Err("unstarted addition no longer has its original complete fresh selection".into());
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
            current_roots_match(&plan, &root_paths(&view)?)?;
            target.layout().inventory_addition(&plan.roots(), &stopped)?;
            let status = control.commit(AdditionCommit {
                preview: plan.addition,
            })?;
            target.recheck()?;
            Ok(Output::Status { status })
        }
    }
}

fn root_paths(config: &storage_engine::Config) -> Result<Vec<String>> {
    config
        .disks
        .iter()
        .map(|disk| {
            disk.root.to_str().map(str::to_owned).ok_or_else(|| {
                "registered root is not UTF-8; preserve the original configuration".into()
            })
        })
        .collect()
}
