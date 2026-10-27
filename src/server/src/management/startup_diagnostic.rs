//! Startup owns consumed credential failures, but credential bytes are secret.
use crate::{ApiError, http_boundary::ApiErrorSource};
use std::{error::Error as _, fmt};

pub(super) struct StartupDiagnostic<'a>(pub(super) &'a ApiError);

impl fmt::Display for StartupDiagnostic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Both strict JSON and typed credential decoding can include an input
        // string in their messages. Do not print either that message or its
        // copy in ApiError.message; preserve the decoder's category/location.
        let json = match self.0.source.as_deref() {
            Some(ApiErrorSource::Json(source))
            | Some(ApiErrorSource::Contract(storage_contract::Error::Json { source, .. })) => {
                Some(source)
            }
            _ => None,
        };
        if let Some(source) = json {
            return write!(
                f,
                "{}: credential JSON {:?} error at line {}, column {}",
                self.0.code,
                source.classify(),
                source.line(),
                source.column()
            );
        }
        // The remaining startup producers emit static validation messages or
        // filesystem/authentication causes, not credential fields or tokens.
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(cause) = source {
            write!(f, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}
