//! Real WASM, independent ordinary pool, two Chrome tabs, one account. Only
//! response delivery is delayed; fixtures never manufacture a successful receipt.
use super::{Browser, Result, json, wait_for, with_cleanup};
const HOOK: &str = include_str!("first_commit_tabs.js");
const KEY: &str = "zxs:first-file-commit:smoke-admin";
fn switch(browser: &Browser, handle: &str) -> Result<()> {
    browser.command("POST", "/window", Some(json!({"handle":handle})))?;
    Ok(())
}
pub(super) fn ready(browser: &Browser, origin: &str, password: &str) -> Result<()> {
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    let panel = wait_for(
        "authenticated file listing or its actual login form",
        || {
            let state=browser.script("return document.querySelector('.zxs-disk-entry,[data-zxs-first-commit-recovery]') ? 'files' : document.querySelector('form[data-auth-form=login] input[autocomplete=current-password]:not([disabled])') ? 'login' : ''")?;
            state
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| "not ready".into())
        },
    )?;
    if panel == "login" {
        browser.keys(
            &browser.css("form[data-auth-form=login] input[autocomplete=username]")?,
            "smoke-admin",
        )?;
        browser.keys(
            &browser.css("form[data-auth-form=login] input[autocomplete=current-password]")?,
            password,
        )?;
        browser.click(&browser.button("Sign in", "登录")?)?;
    }
    Ok(())
}
fn review(browser: &Browser, slot: &str, name: &str) -> Result<()> {
    browser.click(&browser.css(&format!(
        ".zxs-disk-entry[data-slot='{slot}']:not([disabled])"
    ))?)?;
    let file = browser.element(
        "xpath",
        &format!("//button[contains(@class,'zxs-file-name') and normalize-space(.)='{name}']"),
    )?;
    browser.click(&file)?;
    browser.click(&browser.css("[data-zxs-review-first-commit]:not([disabled])")?)?;
    browser.css("[data-zxs-first-commit]:not([disabled])")?;
    Ok(())
}
fn until(browser: &Browser, label: &str, script: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(label.into())
        }
    })
}
fn install(browser: &Browser, body: &str) -> Result<String> {
    let id = browser.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":body}),
    )?;
    Ok(id["identifier"]
        .as_str()
        .ok_or("missing delivery observer")?
        .into())
}
pub(super) fn run(browser: &Browser, origin: &str, password: &str, slot: &str) -> Result<()> {
    let first = browser
        .command("GET", "/window", None)?
        .as_str()
        .ok_or("missing first tab")?
        .to_owned();
    let second = browser.command("POST", "/window/new", Some(json!({"type":"tab"})))?["handle"]
        .as_str()
        .ok_or("missing second tab")?
        .to_owned();
    with_cleanup(
        || {
            switch(browser, &first)?;
            ready(browser, origin, password)?;
            review(browser, slot, "tab-one.txt")?;
            browser.script(&format!("{HOOK}\nwindow.__zxsFirst.pausePost=true;window.__zxsFirst.pauseRead=true;return true"))?;
            switch(browser, &second)?;
            ready(browser, origin, password)?;
            review(browser, slot, "tab-two.txt")?;
            browser.script(&format!("{HOOK}\nreturn true"))?;
            switch(browser, &first)?;
            browser.click(&browser.css("[data-zxs-first-commit]:not([disabled])")?)?;
            until(
                browser,
                "server already published the first original, tab still holds its response",
                "return typeof window.__zxsFirst.releasePost==='function'",
            )?;
            let saved = browser
                .script(&format!("return localStorage.getItem({})", json!(KEY)))?
                .as_str()
                .ok_or("missing first owned original")?
                .to_owned();
            let record: json_codec::Value = json_codec::from_str(&saved)?;
            let id = record["command"]["operation_id"]
                .as_str()
                .ok_or("missing original operation")?;
            if browser.script("return window.__zxsFirst.confirmed")? != id {
                return Err("retained ID differs from the real published operation".into());
            }
            // Tab two had already reviewed a different real Unborn file.
            switch(browser, &second)?;
            browser.click(&browser.css("[data-zxs-first-commit]:not([disabled])")?)?;
            until(
                browser,
                "second tab queues behind the actual same-account Web Lock",
                &format!(
                    "return navigator.locks.query().then(q=>q.held.some(x=>x.name==={0})&&q.pending.some(x=>x.name==={0})&&window.__zxsFirst.posts.length===0&&localStorage.getItem({0})==={1})",
                    json!(KEY),
                    json!(saved)
                ),
            )?;
            switch(browser, &first)?;
            browser.script("window.__zxsFirst.releasePost();return true")?;
            switch(browser, &second)?;
            until(
                browser,
                "other tab preserves the lost original instead of submitting its newer review",
                &format!(
                    "return navigator.locks.query().then(q=>!q.held.some(x=>x.name==={0})&&!q.pending.some(x=>x.name==={0})&&window.__zxsFirst.posts.length===0&&localStorage.getItem({0})==={1})",
                    json!(KEY),
                    json!(saved)
                ),
            )?;
            let hook = install(
                browser,
                &format!("{HOOK}\nwindow.__zxsFirst.pauseRead=true;"),
            )?;
            ready(browser, origin, password)?;
            until(
                browser,
                "refresh only queries the original real receipt",
                &format!(
                    "return window.__zxsFirst.seen==={}&&window.__zxsFirst.posts.length===0&&typeof window.__zxsFirst.releaseRead==='function'",
                    json!(id)
                ),
            )?;
            // An external tab edits the retained text while the reply is in flight.
            // Semantically equivalent JSON is still not the exact original record.
            switch(browser, &first)?;
            browser.script(&format!(
                "localStorage.setItem({},{});return true",
                json!(KEY),
                json!(saved.clone() + " ")
            ))?;
            switch(browser, &second)?;
            browser.script("window.__zxsFirst.releaseRead();return true")?;
            until(
                browser,
                "a settled response cannot delete externally replaced text",
                &format!(
                    "return Array.from(document.querySelectorAll('[data-zxs-first-commit-recovery] [role=alert]')).some(e=>e.textContent.trim())&&localStorage.getItem({})==={}&&window.__zxsFirst.posts.length===0",
                    json!(KEY),
                    json!(saved.clone() + " ")
                ),
            )?;
            browser.cdp(
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({"identifier":hook}),
            )?;
            install(browser, HOOK)?;
            ready(browser, origin, password)?;
            until(
                browser,
                "a fresh owner of the replacement may clean it only after its own original receipt",
                &format!(
                    "return window.__zxsFirst.reads.includes({})&&window.__zxsFirst.posts.length===0&&localStorage.getItem({})===null",
                    json!(id),
                    json!(KEY)
                ),
            )?;
            Ok(())
        },
        || {
            switch(browser, &second)?;
            browser.command("DELETE", "/window", None)?;
            switch(browser, &first)
        },
    )
}
