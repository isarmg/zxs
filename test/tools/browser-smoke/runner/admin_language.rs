//! Read-only administration labels and real language-toggle session restoration.
use super::*;

const ACTIONS: [(&str, &str); 4] = [
    ("Repository", "仓库"),
    ("Path copy rule", "路径副本规则"),
    ("Placement rule", "主盘规则"),
    ("Inspect / query", "检查 / 查询"),
];
const QUERIES: [(&str, &str); 13] = [
    ("Repositories", "仓库列表"),
    ("Disks", "磁盘"),
    ("Integrity check", "完整性检查"),
    ("Repository", "仓库详情"),
    ("Operation", "操作"),
    ("Plan", "计划"),
    ("Branch tree", "分支树"),
    ("Branch details", "分支详情"),
    ("File history", "文件历史"),
    ("History graph", "历史图"),
    ("Policy", "副本规则"),
    ("Placement rule", "主盘规则"),
    ("Topology", "拓扑"),
];

fn labels(pairs: &[(&str, &str)], locale: &str) -> Vec<String> {
    pairs
        .iter()
        .map(|(en, zh)| if locale == "en" { en } else { zh }.to_string())
        .collect()
}
fn locale(browser: &Browser) -> Result<String> {
    match browser
        .script("return document.documentElement.lang")?
        .as_str()
    {
        Some(value @ ("en" | "zh-CN")) => Ok(value.to_owned()),
        value => Err(format!("unexpected document locale: {value:?}").into()),
    }
}
fn visible_text(browser: &Browser, page: AdminPage, selector: &str) -> Result<Value> {
    browser.script(&format!(
        "const section=document.querySelector({}); return [...section.querySelectorAll({})].filter(e=>e.getClientRects().length).map(e=>e.textContent.trim());",
        json!(page.section()), json!(selector),
    ))
}
fn expect_labels(
    browser: &Browser,
    page: AdminPage,
    selector: &str,
    expected: &[(&str, &str)],
    locale: &str,
) -> Result<()> {
    let expected = json!(labels(expected, locale));
    wait_for("localized administration labels", || {
        let actual = visible_text(browser, page, selector)?;
        if actual != expected {
            return Err(
                format!("admin {locale} {selector}: expected {expected}, got {actual}").into(),
            );
        }
        Ok(())
    })
}
fn actions(page: AdminPage) -> &'static [(&'static str, &'static str)] {
    match page {
        AdminPage::Storage => &[],
        AdminPage::Repositories => &ACTIONS[..1],
        AdminPage::Rules => &ACTIONS[1..3],
        AdminPage::Diagnostics => &ACTIONS[3..],
    }
}
fn select_action(browser: &Browser, page: AdminPage, en: &str, zh: &str) -> Result<()> {
    let button = browser.element("xpath", &format!(
        "//section[@data-zxs-admin-section]/nav//button[normalize-space(.)='{en}' or normalize-space(.)='{zh}']"
    ))?;
    browser.click(&button)?;
    browser.css(&format!("{} .zxs-form-grid", page.section()))?;
    Ok(())
}
// Hold the real list body to observe pending UI and the production read queue.
fn refresh_disks_from_header(browser: &Browser) -> Result<()> {
    browser.css(".zxs-header-refresh:not([disabled])")?;
    if browser.script("return [...document.querySelectorAll('.zxs-disk-management button')].some(e=>['Refresh disk list','刷新硬盘列表'].includes(e.textContent.trim()))")? != false {
        return Err("duplicate disk-list refresh entry remains in administration".into());
    }
    browser.script(include_str!("admin_read_queue.js"))?;
    with_cleanup(
        || {
            browser.cdp(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-reduced-motion","value":"no-preference"}]}),
            )?;
            browser.click(&browser.css(".zxs-header-refresh")?)?;
            wait_for(
                "real admin refresh held without visible loading copy",
                || {
                    if browser.script("return window.__zxsAdminRefresh.held&&window.__zxsAdminRefresh.count===1&&document.querySelector('.zxs-header-refresh').disabled&&document.querySelector('.zxs-header-refresh').getAttribute('aria-busy')==='true'&&getComputedStyle(document.querySelector('.zxs-refresh-icon')).animationName==='zxs-refresh-spin'&&document.querySelector('.zxs-disk-management').getAttribute('aria-busy')==='true'&&!document.querySelector('.zxs-disk-management .zxs-loading')")? == true { Ok(()) } else { Err("header refresh state or loading-copy suppression is incorrect".into()) }
                },
            )?;
            wait_for(
                "management reads wait until the held response body finishes",
                || {
                    let evidence = browser.script("const p=window.__zxsAdminRefresh;return {ready:p.framesPassed,peak:p.peak,started:p.paths.length,atHold:p.atHold,failures:p.failures}")?;
                    if evidence["ready"] == true
                        && evidence["peak"] == 1
                        && evidence["started"] == evidence["atHold"]
                        && evidence["failures"] == json!([])
                    {
                        Ok(())
                    } else {
                        Err(format!("management body admission evidence: {evidence}").into())
                    }
                },
            )?;
            browser.cdp(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-reduced-motion","value":"reduce"}]}),
            )?;
            wait_for(
                "reduced-motion refresh remains busy without rotation",
                || {
                    if browser.script("return document.querySelector('.zxs-header-refresh').getAttribute('aria-busy')==='true'&&getComputedStyle(document.querySelector('.zxs-refresh-icon')).animationName==='none'")? == true { Ok(()) } else { Err("refresh animation ignores reduced motion".into()) }
                },
            )?;
            browser.cdp(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-reduced-motion","value":"no-preference"}]}),
            )?;
            browser.script("window.__zxsAdminRefresh.release();return true")?;
            browser.css(".zxs-header-refresh:not([disabled])")?;
            if browser.script("return document.querySelector('.zxs-header-refresh').getAttribute('aria-busy')==='false'&&getComputedStyle(document.querySelector('.zxs-refresh-icon')).animationName==='none'")? != true {
                return Err("refresh ring did not stop after the list and detail reads settled".into());
            }
            wait_for(
                "all management refresh reads drain without 429 or overlap",
                || {
                    let evidence = browser.script(r#"
                    const p=window.__zxsAdminRefresh;
                    const required=['/api/admin/storage/status','/api/admin/storage/discovered-disks',
                        '/api/admin/disks','/api/admin/disks/registrations','/api/admin/disks/addition',
                        '/api/admin/storage/conversion/status','/api/admin/disks/exit','/api/admin/disks/readmission'];
                    return {active:p.active,peak:p.peak,missing:required.filter(path=>!p.paths.includes(path)),failures:p.failures};
                "#)?;
                    if evidence["active"] == 0
                        && evidence["peak"] == 1
                        && evidence["missing"] == json!([])
                        && evidence["failures"] == json!([])
                    {
                        Ok(())
                    } else {
                        Err(format!("management refresh did not drain: {evidence}").into())
                    }
                },
            )?;
            Ok(())
        },
        || {
            let restore = browser.script("window.__zxsAdminRefresh.restore();return true");
            let media = browser.cdp("Emulation.setEmulatedMedia", json!({"features":[]}));
            restore?;
            media?;
            Ok(())
        },
    )
}

fn retired_controls_absent(browser: &Browser, page: AdminPage) -> Result<()> {
    if browser.script(&format!(
        "return [...document.querySelector({}).querySelectorAll('button,label')].some(e=>['SMB binding','SMB 绑定','Enable SMB','启用 SMB','History branch','历史分支','Merge link','合并连线','Purge preview','清理预览','Split preview','拆分预览','Historical save preview','历史保存预览'].includes(e.textContent.trim()))",
        json!(page.section()),
    ))? != false {
        return Err(format!("obsolete creation or SMB controls remain on {page:?}").into());
    }
    Ok(())
}
fn open_console(browser: &Browser, page: AdminPage, locale: &str) -> Result<()> {
    page.open(browser)?;
    expect_labels(browser, page, "nav button", actions(page), locale)?;
    retired_controls_absent(browser, page)
}
fn inspect_queries(browser: &Browser, locale: &str) -> Result<()> {
    let page = AdminPage::Diagnostics;
    open_console(browser, page, locale)?;
    select_action(browser, page, "Inspect / query", "检查 / 查询")?;
    accessibility::input_surfaces(browser)?;
    expect_labels(
        browser,
        page,
        ":scope > div.zxs-flow > button",
        &QUERIES,
        locale,
    )?;
    expect_labels(
        browser,
        page,
        ".zxs-form-grid label > span",
        &[
            ("Repository ID", "仓库 ID"),
            (
                "Operation / plan / policy / topology ID",
                "操作 / 计划 / 策略 / 拓扑 ID",
            ),
        ],
        locale,
    )
}
fn inspect_repository(browser: &Browser, locale: &str) -> Result<()> {
    let page = AdminPage::Repositories;
    open_console(browser, page, locale)?;
    select_action(browser, page, "Repository", "仓库")?;
    expect_labels(
        browser,
        page,
        ".zxs-form-grid label > span",
        &[
            ("Name", "名称"),
            ("Primary slot", "主盘槽位"),
            (
                "Copies including primary; 1 has no redundancy",
                "份数（含主盘；1 份无冗余）",
            ),
        ],
        locale,
    )?;
    expect_labels(
        browser,
        page,
        ":scope > button",
        &[("Save configuration", "保存配置")],
        locale,
    )?;
    accessibility::input_surface_viewports(browser)
}
fn inspect_rules(browser: &Browser, locale: &str) -> Result<()> {
    let page = AdminPage::Rules;
    open_console(browser, page, locale)?;
    for (action, id, target) in [
        (
            ACTIONS[1],
            ("Path rule ID", "路径规则 ID"),
            (
                "Copies including primary; 1 has no redundancy",
                "份数（含主盘；1 份无冗余）",
            ),
        ),
        (
            ACTIONS[2],
            ("Rule ID", "规则 ID"),
            ("Primary slot", "主盘槽位"),
        ),
    ] {
        select_action(browser, page, action.0, action.1)?;
        expect_labels(
            browser,
            page,
            ".zxs-form-grid label > span",
            &[
                id,
                ("Repository ID", "仓库 ID"),
                ("Path prefix", "路径前缀"),
                target,
                ("Expected generation (0 for new)", "预期代数（新建填 0）"),
            ],
            locale,
        )?;
        expect_labels(
            browser,
            page,
            ":scope > button",
            &[("Save configuration", "保存配置")],
            locale,
        )?;
        accessibility::input_surface_viewports(browser)?;
    }
    Ok(())
}
fn inspect(browser: &Browser, locale: &str) -> Result<()> {
    AdminPage::Storage.open(browser)?;
    refresh_disks_from_header(browser)?;
    retired_controls_absent(browser, AdminPage::Storage)?;
    // Only select local forms. Never submit a mutation, query or checkbox.
    inspect_queries(browser, locale)?;
    inspect_repository(browser, locale)?;
    inspect_rules(browser, locale)
}
fn switch(browser: &Browser, target: &str) -> Result<()> {
    // Reload must not hide failures collected earlier in the current document.
    if browser.script("return window.__zxsSmokeErrors")? != json!([]) {
        return Err("browser errors before administration language reload".into());
    }
    let previous_document = browser.script("return performance.timeOrigin")?;
    if browser.script("return document.querySelector('[data-language-toggle]')?.disabled")? != false
    {
        return Err("language guard unexpectedly blocks read-only administration".into());
    }
    browser.click(&browser.css("[data-language-toggle]")?)?;
    wait_for(
        "language reload and authenticated Files restoration",
        || {
            if browser.script(&format!(
            "return performance.timeOrigin !== {} && document.documentElement.lang === {} && new URL(location.href).searchParams.get('lang') === {} && !!document.querySelector('.zxs-disk-entry[data-slot=\"P01\"]:not([disabled])') && !document.querySelector('input[autocomplete=\"current-password\"]');",
            previous_document, json!(target), json!(target),
        ))? == true { Ok(()) } else { Err("language/session restoration pending".into()) }
        },
    )
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    let original = locale(browser)?;
    let other = if original == "en" { "zh-CN" } else { "en" };
    with_cleanup(
        || {
            inspect(browser, &original)?;
            switch(browser, other)?;
            inspect(browser, other)?;
            Ok(())
        },
        || {
            if locale(browser)? != original {
                switch(browser, &original)?;
            }
            browser.click(&browser.button("Files", "文件")?)?;
            browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
            Ok(())
        },
    )?;
    println!(
        "PASS four administration pages, four current actions, 13 queries and localized forms in both languages; real session reload restores original locale and Files"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/admin_language_tests.rs"]
mod tests;
