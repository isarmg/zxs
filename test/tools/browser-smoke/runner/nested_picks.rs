//! Cross-folder selection in the packaged Web UI. Observe the actual single
//! POST and its native receipt, then independently re-open the stopped pool.
use super::{Browser, Result, first_commit_tabs, wait_for, with_cleanup};
use std::{cell::Cell, io::Read, path::Path};
use storage_contract::{
    OperationId, Principal,
    file_draft::{DraftLocation, WorkingRevision},
    snapshot_worktree::{
        SnapshotHandle,
        draft_commit::{
            CommitDirectoryDrafts, DirectoryDraftLookup, DirectoryDraftOutcome,
            DirectoryDraftReceipt, SelectedDirectoryDraft,
        },
    },
};
use storage_engine::{Engine, Repository};
mod fixture;
const LEFT: &str = "pick-left/chosen.txt";
const RIGHT: &str = "pick-right/chosen.txt";
const SKIP: &str = "pick-right/leave.txt";
const LEFT_BODY: &[u8] = b"new retained Born contents, unlike its old version";
const RIGHT_BODY: &[u8] = b"first retained independent contents";
const SKIP_BODY: &[u8] = b"this deliberately unselected working save survives";
pub(super) struct Evidence {
    source: SnapshotHandle,
    actor: Principal,
    selected: [SelectedDirectoryDraft; 2],
    skipped: DraftLocation,
    skipped_revision: WorkingRevision,
    old_version: String,
    original: Cell<Option<OperationId>>,
}
pub(super) fn seed(engine: &mut Engine, repo: &Repository, actor: &Principal) -> Result<Evidence> {
    fixture::seed(engine, repo, actor)
}
fn row(browser: &Browser, name: &str) -> Result<String> {
    browser.element(
        "xpath",
        &format!("//button[contains(@class,'zxs-file-name')][normalize-space(.)='{name}']"),
    )
}
fn choose(browser: &Browser, folder: &str, path: &str) -> Result<()> {
    browser.click(&row(browser, folder)?)?;
    row(browser, "chosen.txt")?;
    browser.click(&browser.css("[data-zxs-directory-snapshot-review]:not([disabled])")?)?;
    browser.click(&browser.css(&format!(
        "[data-zxs-directory-draft] input[aria-label='{path}']:not([disabled])"
    ))?)?;
    Ok(())
}
pub(super) fn run(
    browser: &Browser,
    origin: &str,
    password: &str,
    slot: &str,
    e: &Evidence,
) -> Result<()> {
    first_commit_tabs::ready(browser, origin, password)?;
    browser.click(&browser.css(&format!(
        ".zxs-disk-entry[data-slot='{slot}']:not([disabled])"
    ))?)?;
    row(browser, "pick-left")?;
    // Retain only this non-secret business request and receipt. Do not patch
    // headers, responses, body contents, delivery timing or any Rust UI state.
    browser.script(r#"
      const original=window.fetch,s=window.__zxsNestedSelection={count:0,request:null,receipt:null,restore(){window.fetch=original;}};
      window.fetch=async function(...args){
        const r=args[0], u=new URL(r.url||r,location.href);
        if(u.origin!==location.origin||u.pathname!=='/api/directory-snapshots/commits'||r.method!=='POST')return original.apply(this,args);
        s.count++;s.request=await r.clone().json();const response=await original.apply(this,args);
        if(response.ok)s.receipt=await response.clone().json();return response;
      };return true;
    "#)?;
    with_cleanup(
        || {
            choose(browser, "pick-left", LEFT)?;
            browser.click(&browser.css("[data-zxs-keep-drafts]:not([disabled])")?)?;
            browser.css("[data-zxs-kept-draft]")?;
            browser.click(&browser.css(".zxs-breadcrumb button:nth-of-type(2)")?)?;
            choose(browser, "pick-right", RIGHT)?;
            browser.css(&format!(
                "[data-zxs-directory-draft] input[aria-label='{SKIP}']:not(:checked)"
            ))?;
            if browser.script("return document.querySelector('[data-zxs-kept-draft]')?.textContent.includes('pick-left/chosen.txt')&&window.__zxsNestedSelection.count===0")?!=true{
            return Err("kept original was lost or a browser page change submitted it".into());
        }
            browser.keys(
                &browser.css("[data-zxs-directory-snapshot-state=head] input[type=text]")?,
                "two exact nested sources",
            )?;
            browser.click(&browser.css("[data-zxs-directory-snapshot-commit]:not([disabled])")?)?;
            let collected = wait_for("one actual original nested-directory publication", || {
                let data=browser.script("const s=window.__zxsNestedSelection,p=document.querySelector('[data-zxs-directory-snapshot-result]');return s?.count===1&&s.receipt&&p?.textContent.includes(s.receipt.result.snapshot.node_id)?[s.request,s.receipt]:null")?;
                if data.is_array() {
                    Ok(data)
                } else {
                    Err("no matching actual server receipt and displayed node yet".into())
                }
            })?;
            let request: CommitDirectoryDrafts = json_codec::from_value(collected[0].clone())?;
            let receipt: DirectoryDraftReceipt = json_codec::from_value(collected[1].clone())?;
            if request.message != "two exact nested sources"
                || request.source != e.source
                || request.selected.as_slice() != e.selected.as_slice()
                || request.operation_id != receipt.operation_id
                || !matches!(receipt.result,DirectoryDraftOutcome::Changed{snapshot} if snapshot.publication==request.operation_id && snapshot.node_id!=e.source.node_id)
            {
                return Err("browser did not publish both previously captured actual revision/content expectations in one original".into());
            }
            e.original.set(Some(request.operation_id));
            Ok(())
        },
        || {
            browser.script("if(window.__zxsNestedSelection){window.__zxsNestedSelection.restore();delete window.__zxsNestedSelection;}return true")?;
            Ok(())
        },
    )
}
pub(super) fn cold(root: &Path, e: &Evidence) -> Result<()> {
    let engine = Engine::open(root)?;
    let operation = e
        .original
        .get()
        .ok_or("no original nested browser decision to reopen")?;
    let receipt = engine.directory_draft_receipt(
        DirectoryDraftLookup {
            source: e.source,
            operation_id: operation,
        },
        &e.actor,
    )?;
    if !matches!(receipt.result,DirectoryDraftOutcome::Changed{snapshot} if snapshot.publication==operation&&snapshot.node_id!=e.source.node_id)
    {
        return Err("cold root lost the same original publication".into());
    }
    for (target, body) in e
        .selected
        .iter()
        .map(|it| &it.target)
        .zip([LEFT_BODY, RIGHT_BODY])
    {
        let working = engine.file_working_state(target)?;
        if working.unborn().is_some() || working.draft().is_some() {
            return Err("a selected draft was not consumed".into());
        }
        let base = engine.file_draft_base(target)?;
        let mut bytes = Vec::new();
        engine
            .open_content(&base.version_id().to_string())?
            .file
            .read_to_end(&mut bytes)?;
        if bytes != body {
            return Err("selected historical FILE bytes differ on cold reopen".into());
        }
    }
    let mut retained = Vec::new();
    engine
        .open_content(&e.old_version)?
        .file
        .read_to_end(&mut retained)?;
    if retained != b"old"
        || engine.unborn_working_state(&e.skipped)?.revision() != e.skipped_revision
    {
        return Err("old Born version or unrelated Unborn working revision changed".into());
    }
    let input = engine.open_native_working(&e.skipped, &e.actor, false)?;
    let mut bytes = vec![0; SKIP_BODY.len() + 1];
    let actual = engine.read_native_working(&input, &mut bytes, 0)?;
    if actual != SKIP_BODY.len() || &bytes[..actual] != SKIP_BODY {
        return Err("unselected saved body changed".into());
    }
    Ok(())
}
