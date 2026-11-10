//! Real default-branch receipts exercise workspace notification ownership.
//! Browsing is observed without writes; only final receipt delivery is delayed.
use super::*;

const BRANCH: &str = "browser-notice-lifecycle";
const NOTICE: &str = ".zxs-inline-feedback > .zxs-status";
const DEFAULT: &str = "button[data-zxs-set-directory-default='true']";
const BROWSE: &str = "//div[contains(@class,'zxs-select')][./label[normalize-space(.)='Browse folder branch' or normalize-space(.)='浏览目录分支']]/button[@role='combobox' and not(@disabled)]";
const DEFAULT_SUCCESS_EN: &str = "The default view is updated. Enabled SMB shares use this view.";
const DEFAULT_SUCCESS_ZH: &str = "默认读取视图已更新；已启用的 SMB 共享使用该视图。";

const OBSERVE_FETCH: &str = r#"
    const original=window.fetch;
    const probe=window.__zxsNoticeDelivery={writes:[],previews:0,defaultReads:0,delay:false,release:null,delivered:false,
        restore:()=>{probe.delay=false;window.fetch=original;if(probe.release)probe.release();}};
    window.fetch=async function(...args){
        const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
        const method=(args[1]?.method||request.method||'GET').toUpperCase();
        const local=url.origin===location.origin&&url.pathname.startsWith('/api/');
        const publication=local&&method==='PUT'&&/^\/api\/repositories\/[^/]+\/default-branch$/.test(url.pathname);
        if(local&&!['GET','HEAD'].includes(method))probe.writes.push({method,path:url.pathname});
        const response=await original.apply(this,args);
        if(local&&method==='GET'&&url.pathname.endsWith('/entries')&&url.searchParams.has('directory_branch_id')){
            if(!response.ok)throw new Error('real folder preview failed');
            const page=await response.clone().json();
            if(page.preview!==true||page.writable!==false)throw new Error('folder preview was not read-only');
            probe.previews++;
        }else if(local&&method==='GET'&&url.pathname.endsWith('/entries')&&probe.delivered){
            if(!response.ok)throw new Error('real default view reload failed');
            probe.defaultReads++;
        }
        if(publication&&probe.delay){
            if(!response.ok)throw new Error('real default branch publication failed');
            await new Promise(resolve=>probe.release=resolve);
            probe.delivered=true;
        }
        return response;
    };return true;
"#;

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

fn create_branch(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Create folder branch", "创建目录分支")?)?;
    let input = browser.css(".zxs-branch-name:not([disabled])")?;
    browser.keys(&input, BRANCH)?;
    browser.keys(&input, "\u{e007}")?;
    check(
        browser,
        "real folder branch registration",
        "return !document.querySelector('.zxs-branch-name')&&/registered without switching|已登记，尚未切换/.test(document.querySelector('.zxs-inline-feedback > .zxs-status')?.textContent||'')",
    )
}

fn preview_branch(browser: &Browser) -> Result<()> {
    let combo = browser.element("xpath", BROWSE)?;
    let before = browser.script(
        "const p=window.__zxsNoticeDelivery;return {writes:p.writes.length,previews:p.previews}",
    )?;
    let panel = browser.command(
        "GET",
        &format!("/element/{combo}/attribute/aria-controls"),
        None,
    )?;
    let panel = panel.as_str().ok_or("folder combobox lacks a listbox")?;
    browser.click(&combo)?;
    browser.click(&browser.element("xpath", &select_option_xpath(panel, BRANCH))?)?;
    check(
        browser,
        "read-only folder preview settled without mutation or notice",
        &format!(
            "const p=window.__zxsNoticeDelivery,b=document.querySelector({});return p.previews==={}+1&&p.writes.length==={}&&b&&!b.disabled&&!document.querySelector({})",
            json!(DEFAULT),
            before["previews"],
            before["writes"],
            json!(NOTICE),
        ),
    )
}

fn publish_default(browser: &Browser) -> Result<()> {
    preview_branch(browser)?;
    browser.click(&browser.css(DEFAULT)?)?;
    Ok(())
}

