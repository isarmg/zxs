use super::*;

fn metrics(value: f64) -> Value {
    json!({"metrics":[
        {"name":"LayoutCount","value":value},
        {"name":"RecalcStyleCount","value":value},
        {"name":"LayoutDuration","value":value},
        {"name":"RecalcStyleDuration","value":value},
        {"name":"ScriptDuration","value":value},
        {"name":"TaskDuration","value":value}
    ]})
}

#[test]
fn chrome_metrics_keep_units_and_allow_actual_zero_work() {
    let mut before = metrics(1.0);
    before["metrics"][2]["value"] = json!(1.25);
    let after = metrics(2.0);
    let delta = metric_deltas(&before, &after).unwrap();
    assert_eq!(delta["LayoutCount"], 1.0);
    assert_eq!(delta["LayoutDuration"], 0.75);
    let zero = metric_deltas(&after, &after).unwrap();
    assert_eq!(zero["TaskDuration"], 0.0);
}

#[test]
fn missing_or_invalid_metrics_never_become_zero_evidence() {
    let mut missing = metrics(2.0);
    missing["metrics"].as_array_mut().unwrap().pop();
    let error = metric_deltas(&metrics(1.0), &missing).unwrap_err();
    assert!(error.to_string().contains("TaskDuration"));
    let mut invalid = metrics(2.0);
    invalid["metrics"][0]["value"] = json!("not a number");
    assert!(metric_deltas(&metrics(1.0), &invalid).is_err());
    assert!(metric_deltas(&metrics(2.0), &metrics(1.0)).is_err());
}

#[test]
fn contour_probe_has_valid_javascript_syntax() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/tools/browser-smoke/runner/contour_evidence.js");
    let result = Command::new("node")
        .arg("--check")
        .arg(path)
        .output()
        .expect("Node is required by repository validation");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
