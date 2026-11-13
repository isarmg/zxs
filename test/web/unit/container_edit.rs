use super::*;
use storage_contract::Generation;

fn target() -> Target {
    Target {
        epoch: 2, navigation: 3, entry: "entry".into(),
        request: ReadRequest {
            file_id: storage_contract::FileId::from_random([1; 16]),
            context: Context {
                repository_id: storage_contract::RepositoryId::from_random([2; 16]),
                path: name_policy::PortablePath::parse("file.txt").unwrap(),
            },
        },
    }
}

#[test]
fn policy_reads_reject_previous_session_navigation_occurrence_and_request() {
    let target = target();
    let panel = Panel { target: Some(target.clone()), generation: 9, ..Panel::default() };
    assert!(panel.accepts(&target, 9));
    assert!(!panel.accepts(&target, 8));
    for change in 0..4 {
        let mut stale = target.clone();
        match change {
            0 => stale.epoch += 1,
            1 => stale.navigation += 1,
            2 => stale.entry = "other".into(),
            _ => stale.request.context.path = name_policy::PortablePath::parse("other.txt").unwrap(),
        }
        assert!(!panel.accepts(&stale, 9));
    }
}

#[test]
fn unknown_state_never_enables_automatic_and_dirty_cannot_switch() {
    assert!(decode(json_codec::json!({})).is_err());
    assert!(decode(json_codec::json!({"state":"manual_dirty","generation":"2"})).is_err());
    assert!(can_switch(&ContainerEdit::initial()));
    assert!(can_switch(&ContainerEdit::ManualClean { generation: Generation::new(1) }));
    let dirty = decode(json_codec::json!({
        "state":"manual_dirty","generation":"2","draft":{
            "operation":"op_00000000000000000000000000000001",
            "expected_generation":"1","principal":"alice","size":"5",
            "content_hash":"0000000000000000000000000000000000000000000000000000000000000000",
            "base":{
                "entry":"entry_00000000000000000000000000000001",
                "repository":"repo_00000000000000000000000000000001",
                "branch":"br_00000000000000000000000000000001",
                "node":"ver_00000000000000000000000000000001",
                "version":"ver_00000000000000000000000000000001",
                "selection_generation":"0"
            }
        }
    })).unwrap();
    assert!(!can_switch(&dirty));
    assert!(dirty.set_mode(Generation::new(2), CommitMode::Automatic).is_err());
}