fn saved(browser: &Browser) -> Result<()> {
    check(
        browser,
        "real default branch publication success",
        &format!(
            r#"
        const notice=document.querySelector({notice_selector}),button=document.querySelector({default_selector});
        return notice?.getAttribute('role')==='status'
            &&{messages}.includes(notice.textContent.trim())
            &&button?.disabled;
    "#,
            notice_selector = json!(NOTICE),
            default_selector = json!(DEFAULT),
            messages = json!([DEFAULT_SUCCESS_EN, DEFAULT_SUCCESS_ZH]),
        ),
    )
}

fn exercise_notices(browser: &Browser) -> Result<()> {
    for (english, chinese) in [("Status", "状态"), ("History", "历史记录")]
        .into_iter()
        .chain(AdminPage::ALL.map(AdminPage::labels))
        .chain([("Account settings", "账号设置"), ("Details", "详情")])
    {
        publish_default(browser)?;
        saved(browser)?;
        browser.click(&browser.button(english, chinese)?)?;
        no_notice(browser)?;
        browser.click(&browser.button("Files", "文件")?)?;
        no_notice(browser)?;
    }
    publish_default(browser)?;
    saved(browser)?;
    browser.click(&browser.button("Dismiss operation notice", "关闭操作提示")?)?;
    no_notice(browser)?;
    publish_default(browser)?;
    saved(browser)?;
    let started = Instant::now();
    check(
        browser,
        "ordinary success expires",
        &format!("return !document.querySelector({})", json!(NOTICE)),
    )?;
    if !(Duration::from_secs(3)..=Duration::from_secs(8)).contains(&started.elapsed()) {
        return Err("ordinary success exceeded its five-second lifetime tolerance".into());
    }
    browser.script("window.__zxsNoticeDelivery.delay=true;return true")?;
    publish_default(browser)?;
    check(
        browser,
        "real default branch receipt awaiting delivery",
        "return typeof window.__zxsNoticeDelivery.release==='function'",
    )?;
    browser.click(&browser.button("History", "历史记录")?)?;
    browser.script("window.__zxsNoticeDelivery.release();return true")?;
    check(
        browser,
        "late default branch receipt consumed while away",
        "const p=window.__zxsNoticeDelivery;return p.delivered&&p.defaultReads>0",
    )?;
    // The real default-view reload starts only after WASM consumes the receipt.
    // Check while still away: returning to Files would itself clear a leaked notice.
    no_notice(browser)?;
    browser.click(&browser.button("Files", "文件")?)?;
    browser.element("xpath", BROWSE)?;
    check(
        browser,
        "late receipt settled without reopening its old notice",
        &format!(
            r#"
        const button=document.querySelector({});
        const preview=Array.from(document.querySelectorAll('button')).some(button=>['Return to default view','返回默认视图'].includes(button.textContent.trim()));
        return button?.disabled&&!preview&&!document.querySelector({});
    "#,
            json!(DEFAULT),
            json!(NOTICE),
        ),
    )?;
    if browser.script(
        "const writes=window.__zxsNoticeDelivery.writes;return writes.length===11&&writes.every(write=>write.method==='PUT'&&/^\\/api\\/repositories\\/[^/]+\\/default-branch$/.test(write.path))",
    )? != true {
        return Err("notification lifecycle did not publish exactly eleven default-branch receipts".into());
    }
    Ok(())
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-fixtures']",
    )?)?;
    browser.css("[data-directory-git-initialized='true'][data-directory-git-staged='false'][data-directory-git-needs-restage='false']")?;
    create_branch(browser)?;
    browser.script(OBSERVE_FETCH)?;
    with_cleanup(
        || exercise_notices(browser),
        || {
            browser.script("if(window.__zxsNoticeDelivery){window.__zxsNoticeDelivery.restore();delete window.__zxsNoticeDelivery;}return true")?;
            Ok(())
        },
    )?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    println!(
        "PASS read-only folder browsing and real default-branch notices: page ownership, dismissal, repeated publication, five-second expiry and late receipt after navigation"
    );
    Ok(())
}
