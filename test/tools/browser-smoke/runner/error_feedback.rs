//! Exception decoding and bounded notices through the production WASM/DOM.
use super::*;

fn check(browser: &Browser, label: &str, expression: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(expression)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}

fn dismiss(browser: &Browser) -> Result<()> {
    for _ in 0..5 {
        let count = browser
            .script("return document.querySelector('.zxs-error-center')?.children.length||0")?
            .as_u64()
            .ok_or("invalid notice count")?;
        if count == 0 {
            return Ok(());
        }
        browser.click(&browser.css(".zxs-error-center button")?)?;
        check(
            browser,
            "dismissed notice",
            &format!(
                "return (document.querySelector('.zxs-error-center')?.children.length||0)<{count}"
            ),
        )?;
    }
    check(
        browser,
        "dismissed bounded error center",
        "return !document.querySelector('.zxs-error-center')",
    )
}

fn refresh_mode(browser: &Browser, mode: &str) -> Result<()> {
    browser.script(&format!(
        "window.__zxsErrorFeedback.mode={};window.__zxsErrorFeedback.timeout=null;return true",
        json!(mode)
    ))?;
    browser.click(&browser.button("Refresh", "刷新")?)?;
    Ok(())
}

fn read_errors(browser: &Browser) -> Result<()> {
    for (mode, expected) in [
        ("error", "Error: Smoke structured Error detail"),
        ("network", "TypeError: Smoke fetch or policy rejection"),
        ("dom", "SecurityError: Smoke security exception detail"),
        ("unowned-abort", "AbortError: Smoke unowned abort detail"),
        ("literal", "Smoke literal rejection"),
        ("body", "NetworkError: Smoke response body detail"),
    ] {
        refresh_mode(browser, mode)?;
        check(
            browser,
            "actual browser exception detail",
            &format!(
                "return document.querySelector('.zxs-error-center')?.textContent.includes({})",
                json!(expected)
            ),
        )?;
        if browser.script("return /The read was cancelled|读取已取消|The request timed out|请求超时/.test(document.querySelector('.zxs-error-center').textContent)")? == true {
            return Err("exception name alone was classified as an owned cancellation".into());
        }
        dismiss(browser)?;
    }
    for (mode, expected_timeout) in [
        ("timeout", true),
        ("body-timeout", true),
        ("timeout-race", false),
        ("timeout-undefined", false),
    ] {
        refresh_mode(browser, mode)?;
        check(
            browser,
            "request waiting with its owned deadline",
            "return typeof window.__zxsErrorFeedback.timeout==='function'",
        )?;
        browser.script("window.__zxsErrorFeedback.timeout();return true")?;
        check(
            browser,
            "deadline rejection reached the error center",
            "return !!document.querySelector('.zxs-error-center')",
        )?;
        let timed_out = browser.script("return /The request timed out|请求超时/.test(document.querySelector('.zxs-error-center').textContent)")?;
        if timed_out != expected_timeout {
            return Err(format!("incorrect controller-reason classification for {mode}").into());
        }
        if mode == "timeout-race" && browser.script("return document.querySelector('.zxs-error-center').textContent.includes('TypeError: Smoke unrelated timeout race')")? != true {
            return Err("unrelated timeout-race cause was lost".into());
        }
        dismiss(browser)?;
    }
    refresh_mode(browser, "cancel")?;
    check(
        browser,
        "cancellable directory read pending",
        "return typeof window.__zxsErrorFeedback.timeout==='function'",
    )?;
    browser.script(
        "const s=window.__zxsErrorFeedback;s.mode='pass';s.cancelBefore=s.aborted;return true",
    )?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    check(
        browser,
        "owner cancellation remains silent after navigation",
        "const s=window.__zxsErrorFeedback;return document.querySelectorAll('.zxs-disk-entry').length===3&&!document.querySelector('.zxs-error-center')&&s.aborted===s.cancelBefore+1",
    )?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    open_fixture_directory(browser)
}

