use super::*;

fn node(file: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit")
        .join(file);
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
#[test]
fn foreign_review_delivery_observer_preserves_real_requests_and_response_ownership() {
    node("foreign_review_delivery_test.mjs");
}
#[test]
fn foreign_review_accessibility_checker_rejects_unnamed_or_misleading_recovery_states() {
    node("foreign_review_accessibility_test.mjs");
}
