//! Source-level guards only. The Chromium fixture checks actual CSS behavior.
#[test]
fn shared_styles_do_not_erase_native_input_focus_globally() {
    let css = include_str!("../../../src/web/public/styles/components/base.css");
    assert!(
        !css.contains(":root :is(input,select,textarea,[role=combobox]):focus"),
        "Focus suppression belongs only to controls with an explicit replacement indicator"
    );
}

#[test]
fn completed_contour_fallbacks_are_not_hidden_as_pending_menus() {
    let css = include_str!("../../../src/web/public/styles/components/select.css");
    let hidden_rule = css
        .lines()
        .find(|line| line.contains(":popover-open") && line.contains("visibility: hidden"))
        .expect("The first-frame pending paint guard should remain");
    assert!(
        hidden_rule.contains(":not([data-zxs-contour-state=fallback])"),
        "A completed square fallback must remain usable, regardless of reason"
    );
    assert!(
        css.contains("[data-zxs-contour-state=fallback][data-zxs-contour-reason=not-visible] { visibility: hidden; }"),
        "A previously closed, unmeasurable menu still waits for its first paint"
    );
}

#[test]
fn session_restoration_is_visually_empty() {
    let shell = include_str!("../../../src/interface/src/components/shell.rs");
    let css = include_str!("../../../src/web/public/styles/components/base.css");
    assert!(shell.contains("class=\"zxs-application-boot\" role=\"status\" aria-busy=\"true\""));
    for selector in [
        ".zxs-application-boot::before",
        ".zxs-application-boot::after",
        "zxs-application-boot-spin",
    ] {
        assert!(
            !css.contains(selector),
            "unexpected loading decoration: {selector}"
        );
    }
}

#[test]
fn anonymous_forms_keep_visible_controls_and_isolated_login_rows() {
    let css = include_str!("../../../src/web/public/styles/components/auth.css");
    assert!(!css.contains("border-bottom: 1px solid transparent"));
    assert!(css.contains(".authentication-card .zxs-input { min-width: 0; border: 1px solid"));
    assert!(css.contains(".authentication-card .zxs-button { border: 1px solid"));
    assert!(css.contains(".authentication-card > form > .zxs-error {"));
    assert!(!css.contains(".authentication-card .zxs-error {"));
    assert!(css.contains(".authentication-language { justify-self: end; }"));
    assert!(css.contains(".authentication-card .zxs-stack:not(details)"));
}
