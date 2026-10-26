//! A mutex-owned engine and its validated, non-authoritative admission projection.
//!
//! Engine mutations invalidate the protected snapshot projection. Managed servers
//! instead revalidate the current system authority after acquiring the engine
//! lock. Lock order is engine -> short auth read; managed auth writes never
//! acquire the engine mutex and replicate only through a detached worker.
use crate::{
    ApiError,
    auth::{AdmissionSnapshot, Identity},
};
use std::{
    ops::{Deref, DerefMut},
    sync::OnceLock,
};
use storage_engine::Engine;

pub(super) struct AdmissionEngine {
    engine: Engine,
    managed_auth: Option<crate::auth::AuthReader>,
    admission: OnceLock<Result<Option<AdmissionSnapshot>, std::sync::Arc<ApiError>>>,
    #[cfg(test)]
    validations: std::sync::atomic::AtomicUsize,
}

impl AdmissionEngine {
    pub(super) fn new(engine: Engine) -> Self {
        Self {
            engine,
            managed_auth: None,
            admission: OnceLock::new(),
            #[cfg(test)]
            validations: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(super) fn set_management_auth(&mut self, auth: crate::auth::AuthReader) {
        self.managed_auth = Some(auth);
    }
    pub(super) fn identity(&self, caller: &Identity) -> Result<Identity, ApiError> {
        if let Some(auth) = &self.managed_auth {
            return auth.revalidate_identity(caller);
        }

        let projection = self.admission.get_or_init(|| {
            #[cfg(test)]
            self.validations
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.engine
                .auth_state_ref()
                .map(|snapshot| {
                    AdmissionSnapshot::from_snapshot(snapshot).map_err(std::sync::Arc::new)
                })
                .transpose()
        });
        match projection {
            Ok(Some(snapshot)) => snapshot.revalidate_identity(caller),
            // An engine without authoritative credentials cannot admit data access.
            Ok(None) => Err(ApiError::unauthorized()),
            Err(error) => Err(ApiError::invalid_source(std::sync::Arc::clone(error))),
        }
    }
}

impl Deref for AdmissionEngine {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.engine
    }
}

impl DerefMut for AdmissionEngine {
    fn deref_mut(&mut self) -> &mut Engine {
        self.admission.take();
        &mut self.engine
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/admission_tests.rs"]
mod tests;
