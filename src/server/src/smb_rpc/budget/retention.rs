//! Body retirement keeps the original-query record and its metadata charge.
use super::*;
use platform_io::fs_safety::PayloadOpenMode;

#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::smb_rpc) enum Step {
    BeforeUnlink,
    AfterUnlink,
    AfterSync,
    BeforeOwner,
    AfterOwner,
}
#[cfg(feature = "fault-injection")]
thread_local! {
    static FAILURE: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
}
#[cfg(feature = "fault-injection")]
pub(in crate::smb_rpc) fn fail_at(step: Step) {
    FAILURE.set(Some(step));
}
#[cfg(feature = "fault-injection")]
fn checkpoint(step: Step) -> Result<(), ApiError> {
    if FAILURE.get() == Some(step) {
        FAILURE.set(None);
        return Err(ApiError::io(io::Error::new(io::ErrorKind::Interrupted,
            "injected manual retirement interruption")));
    }
    Ok(())
}

impl Budget {
    pub(in crate::smb_rpc) fn retire_manual_body(
        &self,
        principal: &str,
        operation: OperationId,
        authorize: impl FnOnce() -> Result<(), ApiError>,
        verify: impl FnOnce(&File) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        // This lock serializes retirement and admission inside the service.
        // Frozen FDs are private and never lent to SMB peers. The private held
        // spool directory is the filesystem authority boundary.
        let mut ledger = self.lock_ledger();
        if ledger.uncertain {
            return Err(invalid("SMB spool needs reconciliation"));
        }
        authorize()?;
        let name = EntryName::new(format!("manual-{operation}"))
            .map_err(crate::upload_fs_error)?;
        let source = self.root.join(name.as_path());
        let charge = ledger.files.get(&source)
            .ok_or_else(|| invalid("manual body has no retained budget owner"))?;
        if charge.principal.as_deref() != Some(principal) {
            return Err(invalid("manual body budget owner differs"));
        }
        let directory = PrivateDirectory::open_trusted(&self.root)
            .map_err(crate::upload_fs_error)?;
        let owner_name = EntryName::new(format!("manual-{operation}.budget.json"))
            .map_err(crate::upload_fs_error)?;
        let bytes = directory.read_bounded(&owner_name, RECORD_LIMIT as usize)
            .map_err(crate::upload_fs_error)?;
        let mut owner: Owner = storage_contract::decode_strict(&bytes)
            .map_err(|error| {
                invalid("manual body ownership record is invalid").with_source(error)
            })?;
        if owner.principal != principal || owner.source != format!("manual-{operation}") {
            return Err(invalid("manual body ownership record differs"));
        }
        // Keep a bounded original-query record. It is never replaced or deleted
        // by retirement, and remains covered by the existing metadata overhead.
        let query = EntryName::new(format!("manual-{operation}.work.json"))
            .map_err(crate::upload_fs_error)?;
        directory.read_bounded(&query, RECORD_LIMIT as usize)
            .map_err(crate::upload_fs_error)?;
        let namespace = EntryName::new(format!("manual-{operation}.namespace.json"))
            .map_err(crate::upload_fs_error)?;
        match directory.open_payload(&namespace, PayloadOpenMode::Existing) {
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => return Err(invalid("manual body has unexpected namespace evidence")),
            Err(error) => return Err(crate::upload_fs_error(error)),
        }
        match directory.open_payload(&name, PayloadOpenMode::Existing) {
            Ok(body) => {
                verify(&body)?;
                directory.verify_payload(&name, &body).map_err(crate::upload_fs_error)?;
                #[cfg(feature = "fault-injection")]
                checkpoint(Step::BeforeUnlink)?;
                directory.remove_file(&name).map_err(crate::upload_fs_error)?;
                #[cfg(feature = "fault-injection")]
                checkpoint(Step::AfterUnlink)?;
            }
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(crate::upload_fs_error(error)),
        }
        // An interrupted unlink/sync retries here before any capacity is freed.
        directory.sync().map_err(crate::upload_fs_error)?;
        #[cfg(feature = "fault-injection")]
        checkpoint(Step::AfterSync)?;
        owner.admitted_end = 0;
        // Failed/unknown persistence leaves the conservative old in-memory charge.
        // A retry or cold inventory can settle it only after the body is absent.
        #[cfg(feature = "fault-injection")]
        checkpoint(Step::BeforeOwner)?;
        persist::grow(&self.root, &source, &owner)?;
        #[cfg(feature = "fault-injection")]
        checkpoint(Step::AfterOwner)?;
        if let Err(error) = ledger.replace(source, Some(Charge {
            principal: Some(principal.to_owned()), bytes: OVERHEAD,
        })) {
            ledger.uncertain = true;
            return Err(error);
        }
        Ok(())
    }
}
