//! Existing production-pool owner shared by helper maintenance workers. This
//! never initializes missing roots/credentials and never starts service threads.
use crate::offline::{
    Result,
    managed_layout::{self as layout, Context, Layout},
};
use platform_io::service_lease::ServiceLease;
use std::path::Path;
use storage_engine::{Engine, Mode};
use storage_server::{
    auth::AuthStore, engine_backend::EngineBackend, startup_admission::PreparedService,
};

pub(crate) struct ManagedPool {
    auth: AuthStore,
    backend: EngineBackend,
    layout: Layout,
    prepared: PreparedService,
    lease: ServiceLease,
}
impl ManagedPool {
    pub fn open(context: &Context) -> Result<Self> {
        let lease = ServiceLease::host()?;
        let layout = Layout::open(context.account.name.as_str().into())?;
        if layout.context(lease.identity()) != *context {
            return Err("maintenance worker rejected changed host/account/layout identities before opening storage".into());
        }
        let prepared =
            PreparedService::open(Path::new(layout::STATE), Path::new(layout::MANAGEMENT))?;
        let auth = AuthStore::open_system_management_directory(layout.management()?)?;
        let engine = Engine::open_storage_held(prepared.system_directory()?, None)?;
        if engine.config().mode != Mode::Production {
            return Err(
                "helper membership maintenance requires an existing production pool".into(),
            );
        }
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
    pub fn recheck(&self) -> Result<()> {
        self.lease.recheck()?;
        self.layout.recheck()?;
        self.prepared.recheck()?;
        Ok(())
    }
    pub fn context(&self) -> Context {
        self.layout.context(self.lease.identity())
    }
    pub fn auth(&self) -> &AuthStore {
        &self.auth
    }
    pub fn backend(&self) -> &EngineBackend {
        &self.backend
    }
    pub fn layout(&self) -> &Layout {
        &self.layout
    }
}
