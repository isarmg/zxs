//! Queued timeout, logout cancellation, late 401, and one-shot 429 delivery.
use super::*;

fn check(browser: &Browser, name: &str, script: &str) -> Result<()> {
    wait_for(name, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(format!("{name} not satisfied").into())
        }
    })
}

pub(super) fn run(browser: &Browser, password: &str) -> Result<()> {
    // Start from a real new login, not a previous language reload or refresh.
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    browser.keys(
        &browser.css("input[autocomplete='username']")?,
        "smoke-admin",
    )?;
    browser.keys(
        &browser.css("input[autocomplete='current-password']")?,
        password,
    )?;
    browser.click(&browser.button("Sign in", "登录")?)?;
    browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
    browser.script(include_str!("management_queue_lifecycle.js"))?;
    with_cleanup(
        || {
            AdminPage::Storage.open(browser)?;
            check(
                browser,
                "real discovery held with a specific readmission request queued",
                "const p=window.__zxsManagementQueue;return p.held&&!p.active.signal.aborted&&p.untracked===0&&p.records.some(r=>r.phase==='old'&&r.path==='/api/admin/disks/readmission'&&!r.started&&r.deadline!==null)",
            )?;
            // Invoke the callback registered by production for 120000ms; this does
            // not claim elapsed wall-clock timing precision.
            browser.script("window.__zxsManagementQueue.timeoutQueued();return true")?;
            check(
                browser,
                "queued deadline aborts before any fetch",
                "const p=window.__zxsManagementQueue;return p.timedOut.signal.aborted&&!p.timedOut.started&&!p.active.signal.aborted&&/The request timed out|请求超时/.test(document.body.textContent)",
            )?;

            browser.script("window.__zxsManagementQueue.phase='transition';return true")?;
            browser.click(&browser.button("Sign out", "退出")?)?;
            browser.css("input[autocomplete='current-password']")?;
            browser.script("window.__zxsManagementQueue.phase='new';return true")?;
            browser.keys(
                &browser.css("input[autocomplete='username']")?,
                "smoke-admin",
            )?;
            browser.keys(
                &browser.css("input[autocomplete='current-password']")?,
                password,
            )?;
            browser.click(&browser.button("Sign in", "登录")?)?;
            browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
            AdminPage::Storage.open(browser)?;
            check(
                browser,
                "new-session management read waits behind the old active response",
                "const p=window.__zxsManagementQueue;return p.newCredentialCaptured&&p.newCredentialDistinct&&p.records.some(r=>r.phase==='new'&&r.path==='/api/admin/storage/status'&&!r.started&&r.deadline!==null)",
            )?;
            browser.script("const p=window.__zxsManagementQueue;p.lateUnauthorized=true;p.release();return true")?;
            check(
                browser,
                "old queued reads never dispatch and old 401 cannot revoke new login",
                "const p=window.__zxsManagementQueue;return p.framesPassed&&p.delivered&&p.lateOldStarts===0&&p.newCredentialDistinct&&p.records.some(r=>r.phase==='new'&&r.path==='/api/admin/storage/status'&&r.started&&r.deliveredStatus===200)&&!document.querySelector(\"input[autocomplete='current-password']\")&&!document.body.textContent.includes('smoke-old-management-401')",
            )?;

            browser.css(".zxs-header-refresh:not([disabled])")?;
            check(
                browser,
                "new Bearer is retained, not only the cookie session",
                "const p=window.__zxsManagementQueue;return p.newCredentialChecks>1&&!p.credentialChanged&&p.lateOldStarts===0&&p.untracked===0",
            )?;
            browser.script("const p=window.__zxsManagementQueue;p.mode='rate-limit';p.rateLimitStart=p.records.length;return true")?;
            browser.click(&browser.css(".zxs-header-refresh")?)?;
            check(
                browser,
                "429 keeps its status detail and one attempted request",
                "const p=window.__zxsManagementQueue,text=document.body.textContent;return p.rateLimitCalls===1&&text.includes('management operation capacity reached')&&text.includes('smoke-management-queue')&&text.includes('429')&&text.includes('RESOURCE_LIMIT_EXCEEDED')&&!document.querySelector('.zxs-header-refresh').disabled",
            )?;
            browser.script("const p=window.__zxsManagementQueue;p.checkedFrames=false;requestAnimationFrame(()=>requestAnimationFrame(()=>p.checkedFrames=true));return true")?;
            check(
                browser,
                "429 is not automatically retried",
                "const p=window.__zxsManagementQueue;return p.checkedFrames&&p.rateLimitCalls===1",
            )?;
            browser.script("window.__zxsManagementQueue.mode='pass';return true")?;
            browser.click(&browser.css(".zxs-header-refresh")?)?;
            check(
                browser,
                "explicit refresh returns to real discovery",
                "const p=window.__zxsManagementQueue,reads=p.records.slice(p.rateLimitStart).filter(r=>r.path==='/api/admin/storage/discovered-disks');return reads.length===2&&reads[0].deliveredStatus===429&&reads[1].deliveredStatus===200&&!document.querySelector('.zxs-header-refresh').disabled&&!document.body.textContent.includes('management operation capacity reached')",
            )?;
            Ok(())
        },
        || {
            browser.script(
                "if(window.__zxsManagementQueue)window.__zxsManagementQueue.restore();return true",
            )?;
            Ok(())
        },
    )?;
    println!(
        "PASS management queued deadline, logout cancellation, late 401 fencing and one-shot 429 through production WASM"
    );
    Ok(())
}
