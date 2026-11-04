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
fn anonymous_forms_reuse_six_shared_height_rows_and_panel_contours() {
    let css = include_str!("../../../src/web/public/styles/components/auth.css");
    assert!(css.contains("aspect-ratio: 3 / 2;"));
    assert!(css.contains(
        "grid-template-rows: repeat(6, minmax(var(--zxs-compact-row-height), max-content))"
    ));
    assert!(css.contains("width: min(calc(var(--zxs-compact-row-height) * 9), 100%)"));
    assert!(css.contains("min-height: calc(var(--zxs-compact-row-height) * 6)"));
    assert!(css.contains("--zxs-contour-extent: var(--zxs-contour-panel)"));
    for private_override in [
        "--zxs-contour-control:",
        "--zxs-contour-panel:",
        "--zxs-contour-notification:",
        "380px",
        "100cqw",
        "overflow: hidden",
        "display: contents",
    ] {
        assert!(
            !css.contains(private_override),
            "obsolete private geometry: {private_override}"
        );
    }
    assert!(css.contains(".authentication-card { aspect-ratio: auto; }"));
    let login = include_str!("../../../src/interface/src/components/login.rs");
    let setup = include_str!("../../../src/web/src/setup.rs");
    for source in [login, setup] {
        assert!(source.contains("class=\"zxs-auth-information\""));
        assert!(source.contains("class=\"zxs-auth-actions\""));
        assert_eq!(source.matches("<label class=\"zxs-auth-label\"").count(), 2);
        assert_eq!(source.matches("<input id=").count(), 2);
        assert!(!source.contains("zxs-form-grid"));
        assert!(!source.contains("<h1") && !source.contains("<h2"));
    }
}

#[test]
fn anonymous_card_has_no_nested_decorative_surfaces_or_setup_explanation() {
    let css = include_str!("../../../src/web/public/styles/components/auth.css");
    let rule = css.split(".authentication-shell .authentication-card :is(.zxs-auth-information, .zxs-auth-actions, .zxs-error, .zxs-button) {")
        .nth(1).unwrap().split('}').next().unwrap();
    for property in [
        "border: 0;",
        "background: transparent;",
        "box-shadow: none;",
        "--zxs-contour-extent: 0px;",
        "--zxs-contour-mode: none;",
        "--zxs-contour-foreground: 0;",
    ] {
        assert!(
            rule.contains(property),
            "missing unboxed property: {property}"
        );
    }
    let setup = include_str!("../../../src/web/src/setup.rs");
    assert!(!setup.contains("Before an account is set"));
    assert!(!setup.contains("未设置账户时"));
    assert!(!setup.contains("confirmed:") && !setup.contains("data-setup-confirm"));
    assert!(setup.contains("请设置账户名和密码"));
    for marker in [
        "data-first-account-form",
        "data-setup-user",
        "data-setup-password",
        "data-setup-check",
        "data-setup-clear",
        "can_initialize(s)",
    ] {
        assert!(
            setup.contains(marker),
            "account creation must retain {marker}"
        );
    }
    let ui = include_str!("../../../src/web/src/ui.rs");
    assert!(ui.contains("crate::setup::view(setup_state, sign_in.clone())"));
    let shell = include_str!("../../../src/interface/src/components/shell.rs");
    let card = shell
        .split("<div class=\"authentication-card\">")
        .nth(1)
        .unwrap()
        .split("</div>")
        .next()
        .unwrap();
    assert!(!card.contains("zxs-product-identity"));
}

#[test]
fn file_rows_use_checked_selection_and_pointer_capable_hover() {
    let view = include_str!("../../../src/web/src/files/view.rs");
    let css = include_str!("../../../src/web/public/styles/components/file-manager.css");
    assert!(view.contains("aria-selected=move ||checked.get().to_string()"));
    assert!(!view.contains("selected: Memo<Option<String>>"));
    assert!(css.contains(".zxs-file-table tbody tr { background: transparent; }"));
    assert!(css.contains("@media (hover: hover) and (pointer: fine)"));
}

#[test]
fn header_omits_account_text_but_preserves_accessible_account_control() {
    let ui = include_str!("../../../src/web/src/ui.rs");
    let shell = include_str!("../../../src/interface/src/components/shell.rs");
    let icons = include_str!("../../../src/web/src/icons.rs");
    assert!(!ui.contains("zxs-session-identity"));
    assert!(shell.contains("aria-label=t(\"账号设置\", \"Account settings\")"));
    assert!(icons.contains("M19.518 9.264a8 8 0 1 1-4.782-4.782"));
}

