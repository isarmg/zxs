use super::*;

#[test]
fn row_identity_selection_and_focus_are_independent_required_conditions() {
    let good = json!({
        "rowConnected":true, "buttonSame":true, "checkboxSame":true,
        "checked":true, "rowSelected":true, "focused":true
    });
    assert!(assert_retained_row(good.clone()).is_ok());
    for key in [
        "rowConnected",
        "buttonSame",
        "checkboxSame",
        "checked",
        "rowSelected",
        "focused",
    ] {
        let mut bad = good.clone();
        bad[key] = false.into();
        let error = assert_retained_row(bad.clone()).unwrap_err().to_string();
        assert!(error.contains(&bad.to_string()), "{key}: {error}");
        // Missing measurements must not silently count as retained DOM/focus.
        bad.as_object_mut().unwrap().remove(key);
        assert!(assert_retained_row(bad).is_err(), "{key}");
    }
}

#[test]
fn an_unchecked_row_reports_selection_failure_without_claiming_dom_or_focus_loss() {
    let snapshot = json!({
        "rowConnected":true, "buttonSame":true, "checkboxSame":true,
        "checked":false, "rowSelected":false, "focused":true
    });
    let error = assert_retained_row(snapshot).unwrap_err().to_string();
    assert!(error.contains("\"checked\":false"));
    assert!(error.contains("\"focused\":true"));
    assert!(error.contains("\"rowConnected\":true"));
}
