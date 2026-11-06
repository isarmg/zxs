#[test]
fn backup_navigation_probe_preserves_responses_and_rejects_false_navigation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit/backup_browse_probe_test.mjs");
    let result = std::process::Command::new("node")
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
