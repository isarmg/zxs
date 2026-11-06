use super::*;

#[test]
fn tag_unknown_probe_rejects_unproven_publication_and_changed_replay() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit/tag_unknown_probe_test.mjs");
    let result = Command::new("node")
        .arg(path)
        .output()
        .expect("Node is required by repository validation");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