#[test]
fn header_icons_follow_menu_text_without_shrinking_hit_targets() {
    let css = include_str!("../../../src/web/public/styles/components/geometry.css");
    let rule = css
        .split(".zxs-app-shell .zxs-control-group--comfortable .zxs-button {")
        .nth(1)
        .expect("header button geometry")
        .split('}')
        .next()
        .unwrap();
    assert!(rule.contains("font-size: var(--zxs-compact-font-size);"));
    assert!(rule.contains("--zxs-control-height: var(--zxs-input-row-height);"));
    assert!(rule.contains("min-width: var(--zxs-control-height);"));
    assert!(rule.contains("height: var(--zxs-control-height);"));
    let appearance = include_str!("../../../src/web/public/styles/components/content-blocks.css");
    assert!(appearance.contains(".zxs-header-actions svg { width: var(--zxs-header-icon-size,1em); height: var(--zxs-header-icon-size,1em); }"));
}

#[test]
fn administration_refresh_lives_in_header_without_loading_copy() {
    let ui = include_str!("../../../src/web/src/ui.rs");
    let disks = include_str!("../../../src/web/src/admin/disks.rs");
    assert!(ui.contains("Page::Admin => !admin::can_load_disks(state)"));
    assert!(ui.contains("disabled=move ||state.with(header_refresh_disabled)"));
    assert!(ui.contains("Page::Admin=>admin::load_disks(state)"));
    assert!(!disks.contains("Refresh disk list"));
    assert!(!disks.contains("LoadingState"));
    assert!(disks.contains("aria-busy="));
    assert!(
        disks.contains("<ErrorState message=feedback.message request_id=feedback.request_id/>")
    );
}

#[test]
fn refresh_ring_animation_uses_actual_pending_state_and_reduced_motion() {
    let ui = include_str!("../../../src/web/src/ui.rs");
    let css = include_str!("../../../src/web/public/styles/components/content-blocks.css");
    assert!(ui.contains("aria-busy=move ||state.with(header_refreshing).to_string()"));
    assert!(css.contains(".zxs-header-refresh[aria-busy=true] > .zxs-refresh-icon"));
    assert!(css.contains("@media (prefers-reduced-motion: reduce)"));
    assert!(
        css.contains(
            ".zxs-header-refresh[aria-busy=true] > .zxs-refresh-icon { animation: none; }"
        )
    );
}

#[test]
fn details_remove_return_row_and_its_exclusive_code() {
    let detail = include_str!("../../../src/web/src/files/detail.rs");
    let rows = include_str!("../../../src/web/src/files/rows.rs");
    let icons = include_str!("../../../src/web/src/icons.rs");
    let css = include_str!("../../../src/web/public/styles/components/file-manager.css");
    assert!(!detail.contains("Back to files"));
    assert!(!detail.contains("Page::Files"));
    assert!(!detail.contains("zxs-entry-navigation"));
    assert!(!css.contains("zxs-entry-navigation"));
    assert!(!icons.contains("\"back\" =>"));
    assert!(!rows.contains("selection_id"));
    assert!(detail.contains("join_path(")); // The useful Location property remains.
}

#[test]
fn main_navigation_has_no_separate_health_page() {
    let ui = include_str!("../../../src/web/src/ui.rs");
    let navigation = ui
        .split("<HeaderNavigation ")
        .nth(1)
        .unwrap()
        .split("</HeaderNavigation>")
        .next()
        .unwrap();
    assert!(!navigation.contains("Page::Health"));
    let admin = include_str!("../../../src/web/src/admin/mod.rs");
    assert!(admin.contains("crate::management::recovery(state)"));
    let mut last = 0;
    for page in ["Files", "Details", "Status", "History", "Admin"] {
        let position = navigation.find(&format!("Page::{page}")).unwrap();
        assert!(position > last, "{page} is not in the requested DOM order");
        last = position;
    }
}

