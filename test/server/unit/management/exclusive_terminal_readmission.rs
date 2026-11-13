//! Shared native owner boundaries. Source only; no execution in this change.
use super::*;
use crate::auth::{AuthStore, SmbBinding};

struct Fixture {
    control: tempfile::TempDir,
    system: tempfile::TempDir,
    management: tempfile::TempDir,
    auth: AuthStore,
}
impl Fixture {
    fn empty() -> Self {
        let system = crate::tests::private_tempdir();
        let management = crate::tests::private_tempdir();
        let auth = AuthStore::initialize_management(
            management.path(),
            "admin",
            "native-readmission-fixture-password",
        )
        .unwrap();
        Self {
            control: crate::tests::private_tempdir(),
            system,
            management,
            auth,
        }
    }
    fn open_as(&self, identity: Identity) -> Result<ExclusiveTerminalReadmission, ApiError> {
        ExclusiveTerminalReadmission::open(
            PrivateDirectory::open_existing(self.control.path()).unwrap(),
            PrivateDirectory::open_existing(self.system.path()).unwrap(),
            PrivateDirectory::open_existing(self.management.path()).unwrap(),
            self.auth.reader(),
            identity,
        )
    }
    fn open(&self) -> Result<ExclusiveTerminalReadmission, ApiError> {
        self.open_as(self.auth.sole_identity()?)
    }
}
#[test]
fn owner_holds_same_native_lifecycle_exclusion_until_drop() {
    let fixture = Fixture::empty();
    let owner = fixture.open().unwrap();
    assert!(fixture.open().is_err());
    drop(owner);
    assert!(fixture.open().is_ok());
}
#[test]
fn empty_system_has_no_terminal_preview_and_is_never_initialized() {
    let fixture = Fixture::empty();
    let owner = fixture.open().unwrap();
    assert!(
        owner
            .preview(ReadmissionRequest {
                operation_id: OperationId::generate().unwrap(),
                slot_id: "P01".into(),
                expected_disk_id: "disk_original".into(),
            })
            .is_err()
    );
    assert_eq!(std::fs::read_dir(fixture.system.path()).unwrap().count(), 0);
    assert!(!fixture.management.path().join(DIRECTORY).exists());
}
#[test]
fn original_lookup_needs_native_record_not_an_arbitrary_operation_id() {
    let fixture = Fixture::empty();
    let owner = fixture.open().unwrap();
    assert_eq!(
        owner
            .status(OperationId::generate().unwrap())
            .unwrap_err()
            .code,
        ErrorCode::OperationNotFound
    );
    assert!(!fixture.management.path().join(DIRECTORY).exists());
    assert_eq!(std::fs::read_dir(fixture.system.path()).unwrap().count(), 0);
}
#[test]
fn no_smb_binding_or_other_principal_can_acquire_this_offline_owner() {
    let fixture = Fixture::empty();
    let mut identity = fixture.auth.sole_identity().unwrap();
    identity.smb_binding = Some(SmbBinding {
        repository_id: "repo_original".into(),
        branch_id: "branch_original".into(),
        generation: 1,
    });
    assert!(fixture.open_as(identity).is_err());
    let mut identity = fixture.auth.sole_identity().unwrap();
    identity.principal = "unrelated".into();
    assert!(fixture.open_as(identity).is_err());
    assert!(
        !fixture
            .management
            .path()
            .join("storage-addition-owner.lock")
            .exists()
    );
}

#[test]
fn held_control_replacement_refuses_existing_owner_without_creating_intent() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::empty();
    let owner = fixture.open().unwrap();
    let held = fixture.control.path().with_extension("original");
    std::fs::rename(fixture.control.path(), &held).unwrap();
    std::fs::create_dir(fixture.control.path()).unwrap();
    std::fs::set_permissions(
        fixture.control.path(), std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let error = owner.status(OperationId::generate().unwrap()).unwrap_err();
    assert_eq!(error.code, ErrorCode::IoError);
    assert_eq!(std::fs::read_dir(fixture.control.path()).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&held).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(fixture.system.path()).unwrap().count(), 0);
    std::fs::remove_dir(&held).unwrap();
}
