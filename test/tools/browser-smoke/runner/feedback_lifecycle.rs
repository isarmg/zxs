//! Real folder-branch receipts exercise workspace notification ownership.
//! Only final delivery is delayed in the late-response case.
use super::*;

const BRANCH: &str = "browser-notice-lifecycle";
const NOTICE: &str = ".zxs-inline-feedback > .zxs-status";

fn check(browser: &Browser, label: &str, script: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(label.to_owned().into())
        }
    })
}

fn no_notice(browser: &Browser) -> Result<()> {
    if browser.script(&format!(
        "return !document.querySelector({})",
        json!(NOTICE)
    ))? == true
    {
        Ok(())
    } else {
        Err("operation notice survived navigation or dismissal".into())
    }
}

fn branch(browser: &Browser, select: bool) -> Result<()> {
    let action = if select {
        ("Switch folder branch", "切换目录分支")
    } else {
        ("Create folder branch", "创建目录分支")
    };
    check(
        browser,
        "folder branch action enabled",
        &format!(
            "const labels={},button=Array.from(document.querySelectorAll('button')).find(button=>labels.includes(button.textContent.trim()));return button&&!button.disabled",
            json!([action.0, action.1]),
        ),
    )?;
    browser.click(&browser.button(action.0, action.1)?)?;
    let input = browser.css(".zxs-branch-name:not([disabled])")?;
    browser.keys(&input, BRANCH)?;
    browser.keys(&input, "\u{e007}")?;
    Ok(())
}

fn restored(browser: &Browser) -> Result<()> {
    check(
        browser,
        "scoped directory restoration success",
        &format!(
            r#"
        const notice=document.querySelector({});
        return notice?.getAttribute('role')==='status'
            &&/Restored this folder|已恢复此目录/.test(notice.textContent)
            &&!document.querySelector('.zxs-branch-name');
    "#,
            json!(NOTICE)
        ),
    )
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    browser.click(&browser.element("xpath", "//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-fixtures']]//button[normalize-space(.)='Details' or normalize-space(.)='详情']")?)?;
    branch(browser, false)?;
    check(
        browser,
        "real folder branch registration",
        "return !document.querySelector('.zxs-branch-name')&&/registered without switching|已登记，尚未切换/.test(document.querySelector('.zxs-inline-feedback > .zxs-status')?.textContent||'')",
    )?;
    branch(browser, true)?;
    restored(browser)?;
    for (english, chinese) in [
        ("Status", "状态"),
        ("History", "历史记录"),
        ("Administration", "系统管理"),
        ("Account settings", "账号设置"),
        ("Files", "文件"),
    ] {
        browser.click(&browser.button(english, chinese)?)?;
        no_notice(browser)?;
        browser.click(&browser.button("Details", "详情")?)?;
        no_notice(browser)?;
    }
    branch(browser, true)?;
    restored(browser)?;
    browser.click(&browser.button("Dismiss operation notice", "关闭操作提示")?)?;
    no_notice(browser)?;
    branch(browser, true)?;
    restored(browser)?;
    let started = Instant::now();
    check(
        browser,
        "ordinary success expires",
        &format!("return !document.querySelector({})", json!(NOTICE)),
    )?;
    if !(Duration::from_secs(3)..=Duration::from_secs(8)).contains(&started.elapsed()) {
        return Err("ordinary success exceeded its five-second lifetime tolerance".into());
    }
    browser.script(r#"
        const fetch=window.fetch;
        const probe=window.__zxsNoticeDelivery={release:null,delivered:false,restore:()=>{window.fetch=fetch;if(probe.release)probe.release();}};
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            const response=await fetch.apply(this,args);
            if(url.origin===location.origin&&request.method==='POST'&&url.pathname.endsWith('/directory-select')){
                if(!response.ok)throw new Error('real folder selection failed');
                await new Promise(resolve=>probe.release=resolve);
                probe.delivered=true;
            }
            return response;
        };return true;
    "#)?;
    with_cleanup(
        || {
            branch(browser, true)?;
            check(
                browser,
                "real folder receipt awaiting delivery",
                "return typeof window.__zxsNoticeDelivery.release==='function'",
            )?;
            browser.click(&browser.button("History", "历史记录")?)?;
            browser.script("window.__zxsNoticeDelivery.release();return true")?;
            check(
                browser,
                "late folder receipt delivered",
                "return window.__zxsNoticeDelivery.delivered",
            )?;
            no_notice(browser)?;
            browser.click(&browser.button("Details", "详情")?)?;
            check(
                browser,
                "late receipt settled without reopening its old notice",
                r#"
            const button=Array.from(document.querySelectorAll('button')).find(button=>['Switch folder branch','切换目录分支'].includes(button.textContent.trim()));
            return button&&!button.disabled&&!document.querySelector('.zxs-inline-feedback > .zxs-status');
        "#,
            )?;
            Ok(())
        },
        || {
            browser.script("if(window.__zxsNoticeDelivery){window.__zxsNoticeDelivery.restore();delete window.__zxsNoticeDelivery;}return true")?;
            Ok(())
        },
    )?;
    browser.click(&browser.button("Files", "文件")?)?;
    open_fixture_directory(browser)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    println!(
        "PASS real folder restore notices: page ownership, dismissal, repeated action, five-second expiry and late receipt after navigation"
    );
    Ok(())
}
