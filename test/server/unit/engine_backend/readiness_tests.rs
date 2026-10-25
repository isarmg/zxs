use super::*;
use crate::BackendAction;
use storage_engine::Config;

#[test]
fn readiness_uses_fixed_membership_and_reports_single_disk_no_metadata_redundancy() {
    for count in [1, 2, 3, 4] {
        let temp = tempfile::tempdir().unwrap();
        let disks: Vec<_> = (0..count)
            .map(|n| temp.path().join(format!("disk-{n}")))
            .collect();
        let engine = Engine::initialize(
            &temp.path().join("system"),
            Config::development(disks.clone()),
        )
        .unwrap();
        let backend = EngineBackend::new(engine);
        let anonymous = Identity {
            admission_session: None,
            smb_binding: None,
            principal: String::new(),
        };
        let ready = backend
            .call(BackendAction::Ready, &anonymous, json!({}))
            .unwrap();
        assert_eq!(ready["metadata_member_count"], count);
        assert_eq!(ready["storage_disk_count"], count);
        assert_eq!(
            ready["metadata_redundancy"],
            if count == 1 {
                "none"
            } else {
                "multiple-copies"
            }
        );
        std::fs::rename(&disks[count - 1], temp.path().join("missing-disk")).unwrap();
        assert_eq!(
            backend
                .call(BackendAction::Ready, &anonymous, json!({}))
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::MetadataUnavailable
        );
    }
}
