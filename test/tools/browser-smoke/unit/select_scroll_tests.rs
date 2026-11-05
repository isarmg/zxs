use super::*;

#[test]
fn select_scroll_observer_rejects_transient_drift_and_stationary_only_evidence() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit/select_scroll_probe_test.mjs");
    let result = Command::new("node")
        .arg(path)
        .output()
        .expect("Node is required by repository validation");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