#[test]
fn menu_refresh_pages_do_not_create_loading_text_rows() {
    for source in [
        include_str!("../../../src/web/src/files/view.rs"),
        include_str!("../../../src/web/src/files/detail.rs"),
        include_str!("../../../src/web/src/history.rs"),
        include_str!("../../../src/web/src/monitor.rs"),
        include_str!("../../../src/web/src/monitor/scrub.rs"),
        include_str!("../../../src/web/src/admin/disks.rs"),
    ] {
        assert!(!source.contains("LoadingState"));
        assert!(!source.contains("Loading…"));
        assert!(source.contains("aria-busy="));
    }
    let chart = include_str!("../../../src/web/src/monitor/chart.rs");
    assert!(!chart.contains("Loading history"));
    assert!(chart.contains("aria-busy="));
}

#[test]
fn history_disk_filter_removes_label_dom_but_keeps_control_names() {
    let history = include_str!("../../../src/web/src/history.rs");
    let select = include_str!("../../../src/interface/src/components/select.rs");
    assert!(history.contains("<Select label=t(\"硬盘\",\"Disk\") show_label=false"));
    assert!(select.contains("show_label.then(move ||view!{<label"));
    assert_eq!(
        select
            .matches("aria-label=(!show_label).then(||label.get_value())")
            .count(),
        2
    );
    assert_eq!(
        select
            .matches("aria-labelledby=show_label.then(||label_id.get_value())")
            .count(),
        2
    );
}

#[test]
fn empty_folder_message_is_rendered_inside_pagination() {
    let view = include_str!("../../../src/web/src/files/view.rs");
    let pagination = view
        .split("<nav class=\"zxs-file-pagination ")
        .nth(1)
        .unwrap()
        .split("</nav>")
        .next()
        .unwrap();
    assert!(pagination.contains("This folder is empty."));
    assert!(
        pagination.contains("<span class=\"zxs-file-empty\" role=\"status\" aria-live=\"polite\"")
    );
    assert_eq!(view.matches("This folder is empty.").count(), 1);
    assert!(
        pagination.contains("s.files.loaded&&s.files.entries.is_empty()&&s.files.query.is_empty()")
    );
    assert!(view.contains("No files match your search."));
}

#[test]
fn shared_form_groups_align_fields_without_individual_input_boxes() {
    let css = include_str!("../../../src/web/public/styles/components/base.css");
    assert!(css.contains(".zxs-form-grid {"));
    assert!(css.contains(".zxs-form-field {"));
    assert!(css.contains("align-items: center; column-gap:"));
    assert!(css.contains("border: 0; border-bottom: 1px dashed"));
    assert!(
        css.contains(
            ":root :is(input,textarea).zxs-input:focus-visible { border-bottom-width: 3px;"
        )
    );
    assert!(css.contains(
        "background: transparent; outline: 0; --zxs-contour-extent: 0px; --zxs-contour-mode: none;"
    ));
    assert!(!css.contains("@media (forced-colors: active)"));
    let geometry = include_str!("../../../src/web/public/styles/components/geometry.css");
    assert!(!geometry.contains(".zxs-input--underline"));
    assert!(!css.contains(".zxs-form-field + .zxs-form-field"));
    let login = include_str!("../../../src/interface/src/components/login.rs");
    let account = include_str!("../../../src/interface/src/components/account.rs");
    assert!(login.contains("data-auth-form=\"login\""));
    assert!(!login.contains("zxs-form-grid"));
    assert!(account.contains(".class(\"zxs-account-fields zxs-form-grid\")"));
    let account_css =
        include_str!("../../../src/web/public/styles/components/account-settings.css");
    assert!(!account_css.contains("grid-template-columns"));
    assert!(!account_css.contains("36rem"));
}

#[test]
fn smb_selection_and_passwords_share_aligned_form_groups() {
    let smb = include_str!("../../../src/web/src/smb_account.rs");
    let binding = smb
        .split("<div class=\"zxs-form-grid\">")
        .nth(1)
        .expect("SMB binding uses the shared field group")
        .split("</form>")
        .next()
        .unwrap();
    assert!(binding.contains("<div class=\"zxs-form-field\"><span"));
    assert!(binding.contains("<Select show_label=false"));
    assert!(binding.contains("name=\"smb_binding_administrator_password\""));
    assert_eq!(smb.matches("<div class=\"zxs-form-grid\">").count(), 2);
    let css = include_str!("../../../src/web/public/styles/components/base.css");
    assert!(css.contains(":root :is(input,textarea).zxs-input {"));
    assert!(css.contains("border: 0; border-bottom: 1px dashed"));
}
