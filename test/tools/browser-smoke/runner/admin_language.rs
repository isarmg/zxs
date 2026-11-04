//! Read-only administration labels and real language-toggle session restoration.
use super::*;

pub(super) const SECTION: &str =
    "section[aria-label='Daily administration'], section[aria-label='日常管理']";

const ACTIONS: [(&str, &str); 10] = [
    ("Repository", "仓库"),
    ("SMB binding", "SMB 绑定"),
    ("Path copy rule", "路径副本规则"),
    ("Placement rule", "主盘规则"),
    ("History branch", "历史分支"),
    ("Merge link", "合并连线"),
    ("Purge preview", "清理预览"),
    ("Split preview", "拆分预览"),
    ("Historical save preview", "历史保存预览"),
    ("Inspect / query", "检查 / 查询"),
];
const QUERIES: [(&str, &str); 14] = [
    ("Repositories", "仓库列表"),
    ("Disks", "磁盘"),
    ("Integrity check", "完整性检查"),
    ("Repository", "仓库详情"),
    ("SMB binding", "SMB 绑定"),
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
fn visible_text(browser: &Browser, selector: &str) -> Result<Value> {
    browser.script(&format!(
        "const section=document.querySelector({}); return [...section.querySelectorAll({})].filter(e=>e.getClientRects().length).map(e=>e.textContent.trim());",
        json!(SECTION), json!(selector),
    ))
}
fn expect_labels(
    browser: &Browser,
    selector: &str,
    expected: &[(&str, &str)],
    locale: &str,
) -> Result<()> {
    let expected = json!(labels(expected, locale));
    wait_for("localized administration labels", || {
        let actual = visible_text(browser, selector)?;
        if actual != expected {
            return Err(
                format!("admin {locale} {selector}: expected {expected}, got {actual}").into(),
            );
        }
        Ok(())
    })
}
fn select_action(browser: &Browser, en: &str, zh: &str) -> Result<()> {
    let button = browser.element("xpath", &format!(
        "//section[@aria-label='Daily administration' or @aria-label='日常管理']/nav//button[normalize-space(.)='{en}' or normalize-space(.)='{zh}']"
    ))?;
    browser.click(&button)
}
// Hold only delivery of the real list Response to observe the pending UI.
fn refresh_disks_from_header(browser: &Browser) -> Result<()> {
    browser.css(".zxs-header-refresh:not([disabled])")?;
    if browser.script("return [...document.querySelectorAll('.zxs-disk-management button')].some(e=>['Refresh disk list','刷新硬盘列表'].includes(e.textContent.trim()))")? != false {
        return Err("duplicate disk-list refresh entry remains in administration".into());
    }
    browser.script(r#"
        const original=window.fetch;
        let release; const gate=new Promise(resolve=>release=resolve);
        window.__zxsAdminRefresh={count:0,held:false,release,restore:()=>{release();window.fetch=original;delete window.__zxsAdminRefresh;}};
        window.fetch=async function(...args){
            const result=await Reflect.apply(original,this,args),request=args[0];
            const url=new URL(request instanceof Request?request.url:request,location.href);
            if(url.origin===location.origin&&url.pathname==='/api/admin/disks'){
                window.__zxsAdminRefresh.count++;window.__zxsAdminRefresh.held=true;await gate;
            }
            return result;
        };return true;
    "#)?;
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

fn inspect(browser: &Browser, locale: &str) -> Result<()> {
    browser.click(&browser.button("Administration", "系统管理")?)?;
    browser.css(SECTION)?;
    refresh_disks_from_header(browser)?;
    select_action(browser, "Inspect / query", "检查 / 查询")?;
    expect_labels(browser, "nav button", &ACTIONS, locale)?;
    expect_labels(browser, ":scope > button", &QUERIES, locale)?;
    expect_labels(
        browser,
        "label > span",
        &[
            ("Repository ID", "仓库 ID"),
            (
                "Operation / plan / policy / topology ID",
                "操作 / 计划 / 策略 / 拓扑 ID",
            ),
        ],
        locale,
    )?;
    // These navigation buttons only select local forms. Never submit a mutation
    // or click a query/checkbox, even when the disposable fixture is writable.
    select_action(browser, "Repository", "仓库")?;
    expect_labels(
        browser,
        "label > span",
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
        ":scope > button",
        &[("Save configuration / Create preview", "保存配置 / 创建预览")],
        locale,
    )?;
    select_action(browser, "SMB binding", "SMB 绑定")?;
    expect_labels(
        browser,
        "label > span",
        &[
            ("Repository ID", "仓库 ID"),
            ("Branch ID", "分支 ID"),
            ("Expected history generation", "预期历史代数"),
        ],
        locale,
    )?;
    expect_labels(
        browser,
        "label.zxs-row",
        &[("Enable SMB", "启用 SMB")],
        locale,
    )?;
    Ok(())
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
        "PASS administration has 10 actions, 14 queries and localized forms in both languages; real session reload restores original locale and Files"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/admin_language_tests.rs"]
mod tests;
