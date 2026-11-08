//! Only the non-root service UID touches existing Engine/native membership data.
use super::*;
use platform_io::service_lease::ServiceLease;
use std::path::PathBuf;
use storage_contract::{
    storage_addition::{AdditionCommit, AdditionRequest, DirectoryAddition},
    storage_directories::InspectRequest,
};
use storage_engine::{Engine, Mode};
use storage_server::{
    auth::AuthStore, engine_backend::EngineBackend, management::storage_addition::ActiveAddition,
    startup_admission::PreparedService, storage_directories,
};
struct Target {
    auth: AuthStore,
    backend: EngineBackend,
    layout: Layout,
    prepared: PreparedService,
    lease: ServiceLease,
}
impl Target {
    fn open(context: &layout::Context) -> Result<Self> {
        let lease = ServiceLease::host()?;
        let layout = Layout::open(context.account.name.as_str().into())?;
        if layout.context(lease.identity()) != *context {
            return Err("addition worker rejected changed host/account/layout identities before opening storage".into());
        }
        let prepared =
            PreparedService::open(Path::new(layout::STATE), Path::new(layout::MANAGEMENT))?;
        let auth = AuthStore::open_system_management_directory(layout.management()?)?;
        let engine = Engine::open(Path::new(layout::STATE))?;
        let backend = EngineBackend::new(engine);
        backend.set_management_auth(auth.reader())?;
        let target = Self {
            auth,
            backend,
            layout,
            prepared,
            lease,
        };
        target.recheck()?;
        Ok(target)
    }
    fn recheck(&self) -> Result<()> {
        self.lease.recheck()?;
        self.layout.recheck()?;
        self.prepared.recheck()?;
        Ok(())
    }
    fn original(&self, plan: &Plan) -> Result<()> {
        self.recheck()?;
        if self.layout.context(self.lease.identity()) != plan.context
            || self.auth.credential_revision()? != plan.credential
        {
            return Err("original addition no longer matches its current root identities or primary credentials; no fresh identity/session was invented".into());
        }
        self.layout.inventory_all(&plan.roots())
    }
}
pub(super) fn execute(request: WorkerRequest) -> Result<Output> {
    let target = Target::open(&request.context)?;
    let identity = target.auth.sole_identity()?;
    let control = ActiveAddition::open(
        &target.backend,
        &identity,
        target.layout.management()?,
        layout::DISKS,
    )?;
    match request.action {
        Action::Preview {
            operation_id,
            additions,
        } => {
            // Snapshot comes from the admitted Engine, not from client-supplied
            // JSON or a copied SQLite index. No stopped root is implicitly reused.
            let view = control.current_configuration()?;
            if view.mode != Mode::Production {
                return Err(
                    "helper additions currently require an existing production pool".into(),
                );
            }
            let registered_roots = root_paths(&view)?;
            let all: Vec<_> = registered_roots
                .iter()
                .cloned()
                .chain(additions.iter().map(|a| a.root_path.clone()))
                .collect();
            target.layout.inventory_all(&all)?;
            let mut excluded: Vec<_> = vec![
                PathBuf::from(layout::STATE),
                PathBuf::from(layout::MANAGEMENT),
            ];
            excluded.extend(view.disks.iter().map(|d| d.root.clone()));
            let mut selected = Vec::with_capacity(additions.len());
            for item in additions {
                let location = storage_directories::browse(&item.root_path)?;
                let selection = storage_directories::inspect_within(
                    layout::DISKS,
                    &InspectRequest {
                        mount_path: location.mount_path,
                        root_path: item.root_path.clone(),
                    },
                    &excluded,
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
                credential: target.auth.credential_revision()?,
                registered_roots,
                addition,
            };
            Ok(Output::Preview { plan })
        }
        Action::Status { plan } => {
            target.original(&plan)?;
            current_roots_match(&plan, &root_paths(&control.current_configuration()?)?)?;
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
                Err(error) if error.code == storage_contract::ErrorCode::OperationNotFound => {
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
            target.original(&plan)?;
            current_roots_match(&plan, &root_paths(&control.current_configuration()?)?)?;
            let status = control.commit(AdditionCommit {
                preview: plan.addition,
            })?;
            target.recheck()?;
            Ok(Output::Status { status })
        }
    }
}

fn root_paths(config: &storage_contract::Config) -> Result<Vec<String>> {
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
