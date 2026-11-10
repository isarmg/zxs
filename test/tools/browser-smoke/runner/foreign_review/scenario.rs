use super::*;
use session::Session;

fn input(browser: &Browser, selector: &str, value: &str) -> Result<()> {
    let element = browser.css(selector)?;
    browser.command(
        "POST",
        &format!("/element/{element}/clear"),
        Some(json!({})),
    )?;
    browser.keys(&element, value)
}
fn last(browser: &Browser, kind: &str) -> Result<Value> {
    browser.script(&format!(
        "return window.__zxsForeignReview.records.filter(r=>r.kind==={}).at(-1)",
        json!(kind)
    ))
}
fn expected_source(browser: &Browser, fixture: &Fixture) -> Result<Value> {
    let record = last(browser, "source_preview")?;
    let source = &record["response"];
    let histories = source["histories"]
        .as_array()
        .ok_or("source preview omitted histories")?;
    if record["status"] != 200
        || source["source_level"] != 3
        || source["source"]["path"] != fixture.source.to_str().ok_or("non-UTF8 source")?
        || source["execution"] != "unavailable"
        || histories.len() != 2
        || !histories.iter().any(|h| {
            h["temporarily_unavailable_versions"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        })
    {
        return Err(format!(
            "browser did not inspect the real tier-3 multi-origin fixture: {record}"
        )
        .into());
    }
    Ok(source.clone())
}
fn keyboard_consent(browser: &Browser) -> Result<Value> {
    let checkbox = browser.css("[data-foreign-plan-confirm]")?;
    // WebDriver sends native keys. This does not set checked, dispatch a fake
    // change, call focus(), or synthesize :focus-visible for the assertion.
    browser.keys(&checkbox, " ")?;
    browser.css("[data-foreign-plan-save]:not([disabled])")?;
    browser.keys(&checkbox, "\u{e004}")?;
    let focus = browser.script("const n=document.activeElement,s=getComputedStyle(n);return {save:n.matches('[data-foreign-plan-save]'),visible:n.matches(':focus-visible'),outline:s.outlineStyle,width:s.outlineWidth}")?;
    if focus["save"] != true
        || focus["visible"] != true
        || focus["outline"] == "none"
        || focus["width"] == "0px"
    {
        return Err(format!(
            "plan-only consent did not lead to visible native save focus: {focus}"
        )
        .into());
    }
    browser.keys(
        &browser.css("[data-foreign-plan-save]:not([disabled])")?,
        "\u{e007}",
    )?;
    Ok(focus)
}
fn preserved(fixture: &Fixture, before: &fixture::Snapshot) -> Result<()> {
    fixture.source_unchanged()?;
    if fixture::snapshot(&fixture.target)? != *before {
        return Err("review/save/status touched the registered data disk".into());
    }
    Ok(())
}
pub(super) fn exercise(
    browser: &Browser,
    fixture: &Fixture,
    actor: &Session,
    before: &fixture::Snapshot,
) -> Result<Value> {
    AdminPage::Storage.open(browser)?;
    browser.click(&browser.css("[data-storage-discover]:not([disabled])")?)?;
    browser.click(&browser.css(&format!(
        "[data-storage-candidate={}]:not([disabled])",
        json!(fixture.source)
    ))?)?;
    browser.click(&browser.css("[data-foreign-preview-start]:not([disabled])")?)?;
    browser.css("[data-foreign-plan-form]")?;
    let source = expected_source(browser, fixture)?;
    let mut audits = vec![audit(browser, "draft", &source)?];
    for mode in ["AdmitNewPrimary", "AdmitSecondary"] {
        browser.click(&browser.css(&format!("[data-foreign-plan-mode='{mode}']"))?)?;
        audits.push(audit(browser, "local", &source)?);
        check(
            browser,
            "unsupported intent remains a local draft",
            "return window.__zxsForeignReview.records.every(r=>r.kind==='source_preview')",
        )?;
    }
    browser.click(&browser.css("[data-foreign-plan-mode='ReconstructExistingPrimary']")?)?;
    input(browser, "[data-foreign-plan-slot]", "p01")?;
    input(browser, "[data-foreign-plan-parent]", fixture::PARENT)?;
    for (index, history) in source["histories"]
        .as_array()
        .ok_or("missing histories")?
        .iter()
        .enumerate()
    {
        input(
            browser,
            &format!("[data-foreign-plan-name={}]", history["group_id"]),
            &format!("origin-{}", index + 1),
        )?;
    }
    browser.click(&browser.css("[data-foreign-plan-preview]:not([disabled])")?)?;
    browser.css("[data-foreign-plan-review]")?;
    let preview = last(browser, "review_preview")?;
    if preview["status"] != 200
        || preview["response"]["source"] != source
        || preview["response"]["target"]["identity"]["slot_id"] != "P01"
    {
        return Err(format!(
            "review is not bound to actual source and canonical target: {preview}"
        )
        .into());
    }
    audits.push(audit(browser, "review", &source)?);
    preserved(fixture, before)?;
    browser.script("window.__zxsForeignReview.loseNextSave=true;return true")?;
    let keyboard = keyboard_consent(browser)?;
    browser.css("[data-foreign-plan-retry]:not([disabled])")?;
    audits.push(audit(browser, "unknown", &source)?);
    let first = last(browser, "save")?;
    if first["status"] != 200
        || first["delivery"] != "lost_after_actual_success"
        || first["body"]["preview"] != preview["response"]
        || first["body"]["acknowledge_plan_only"] != true
    {
        return Err(
            format!("unknown outcome did not follow the actual original save: {first}").into(),
        );
    }
    // Normal SPA navigation must keep the original uncertain request. It is not
    // evidence of full reload/session-end automatic recovery (still unsupported).
    browser.click(&browser.button("Files", "文件")?)?;
    AdminPage::Storage.open(browser)?;
    browser.css("[data-foreign-plan-retry]:not([disabled])")?;
    audits.push(audit(browser, "unknown", &source)?);
    browser.script("window.__zxsForeignReview.holdNext='save';const b=document.querySelector('[data-foreign-plan-retry]');b.click();b.click();return true")?;
    check(
        browser,
        "single retried original save is held",
        "return window.__zxsForeignReview.pendingIds().length===1 && window.__zxsForeignReview.records.filter(r=>r.kind==='save').length===2",
    )?;
    let retry = last(browser, "save")?;
    if retry["body"] != first["body"] || retry["response"] != first["response"] {
        return Err("save retry replaced its operation identity/body or immutable receipt".into());
    }
    browser.script("const p=window.__zxsForeignReview;p.release(p.pendingIds()[0]);return true")?;
    browser.css("[data-foreign-plan-saved]")?;
    audits.push(audit(browser, "stored", &source)?);
    browser.click(&browser.css("[data-foreign-plan-status]:not([disabled])")?)?;
    check(
        browser,
        "real original review status received",
        "return window.__zxsForeignReview.records.some(r=>r.kind==='status'&&r.status===200)",
    )?;
    let status = last(browser, "status")?;
    let operation = first["body"]["preview"]["request"]["operation_id"]
        .as_str()
        .ok_or("missing original operation ID")?;
    if status["query"] != format!("?operation_id={operation}")
        || status["response"] != first["response"]
    {
        return Err(
            format!("status did not recover the original immutable record: {status}").into(),
        );
    }
    browser.click(&browser.css("[data-foreign-plan-revalidate]:not([disabled])")?)?;
    check(
        browser,
        "real unchanged review revalidated",
        "return window.__zxsForeignReview.records.some(r=>r.kind==='revalidate'&&r.status===200&&r.response.validation==='matched_at_observation')",
    )?;
    preserved(fixture, before)?;
    // A separate authenticated actor publishes an unrelated directory. Merely
    // opening a workspace does not advance the published target checkpoint.
    let operation = storage_engine::OperationId::generate()?;
    let publication = actor.success(
        "POST",
        "/api/file-manager/disks/P01/namespace",
        Some(json!({"kind":"mkdir","path":"OtherActor","operation_id":operation})),
    )?;
    if publication["operation_id"] != json!(operation) || publication["state"] != "completed" {
        return Err(format!(
            "target actor did not publish its exact namespace operation: {publication}"
        )
        .into());
    }
    let after_actor = fixture::snapshot(&fixture.target)?;
    if after_actor == *before {
        return Err("real namespace publication did not advance the target disk".into());
    }
    browser.click(&browser.css("[data-foreign-plan-revalidate]:not([disabled])")?)?;
    check(
        browser,
        "actual target change rejects stale review",
        "const r=window.__zxsForeignReview.records.filter(r=>r.kind==='revalidate').at(-1);return r?.status===409&&r.response?.error?.code==='PLAN_STALE'",
    )?;
    browser.css("[data-foreign-plan-revalidate]:not([disabled])")?;
    audits.push(audit(browser, "stored", &source)?);
    check(
        browser,
        "stale observation no longer shown as matched",
        "const n=document.querySelector('[data-foreign-plan-saved]');return n&&(n.textContent.includes('requires revalidation')||n.textContent.includes('仍需重新核对'))",
    )?;
    preserved(fixture, &after_actor)?;
    let records = browser.script("return window.__zxsForeignReview.records")?;
    if records
        .as_array()
        .ok_or("missing review records")?
        .iter()
        .any(|record| record["kind"] == "forbidden")
    {
        return Err(
            "review UI sent an execution, intent-writer or unsupported foreign route".into(),
        );
    }
    Ok(
        json!({"source":"production daemon and WASM; only original response delivery is faulted",
        "execution":"unavailable", "full_reload_recovery":"not implemented or claimed",
        "keyboard":keyboard,"accessibility":audits,"records":records,
        "intentional_target_change":{"operation_id":operation,"repository_id":fixture.repository.repository_id,"path":"OtherActor","receipt":publication},
        "preservation":"source identities/bytes unchanged; target unchanged except separate real actor"}),
    )
}
