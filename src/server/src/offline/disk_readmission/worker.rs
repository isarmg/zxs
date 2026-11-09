//! Only the service UID owns native metadata and the confirmed retained root.
use super::*;
use platform_io::service_lease::ServiceLease;
use storage_engine::storage_readmission::ReadmissionCommit;
use storage_server::{
    auth::AuthStore, management::storage_readmission::ExclusiveTerminalReadmission,
    startup_admission::PreparedService,
};
struct Target {
    control: ExclusiveTerminalReadmission,
    auth: AuthStore,
    prepared: PreparedService,
    layout: Layout,
    lease: ServiceLease,
}
impl Target {
    fn open(context: &layout::Context) -> Result<Self> {
        let lease = ServiceLease::host()?;
        let layout = Layout::open(context.account.name.as_str().into())?;
        if layout.context(lease.identity()) != *context {
            return Err("readmission worker host/account/layout identity changed".into());
        }
        let prepared =
            PreparedService::open(Path::new(layout::STATE), Path::new(layout::MANAGEMENT))?;
        let auth = AuthStore::open_system_management_directory(layout.management()?)?;
        let control = ExclusiveTerminalReadmission::open(
            layout.state()?,
            layout.management()?,
            auth.reader(),
            auth.sole_identity()?,
        )?;
        Ok(Self {
            control,
            auth,
            prepared,
            layout,
            lease,
        })
    }
    fn recheck(&self) -> Result<()> {
        self.lease.recheck()?;
        self.layout.recheck()?;
        self.prepared.recheck()?;
        Ok(())
    }
    fn original(&self, plan: &Plan) -> Result<()> {
        self.recheck()?;
        supported(&plan.readmission)?;
        layout::names(std::slice::from_ref(&plan.readmission.target.root_path))?;
        if self.layout.context(self.lease.identity()) != plan.context
            || self.auth.credential_revision()? != plan.credential
        {
            return Err(
                "original readmission roots/account or primary credential revision changed".into(),
            );
        }
        Ok(())
    }
}
pub(super) fn execute(request: WorkerRequest) -> Result<Output> {
    let target = Target::open(&request.context)?;
    target.recheck()?;
    let output = match request.action {
        Action::Preview { request: selection } => {
            let readmission = target.control.preview(selection)?;
            supported(&readmission)?;
            layout::names(std::slice::from_ref(&readmission.target.root_path))?;
            Output::Preview {
                plan: Plan {
                    format: Format::Readmission,
                    context: request.context,
                    credential: target.auth.credential_revision()?,
                    readmission,
                },
            }
        }
        Action::Original { plan } => {
            target.original(&plan)?;
            let status = match target.control.status(plan.readmission.request.operation_id) {
                Ok(status) => {
                    if status.preview != plan.readmission {
                        return Err("original native readmission differs from the root plan".into());
                    }
                    Some(status)
                }
                Err(error) if error.code == storage_engine::ErrorCode::OperationNotFound => {
                    if target.control.preview(plan.readmission.request.clone())? != plan.readmission
                    {
                        return Err(
                            "unstarted readmission no longer has the exact original source".into(),
                        );
                    }
                    None
                }
                Err(error) => return Err(error.into()),
            };
            Output::Original { status }
        }
        Action::Commit { plan } => {
            target.original(&plan)?;
            let status = target.control.commit(ReadmissionCommit {
                preview: plan.readmission,
            })?;
            Output::Status { status }
        }
    };
    target.recheck()?;
    Ok(output)
}
