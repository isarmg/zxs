//! Workspace-facing JSON target spelling is independent of OC2 field placement.
use super::*;

#[test]
fn first_append_and_fork_keep_their_exact_wire_shapes() {
    let vectors = [
        (
            core::Target::FirstMain {
                name: "样本".into(),
            },
            r#"{"kind":"first_main","name":"样本"}"#,
        ),
        (
            core::Target::Branch {
                name: String::from("main"),
                expected_head: [0x44; 16],
            },
            r#"{"kind":"branch","branch":"main","expected_head":"44444444444444444444444444444444"}"#,
        ),
        (
            core::Target::HistoricalMain {
                new_branch: "fork".into(),
                original: [0x55; 16],
            },
            r#"{"kind":"historical_main","branch":"fork","from_node":"55555555555555555555555555555555"}"#,
        ),
    ];
    for (target, expected) in vectors {
        let mut json = Json(String::new());
        json.target(&target).unwrap();
        assert_eq!(json.0, expected);
    }
}
