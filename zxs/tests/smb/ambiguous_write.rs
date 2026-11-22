//! Fixed status routing, not an issued storage operation or injected outcome.
use super::*;

#[test]
fn an_inconclusive_original_never_maps_to_a_signed_write_failure(){
    for unseen in [
        WriteFault::Original(io::ErrorKind::ConnectionAborted.into()),
        WriteFault::Confirmation(io::ErrorKind::ConnectionAborted.into()),
    ]{
        assert!(matches!(failure(FileWriteError::Fault(unseen)),Err(Fault::WriteOutcomeUnknown)));
    }
    for definite in [
        WriteFault::Original(io::ErrorKind::StorageFull.into()),
        WriteFault::Confirmation(io::ErrorKind::StorageFull.into()),
    ]{
        assert!(matches!(failure(FileWriteError::Fault(definite)),Ok(WriteFailure::DiskFull)));
    }
}
