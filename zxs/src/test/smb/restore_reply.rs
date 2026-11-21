//! Closed mapping only, not an invented transfer state or R authority.
use super::*;

#[test]
fn a_real_collision_never_looks_like_replacement_or_ambiguous_commit(){
    let collision=RestoreError::Original(io::ErrorKind::AlreadyExists.into());
    assert!(matches!(restore_failure(collision),Ok(RestoreFailure::ObjectNameCollision)));
    let revoked=RestoreError::Original(io::ErrorKind::PermissionDenied.into());
    assert!(matches!(restore_failure(revoked),Ok(RestoreFailure::AccessDenied)));
    let unknown=RestoreError::Original(io::ErrorKind::ConnectionAborted.into());
    assert!(matches!(restore_failure(unknown),Err(Fault::WriteOutcomeUnknown)));
    assert!(matches!(parse_failure(RenameError::Fields(SetError::InformationLength)),
        Ok(RestoreFailure::InformationLengthMismatch)));
    assert!(matches!(parse_failure(RenameError::Name(NameError::InvalidName)),Ok(RestoreFailure::ObjectNameInvalid)));
}

#[test]
fn ordinary_rename_reports_only_known_refusals(){
    for (failure,expected) in [
        (OrdinaryRenameError::SharingViolation,0xc000_0043),
        (OrdinaryRenameError::DeletePending,0xc000_0056),
        (OrdinaryRenameError::Name(NameError::Field(FieldError::Unsupported)),0xc000_00bb),
        (OrdinaryRenameError::Original(io::ErrorKind::AlreadyExists.into()),0xc000_0035),
    ]{
        assert_eq!(ordinary_rename_failure(failure).unwrap() as u32,expected);
    }
    let unknown=OrdinaryRenameError::Original(io::ErrorKind::ConnectionAborted.into());
    assert!(matches!(ordinary_rename_failure(unknown),Err(Fault::WriteOutcomeUnknown)));
}