fn poll_errors(browser: &Browser) -> Result<()> {
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    let edit = browser.button("Edit file note", "编辑文件备注")?;
    wait_for("metadata admission before exception fixture", || {
        if browser.command("GET", &format!("/element/{edit}/enabled"), None)? == true {
            Ok(())
        } else {
            Err("file history is not ready".into())
        }
    })?;
    browser.script("window.__zxsErrorFeedback.mode='mutation';return true")?;
    browser.click(&edit)?;
    let input = browser.css(".zxs-inline-input")?;
    browser.keys(&input, "browser error recovery note")?;
    browser.keys(&input, "\u{e007}")?;
    check(
        browser,
        "repeated failed polls remain bounded by operation identity",
        r#"
        const s=window.__zxsErrorFeedback,center=document.querySelector('.zxs-error-center');
        return s.polls>=3&&s.writes===1&&s.mutationStatus===200&&center?.children.length===2
            &&center.textContent.includes('smoke-poll-'+s.polls)
            &&center.textContent.includes('AbortError: Smoke mutation reply lost after publication')
            &&document.querySelector('.zxs-inline-input')?.disabled
            &&document.querySelector('[data-language-toggle]')?.disabled;
    "#,
    )?;
    browser.script("const s=window.__zxsErrorFeedback;s.dismissedAt=s.polls;for(const row of document.querySelector('.zxs-error-center').children){if(row.textContent.includes('Smoke poll failure'))row.querySelector('button').click();}return true")?;
    check(
        browser,
        "dismissed poll notice reappears on a later failure",
        "const s=window.__zxsErrorFeedback;return s.polls>s.dismissedAt&&document.querySelector('.zxs-error-center')?.children.length===2&&document.querySelector('.zxs-error-center').textContent.includes('smoke-poll-'+s.polls)",
    )?;
    browser.script("window.__zxsErrorFeedback.recover=true;return true")?;
    check(
        browser,
        "original operation poll recovers the published mutation",
        r#"
        const s=window.__zxsErrorFeedback;
        const edit=Array.from(document.querySelectorAll('button')).find(b=>['Edit file note','编辑文件备注'].includes(b.getAttribute('aria-label')));
        return s.writes===1&&s.pollPaths.every(path=>path==='/api/operations/'+s.operation)
            &&edit&&!edit.disabled&&edit.textContent.includes('browser error recovery note')
            &&!document.querySelector('.zxs-inline-input')&&!document.querySelector('[data-language-toggle]').disabled;
    "#,
    )?;
    dismiss(browser)?;
    // Return the real fixture note to its initial value for the later metadata test.
    browser.script("window.__zxsErrorFeedback.mode='pass';return true")?;
    browser.click(&browser.button("Edit file note", "编辑文件备注")?)?;
    browser.script("const input=document.querySelector('.zxs-inline-input');input.value='';input.dispatchEvent(new Event('input',{bubbles:true}));return true")?;
    browser.keys(&browser.css(".zxs-inline-input")?, "\u{e007}")?;
    check(
        browser,
        "real note reset completed",
        "return !document.querySelector('.zxs-inline-input')&&!document.querySelector('.zxs-entry-content').textContent.includes('browser error recovery note')",
    )?;
    browser.click(&browser.button("Files", "文件")?)?;
    Ok(())
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(include_str!("error_feedback.js"))?;
    with_cleanup(
        || {
            read_errors(browser)?;
            poll_errors(browser)
        },
        || {
            browser.script("if(window.__zxsErrorFeedback){window.__zxsErrorFeedback.restore();delete window.__zxsErrorFeedback;}return true")?;
            Ok(())
        },
    )?;
    println!(
        "PASS production WASM Error/DOMException details, owned timeout vs unrelated rejection, silent navigation cancellation, deduplicated polling and real original-operation recovery"
    );
    Ok(())
}
