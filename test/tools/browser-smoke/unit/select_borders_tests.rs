use super::*;

#[test]
fn select_border_observer_rejects_wrong_palette_width_or_changed_fill() {
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

#[test]
fn text_input_observer_rejects_boxes_and_stale_contour_paint() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit/input_surfaces_probe_test.mjs");
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

#[test]
fn select_focus_observer_rejects_square_hidden_or_stale_focus_paint() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/unit/select_focus_probe_test.mjs");
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
