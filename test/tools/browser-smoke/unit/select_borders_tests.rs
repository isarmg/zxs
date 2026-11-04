use super::*;

#[test]
fn select_border_observer_rejects_missing_or_nonblack_paint() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit/select_borders_probe_test.mjs");
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
