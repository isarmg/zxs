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
fn anonymous_forms_reuse_six_shared_height_rows_and_original_card_corner() {
    let css = include_str!("../../../src/web/public/styles/components/auth.css");
    assert!(css.contains("aspect-ratio: 3 / 2;"));
    assert!(css.contains(
        "grid-template-rows: repeat(6, minmax(var(--zxs-compact-row-height), max-content))"
    ));
    assert!(css.contains("width: min(calc(var(--zxs-compact-row-height) * 9), 100%)"));
    assert!(css.contains("aspect-ratio: 3 / 2; min-height: max-content;"));
    assert!(css.contains(".authentication-card:has(.zxs-error) { aspect-ratio: auto; }"));
    assert!(!css.contains("min-height: min-content;"));
    assert!(css.contains("--zxs-contour-extent: 48px"));
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
    assert!(css.contains(".authentication-shell .authentication-card input.zxs-input {"));
    assert!(
        css.contains(
            "padding: 0; border: 0; border-inline-start: 2px solid transparent; border-radius: 0; background: transparent; outline: 0;"
        )
    );
    assert!(css.contains(
        "input.zxs-input:focus-visible { border-inline-start-color: var(--zxs-focus,Highlight); }"
    ));
    assert!(!css.contains("dashed"));
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
    assert!(setup.contains("Set a name and password."));
    assert!(!setup.contains("Please set an account name and password."));
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
    assert!(css.contains("left: var(--zxs-file-scroll-left); width: 100cqw;"));
    assert!(!css.contains(".zxs-file-row-decoration::after"));
    assert!(!css.contains(".zxs-file-pagination::before"));
    assert!(css.contains("--zxs-contour-extent: var(--zxs-contour-panel);"));
    assert!(css.contains("tr[aria-selected=true] > td > .zxs-file-row-decoration > .zxs-file-row-surface { opacity: 1; }"));
    assert!(css.contains(
        "tr:hover > td > .zxs-file-row-decoration > .zxs-file-row-surface { opacity: 1; }"
    ));
    assert_eq!(
        view.matches("<span class=\"zxs-file-row-surface\" aria-hidden=\"true\"></span>")
            .count(),
        2
    );
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
fn main_navigation_splits_administration_and_keeps_health_in_diagnostics() {
    let ui = include_str!("../../../src/web/src/ui.rs");
    let navigation = ui
        .split("<HeaderNavigation ")
        .nth(1)
        .unwrap()
        .split("</HeaderNavigation>")
        .next()
        .unwrap();
    assert!(!ui.contains("Page::Health"));
    let (main_navigation, management_navigation) = navigation
        .split_once("administration_pages().into_iter()")
        .expect("the declared management pages must actually render in the main navigation");
    let mut last = 0;
    for page in ["Files", "Details", "Status", "History"] {
        let position = main_navigation.find(&format!("Page::{page}")).unwrap();
        assert!(position > last, "{page} is not in the requested DOM order");
        last = position;
    }
    assert!(main_navigation.contains("<Show when=move ||state.with(|s|s.session_active())>"));
    assert!(management_navigation.contains("data-zxs-management-menu=\"true\""));
    assert!(management_navigation.contains("navigate(state,page)"));

    let administration = ui
        .split_once("fn administration_pages()")
        .unwrap()
        .1
        .split_once("fn workspace(")
        .unwrap()
        .0;
    let pages = administration
        .split("Page::")
        .skip(1)
        .map(|page| {
            page.split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(pages, ["Admin", "Repositories", "Rules", "Diagnostics"]);
    for label in ["磁盘与接入", "仓库", "备份与放置", "恢复与检查"] {
        assert!(
            administration.contains(label),
            "missing management label {label}"
        );
    }
    let admin = include_str!("../../../src/web/src/admin/mod.rs");
    assert!(admin.contains(
        "(section==Section::Diagnostics).then(move ||crate::management::recovery(state))"
    ));
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
    assert!(!css.contains("dashed"));
    assert!(
        css.contains(
            ":root :is(input,textarea).zxs-input:focus-visible { border-inline-start-color: var(--zxs-focus,Highlight); }"
        )
    );
    assert!(css.contains(
        "background: transparent; outline: 0; --zxs-contour-extent: 0px; --zxs-contour-mode: none;"
    ));
    assert!(css.contains("@media (forced-colors: active)"));
    assert!(css.contains(
        ":focus-visible:not(.zxs-file-search > input) { border-inline-start-color: Highlight; }"
    ));
    assert!(css.contains("border-inline-start: 2px solid transparent;"));
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
    assert!(!css.contains("dashed"));
}

#[test]
fn folder_search_uses_one_input_surface_and_explicit_enter_submission() {
    let view = include_str!("../../../src/web/src/files/view.rs");
    let search = view
        .split("<label class=\"zxs-file-search zxs-form-grid\">")
        .nth(1)
        .expect("full-width search label")
        .split("</label>")
        .next()
        .unwrap();
    assert!(!search.contains("<span") && !search.contains("zxs-form-field"));
    assert!(search.contains("aria-label=t(\"搜索当前文件夹\",\"Search this folder\")"));
    assert!(search.contains("placeholder=t(\"搜索当前文件夹\",\"Search this folder\")"));
    assert!(search.contains("on:input=move |event|actions::edit_search"));
    assert!(search.contains("event.key()==\"Enter\"&&!event.is_composing()"));
    assert!(search.contains("if !event.repeat()"));
    assert!(search.contains("actions::search(state,query)"));
    let css = include_str!("../../../src/web/public/styles/components/file-manager.css");
    assert!(
        css.contains(".zxs-file-search > input.zxs-input:focus-visible { border: 0; outline: 0; }")
    );
    let focus = css
        .split(".zxs-file-search:has(> input:focus-visible) {")
        .nth(1)
        .expect("search focus paint")
        .split('}')
        .next()
        .unwrap();
    assert!(focus.contains("outline: 0;"));
    assert!(focus.contains("--zxs-contour-mode: paint;"));
    assert!(focus.contains("box-shadow: 0 0 0 3px"));
    assert!(focus.contains("0 0 0 5px var(--zxs-focus, Highlight)"));
    assert!(css.contains("@media (forced-colors: active)"));
    assert!(css.contains("outline: 2px solid Highlight; outline-offset: 3px; box-shadow: none;"));
}

fn style_rule<'a>(css: &'a str, selector: &str) -> &'a str {
    let opening = format!("{selector} {{");
    let (start, _) = css
        .match_indices(&opening)
        .find(|(start, _)| css[..*start].rsplit('\n').next().unwrap().trim().is_empty())
        .unwrap_or_else(|| panic!("missing style rule: {selector}"));
    css[start + opening.len()..].split('}').next().unwrap()
}

#[test]
fn decorative_borders_are_exclusive_to_the_expanded_select_card() {
    let base = include_str!("../../../src/web/public/styles/components/base.css");
    let geometry = include_str!("../../../src/web/public/styles/components/geometry.css");
    let select = include_str!("../../../src/web/public/styles/components/select.css");
    let files = include_str!("../../../src/web/public/styles/components/file-manager.css");
    let history = include_str!("../../../src/web/public/styles/components/history.css");
    for (css, selector) in [
        (base, ".zxs-button,.zxs-input"),
        (base, ".zxs-form-grid"),
        (base, ".zxs-error"),
        (geometry, ".zxs-app-shell .zxs-frame"),
        (select, ":root .zxs-select-trigger.zxs-input"),
        (files, ".zxs-app-shell .zxs-node-tag"),
    ] {
        assert!(
            style_rule(css, selector).contains("border: 0;"),
            "{selector}"
        );
    }
    let panel = style_rule(select, ".zxs-select .zxs-select-panel");
    assert!(panel.contains("border: var(--zxs-select-border-width) solid color-mix(in srgb, var(--zxs-bg-panel) 85%, #000);"));
    assert!(panel.contains("--zxs-row-border: var(--zxs-select-border-width);"));
    assert!(!select.contains("border-color: #000"));
    assert!(geometry.contains("--zxs-select-border-width: 2px;"));
    assert!(panel.contains("background: var(--zxs-bg-panel, #fff);"));
    assert!(!files.contains("border-block") && !history.contains("border-block"));
    assert!(select.contains(":focus-visible"));
    assert!(select.contains("outline: 2px solid Highlight;"));
}

#[test]
fn shared_rows_own_one_inset_without_wrapping_controls_in_extra_row_padding() {
    let base = include_str!("../../../src/web/public/styles/components/base.css");
    let geometry = include_str!("../../../src/web/public/styles/components/geometry.css");
    let files = include_str!("../../../src/web/public/styles/components/file-manager.css");
    let management = include_str!("../../../src/web/src/management.rs");
    assert!(
        style_rule(base, ".zxs-form-field")
            .contains("padding-inline: var(--zxs-list-content-inset,.75rem);")
    );
    assert!(!base.contains(".zxs-form-grid > .zxs-form-field"));
    assert!(
        style_rule(base, ".zxs-app-shell .zxs-form-field > .zxs-row")
            .contains("padding-inline: 0;")
    );
    assert!(style_rule(geometry, ".zxs-app-shell .zxs-flow.zxs-row").contains("padding-block: 0;"));
    assert!(style_rule(files, ".zxs-file-properties dd").contains("margin: 0;"));
    assert!(!management.contains("class=\"zxs-flow zxs-row\""));
    assert!(geometry.contains("--zxs-compact-row-height: 36px;"));
    assert!(geometry.contains("--zxs-input-row-height: var(--zxs-compact-row-height);"));
}

#[test]
fn select_option_hover_is_scoped_to_the_open_menu() {
    let appearance = include_str!("../../../src/web/public/styles/components/content-blocks.css");
    let files = include_str!("../../../src/web/public/styles/components/file-manager.css");
    let select = include_str!("../../../src/web/public/styles/components/select.css");
    assert!(!appearance.contains("background: var(--zxs-row-hover-bg);"));
    assert!(!appearance.contains("--zxs-contour-mode: paint;"));
    assert!(select.contains("@media (hover: hover) and (pointer: fine)"));
    let hover = style_rule(
        select,
        ".zxs-select-panel:popover-open .zxs-select-option[data-active=true][aria-disabled=false]:hover",
    );
    assert!(hover.contains("background: var(--zxs-row-hover-bg);"));
    assert!(hover.contains("--zxs-contour-mode: paint;"));
    assert!(!hover.contains("border-radius:") && !hover.contains("border:"));
    assert!(
        style_rule(files, ".zxs-file-row-surface").contains("background: var(--zxs-row-hover-bg);")
    );
    assert!(
        style_rule(select, ".zxs-select:has(> .zxs-select-trigger:focus-visible) .zxs-select-panel:popover-open .zxs-select-option[data-active=true]")
            .contains("--zxs-contour-mode: paint;")
    );
}

#[test]
fn account_form_keeps_unknown_outcomes_disabled_without_a_false_busy_indicator() {
    let ui = include_str!("../../../src/web/src/ui.rs");
    let account = include_str!("../../../src/interface/src/components/account.rs");
    assert!(ui.contains("let pending = Memo::new(move |_| state.with(State::password_pending));"));
    assert!(ui.contains(
        "let disabled = Memo::new(move |_| state.with(State::password_change_blocked));"
    ));
    assert!(ui.contains("pending=pending.into() disabled=disabled.into()"));
    assert!(account.contains("if pending.get_untracked() || disabled.get_untracked()"));
    assert_eq!(
        account.matches("pending.get() || disabled.get()").count(),
        4
    );
    assert!(account.contains("pending.get().to_string()"));
    assert!(account.contains("if pending.get()"));
}

#[test]
fn anonymous_input_hover_hosts_preserve_the_six_rows_and_native_controls() {
    for (source, prefix) in [
        (
            include_str!("../../../src/interface/src/components/login.rs"),
            "login",
        ),
        (include_str!("../../../src/web/src/setup.rs"), "setup"),
    ] {
        let hosts = source
            .split("<div class=\"zxs-auth-input-row zxs-hover-row\">")
            .skip(1)
            .map(|part| part.split("</div>").next().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(hosts.len(), 2);
        for (host, field) in hosts.iter().zip(["username", "password"]) {
            assert_eq!(host.matches("<input ").count(), 1);
            assert!(!host.contains("<label") && !host.contains("<button"));
            assert!(host.contains(&format!("id=\"zxs-{prefix}-{field}\"")));
            assert!(source.contains(&format!("for=\"zxs-{prefix}-{field}\"")));
            assert!(host.contains(&format!("name=\"{field}\"")));
            assert!(host.contains("class=\"zxs-input\""));
        }
    }
}

#[test]
fn select_ring_and_fill_stay_with_the_card_while_only_options_scroll() {
    let css = include_str!("../../../src/web/public/styles/components/select.css");
    let trigger = style_rule(css, ":root .zxs-select-trigger.zxs-input");
    assert!(trigger.contains("background: var(--zxs-bg-panel);"));
    assert!(trigger.contains("border: 0;"));
    let panel = style_rule(css, ".zxs-select .zxs-select-panel");
    assert!(panel.contains("overflow: visible;"));
    assert!(!panel.contains("overflow: auto;"));
    assert!(panel.contains("border: var(--zxs-select-border-width) solid color-mix(in srgb, var(--zxs-bg-panel) 85%, #000);"));
    let scroller = style_rule(css, ".zxs-select-options");
    assert!(scroller.contains("position: relative; min-height: 0; overflow: auto;"));
    assert!(scroller.contains("overscroll-behavior: contain;"));
    let shared_box = style_rule(
        css,
        ".zxs-select-panel[data-zxs-contour-state=clip] > .zxs-select-options",
    );
    assert!(shared_box.contains("margin: calc(0px - var(--zxs-select-border-width));"));
    assert!(shared_box.contains("--zxs-row-border: 0px;"));
    let component = include_str!("../../../src/interface/src/components/select.rs");
    assert!(component.contains("data-zxs-contour-foreground-anchor=\"local\""));
    assert!(component.contains("<div class=\"zxs-select-options\" node_ref=scroller>"));
    assert!(component.contains("scroller.set_scroll_top(bottom - scroller.client_height())"));
    assert!(!component.contains("panel.set_scroll_top("));
    let controller = include_str!("../../../src/interface/src/contours/controller.rs");
    assert!(
        controller.contains("set_css_text(&paint::foreground_style(width, height, &css, local))")
    );
    assert!(controller.contains("== \"fixed\""));
}
