#[cfg(test)]
mod test {
    use crate::*;

    #[test]
    pub fn error_new() {
        let assoc = vec![
            (SQLITE_INTERNAL, ErrorCode::InternalMalfunction),
            (SQLITE_PERM, ErrorCode::PermissionDenied),
            (SQLITE_ABORT_ROLLBACK, ErrorCode::OperationAborted),
            (SQLITE_BUSY_RECOVERY, ErrorCode::DatabaseBusy),
            (SQLITE_LOCKED_SHAREDCACHE, ErrorCode::DatabaseLocked),
            (SQLITE_NOMEM, ErrorCode::OutOfMemory),
            (SQLITE_IOERR_READ, ErrorCode::SystemIoFailure),
            (SQLITE_NOTFOUND, ErrorCode::NotFound),
            (SQLITE_FULL, ErrorCode::DiskFull),
            (SQLITE_PROTOCOL, ErrorCode::FileLockingProtocolFailed),
            (SQLITE_SCHEMA, ErrorCode::SchemaChanged),
            (SQLITE_TOOBIG, ErrorCode::TooBig),
            (SQLITE_MISMATCH, ErrorCode::TypeMismatch),
            (SQLITE_NOLFS, ErrorCode::NoLargeFileSupport),
            (SQLITE_RANGE, ErrorCode::ParameterOutOfRange),
            (SQLITE_NOTADB, ErrorCode::NotADatabase),
        ];
        for (sqlite_code, rust_code) in assoc {
            let err = Error::new(sqlite_code);
            assert_eq!(
                err,
                Error {
                    code: rust_code,
                    extended_code: sqlite_code
                }
            );
            let s = format!("{err}");
            assert!(!s.is_empty());
        }
    }
}
