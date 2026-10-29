//! Real WASM/DOM history controls with explicitly synthetic paged delivery.
//! Captured writes are rejected before the server: these checks prove browser
//! action construction/lifetime, not engine scale or a published merge.
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

fn button(browser: &Browser, scope: &str, en: &str, zh: &str) -> Result<String> {
    browser.element("xpath", &format!(
        "//*[contains(concat(' ',normalize-space(@class),' '),' {scope} ')]//button[normalize-space(.)='{en}' or normalize-space(.)='{zh}']"
    ))
}

fn choose(browser: &Browser, scope: &str, en: &str, zh: &str, choice: &str) -> Result<()> {
    let select = format!(
        "//*[contains(concat(' ',normalize-space(@class),' '),' {scope} ')]//div[contains(concat(' ',normalize-space(@class),' '),' zxs-select ')][label[normalize-space(.)='{en}' or normalize-space(.)='{zh}']]"
    );
    browser.click(&browser.element("xpath", &format!("{select}/button"))?)?;
    browser.click(&browser.element(
        "xpath",
        &format!("{select}//div[@role='option'][starts-with(normalize-space(.),'{choice}')]"),
    )?)?;
    Ok(())
}

fn branch_next(browser: &Browser, scope: &str) -> Result<()> {
    browser.click(&button(browser, scope, "Next branches", "下一页分支")?)
}
fn branch_previous(browser: &Browser, scope: &str) -> Result<()> {
    browser.click(&button(browser, scope, "Previous branches", "上一页分支")?)
}

fn assert_idle_branch_failure(browser: &Browser, expected: u64) -> Result<()> {
    // Twelve animation turns cover network completion and queued reactive
    // effects. This is an explicit no-auto-retry observation window.
    let result = browser.cdp("Runtime.evaluate", json!({
        "expression":"(async()=>{for(let i=0;i<12;i++)await new Promise(requestAnimationFrame);return window.__zxsHistoryPages.calls.filter(call=>call.path.endsWith('/branches')).length;})()",
        "awaitPromise":true,"returnByValue":true,
    }))?;
    if !result["exceptionDetails"].is_null() || result["result"]["value"] != expected {
        return Err(format!("persistent branch failure automatically retried: {result}").into());
    }
    Ok(())
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(include_str!("history_pages.js"))?;
    with_cleanup(
        || exercise(browser),
        || {
            browser.script("if(window.__zxsHistoryPages){window.__zxsHistoryPages.restore();delete window.__zxsHistoryPages;}return true")?;
            Ok(())
        },
    )?;
    // Restore the real file root before the rest of the product smoke suite.
    browser.click(&browser.button("Refresh", "刷新")?)?;
    check(
        browser,
        "real history restored",
        "return document.querySelectorAll('.zxs-version-node').length===2&&!document.querySelector('[data-node-id=history-node-200]')",
    )?;
    println!(
        "PASS synthetic bounded history DOM: 201 branches across merge/tags, editor guards, bidirectional parent pages, explicit retry and cancelled-response fences (not engine-scale evidence)"
    );
    Ok(())
}

fn exercise(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Refresh", "刷新")?)?;
    check(
        browser,
        "history and branch first pages",
        "return document.querySelectorAll('.zxs-version-node').length===50&&document.querySelector('[data-node-id=history-node-200]')&&document.querySelector('.zxs-file-branches .zxs-select').querySelectorAll('[role=option]').length===100",
    )?;
    branch_next(browser, "zxs-file-branches")?;
    check(
        browser,
        "second branch page",
        "return document.querySelector('.zxs-file-branches .zxs-select').textContent.includes('Branch 100')&&!document.querySelector('.zxs-file-branches .zxs-select').textContent.includes('Branch 001')",
    )?;
    branch_next(browser, "zxs-file-branches")?;
    check(
        browser,
        "last branch and outside-page current identity",
        "const select=document.querySelector('.zxs-file-branches .zxs-select');return select.querySelectorAll('[role=option]').length===2&&select.textContent.includes('Branch 200')&&select.textContent.includes('history-branch-000')",
    )?;
    choose(
        browser,
        "zxs-file-branches",
        "Current branch",
        "当前分支",
        "Branch 200",
    )?;
    check(
        browser,
        "last branch switch request is reachable",
        "return window.__zxsHistoryPages.writes.some(write=>write.path.endsWith('/select')&&write.body.branch_id==='history-branch-200')&&!document.querySelector('.zxs-file-branches .zxs-select-trigger').disabled",
    )?;
    branch_previous(browser, "zxs-file-branches")?;
    check(
        browser,
        "branch previous middle page",
        "return document.querySelector('.zxs-file-branches .zxs-select').textContent.includes('Branch 100')",
    )?;
    branch_previous(browser, "zxs-file-branches")?;
    check(
        browser,
        "branch previous first page",
        "return document.querySelector('.zxs-file-branches .zxs-select').textContent.includes('Branch 001')",
    )?;
    cross_page_merge(browser)?;
    cross_page_tags(browser)?;
    editor_navigation(browser)?;
    parent_pages(browser)?;
    failures_and_cancellation(browser)?;
    check(
        browser,
        "bounded tree observers after repeated replacements",
        "const records=window.__zxsHistoryPages.observers.filter(record=>record.root);const active=records.filter(record=>!record.disconnected);return active.length===2&&active.filter(record=>record.name==='ResizeObserver').every(record=>record.targets.size<=51)&&records.filter(record=>!record.root.isConnected).every(record=>record.disconnected&&record.targets.size===0)&&document.querySelectorAll('.zxs-version-node').length===50",
    )?;
    Ok(())
}

fn cross_page_merge(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Merge file branches", "合并文件分支")?)?;
    check(
        browser,
        "only the current branch is selected initially",
        "return document.querySelectorAll('.zxs-merge-review input:checked').length===1&&document.querySelectorAll('.zxs-merge-review input[type=checkbox]').length===100",
    )?;
    branch_next(browser, "zxs-merge-review")?;
    check(
        browser,
        "merge middle candidate page",
        "return document.querySelector('.zxs-merge-review').textContent.includes('Branch 100')",
    )?;
    branch_next(browser, "zxs-merge-review")?;
    check(
        browser,
        "merge last candidate retains selected first branch",
        "return document.querySelectorAll('.zxs-merge-review input[type=checkbox]').length===2&&document.querySelector('.zxs-merge-review').textContent.includes('Branch 200')&&document.querySelector('.zxs-merge-review').textContent.includes('Branch 000')",
    )?;
    browser.click(&browser.element("xpath", "//section[contains(@class,'zxs-merge-review')]//label[span[normalize-space(.)='Branch 200']]/input")?)?;
    choose(
        browser,
        "zxs-merge-review",
        "Result node (any node of this file; page to find it)",
        "结果节点（可选此文件任一节点，可翻页查找）",
        "1 ·",
    )?;
    choose(
        browser,
        "zxs-merge-review",
        "Surviving branch",
        "合并后保留的分支",
        "Branch 000",
    )?;
    browser.click(&browser.button("Confirm merge of selected branches", "确认合并所选分支")?)?;
    check(
        browser,
        "merge request spans first and last candidate pages without a subset",
        "const write=window.__zxsHistoryPages.writes.find(write=>write.path.endsWith('/merge'));return write&&write.body.inputs.length===2&&write.body.inputs[0].branch_id==='history-branch-000'&&write.body.inputs[1].branch_id==='history-branch-200'&&write.body.inputs.every(input=>input.expected_node_id==='history-node-200')&&write.body.chosen_node_id==='history-node-200'&&write.body.surviving_branch_id==='history-branch-000'",
    )?;
    browser.click(&button(browser, "zxs-merge-review", "Cancel", "取消")?)?;
    check(
        browser,
        "merge cancel releases its selection view",
        "return !document.querySelector('.zxs-merge-review')",
    )?;
    Ok(())
}

fn cross_page_tags(browser: &Browser) -> Result<()> {
    browser.click(&browser.css("[data-node-id=history-node-200] .zxs-node-checkbox")?)?;
    browser.click(&browser.button("Edit selected node tags", "编辑所选节点标签")?)?;
    choose(
        browser,
        "zxs-tag-review",
        "Associated branch",
        "关联分支",
        "Branch 200",
    )?;
    branch_previous(browser, "zxs-tag-review")?;
    check(
        browser,
        "selected tag branch survives candidate replacement",
        "const select=document.querySelector('.zxs-tag-review .zxs-select');return select.querySelector('.zxs-select-value').textContent.includes('Branch 200')&&select.textContent.includes('Branch 100')&&select.querySelectorAll('[role=option]').length===101",
    )?;
    browser.keys(&browser.css(".zxs-tag-review input[aria-label='Add tag'],.zxs-tag-review input[aria-label='新增标签']")?, "reviewed")?;
    browser.click(&button(browser, "zxs-tag-review", "Add tag", "添加标签")?)?;
    browser.click(&browser.button("Save tag child nodes", "保存标签子节点")?)?;
    check(
        browser,
        "tag request uses explicitly selected last-page head",
        "const write=window.__zxsHistoryPages.writes.find(write=>write.path.endsWith('/tags'));return write&&write.body.targets.length===1&&write.body.targets[0].source_node_id==='history-node-200'&&write.body.targets[0].branch_id==='history-branch-200'&&write.body.targets[0].expected_node_id==='history-node-200'",
    )?;
    browser.click(&button(browser, "zxs-tag-review", "Cancel", "取消")?)?;
    browser.click(&browser.button("Clear selected nodes", "清除节点选择")?)?;
    Ok(())
}

fn editor_navigation(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Older nodes", "较早节点")?)?;
    check(
        browser,
        "later node page replaces first",
        "return document.querySelector('[data-node-id=history-node-150]')&&!document.querySelector('[data-node-id=history-node-200]')&&document.querySelectorAll('.zxs-version-node').length===50",
    )?;
    browser.click(&browser.css("[data-node-id=history-node-150] .zxs-version-note-action")?)?;
    let editor = browser.css(".zxs-version-node .zxs-inline-input")?;
    browser.keys(&editor, " retained draft")?;
    browser.script("const probe=window.__zxsHistoryPages;probe.editor=document.querySelector('.zxs-version-node .zxs-inline-input');probe.text=probe.editor.value;probe.mainCalls=probe.calls.filter(call=>call.path.endsWith('/history')).length;return true")?;
    for (en, zh) in [
        ("First page", "第一页"),
        ("Newer nodes", "较新节点"),
        ("Older nodes", "较早节点"),
    ] {
        browser.click(&browser.button(en, zh)?)?;
        check(
            browser,
            "navigation preserves the editor before blur",
            "const p=window.__zxsHistoryPages;return p.editor.isConnected&&document.activeElement===p.editor&&p.editor.value===p.text&&p.calls.filter(call=>call.path.endsWith('/history')).length===p.mainCalls",
        )?;
    }
    browser.click(&browser.css("[data-node-id=history-node-150] .zxs-version-parent-stub")?)?;
    check(
        browser,
        "anchor navigation preserves the active edit",
        "const p=window.__zxsHistoryPages;return p.editor.isConnected&&p.editor.value===p.text&&p.calls.filter(call=>call.path.endsWith('/history')).length===p.mainCalls",
    )?;
    browser.keys(&editor, "\u{e00c}")?;
    browser.click(&browser.button("First page", "第一页")?)?;
    check(
        browser,
        "cancel then first releases the former page",
        "return document.querySelector('[data-node-id=history-node-200]')&&!window.__zxsHistoryPages.editor.isConnected&&document.querySelectorAll('.zxs-version-node').length===50",
    )?;
    check(
        browser,
        "selected branch rename is ready",
        "return Array.from(document.querySelectorAll('button')).some(button=>['Rename file branch','重命名文件分支'].includes(button.getAttribute('aria-label'))&&!button.disabled)",
    )?;
    browser.click(&browser.button("Rename file branch", "重命名文件分支")?)?;
    let branch_editor = browser.css(".zxs-file-branches .zxs-inline-input")?;
    browser.keys(&branch_editor, " retained branch")?;
    browser.script("const p=window.__zxsHistoryPages;p.branchEditor=document.querySelector('.zxs-file-branches .zxs-inline-input');p.branchText=p.branchEditor.value;return true")?;
    branch_next(browser, "zxs-file-branches")?;
    check(
        browser,
        "branch paging does not blur-cancel a branch name",
        "const p=window.__zxsHistoryPages;return p.branchEditor.isConnected&&p.branchEditor.value===p.branchText&&document.activeElement===p.branchEditor",
    )?;
    browser.keys(&branch_editor, "\u{e00c}")?;
    Ok(())
}

fn parent_pages(browser: &Browser) -> Result<()> {
    browser.click(&browser.css("[data-node-id=history-node-200] .zxs-version-parent-more")?)?;
    check(
        browser,
        "parent first page",
        "const panel=document.querySelector('.zxs-version-parents-panel');return panel&&panel.textContent.includes('parent-000')&&panel.textContent.includes('parent-099')&&!panel.textContent.includes('parent-100')",
    )?;
    browser.click(&button(
        browser,
        "zxs-version-parents-panel",
        "Next",
        "下一页",
    )?)?;
    check(
        browser,
        "parent middle page",
        "return document.querySelector('.zxs-version-parents-panel').textContent.includes('parent-100')",
    )?;
    browser.click(&button(
        browser,
        "zxs-version-parents-panel",
        "Next",
        "下一页",
    )?)?;
    check(
        browser,
        "parent last page",
        "const panel=document.querySelector('.zxs-version-parents-panel');return panel.textContent.includes('parent-200')&&!panel.textContent.includes('parent-100')",
    )?;
    browser.click(&button(
        browser,
        "zxs-version-parents-panel",
        "Previous",
        "上一页",
    )?)?;
    check(
        browser,
        "parent previous middle page",
        "return document.querySelector('.zxs-version-parents-panel').textContent.includes('parent-100')",
    )?;
    browser.click(&button(
        browser,
        "zxs-version-parents-panel",
        "Previous",
        "上一页",
    )?)?;
    check(
        browser,
        "parent previous first page",
        "return document.querySelector('.zxs-version-parents-panel').textContent.includes('parent-000')",
    )?;
    browser.click(&button(
        browser,
        "zxs-version-parents-panel",
        "Close",
        "关闭",
    )?)?;
    check(
        browser,
        "closing releases parent DOM",
        "return !document.querySelector('.zxs-version-parents-panel')",
    )?;
    Ok(())
}

fn failures_and_cancellation(browser: &Browser) -> Result<()> {
    for failure in [json!(429), json!(500), json!("json")] {
        browser.script(&format!(
            "window.__zxsHistoryPages.branchFailure={failure};return true"
        ))?;
        browser.click(&browser.button("Refresh", "刷新")?)?;
        browser.button("Retry branches", "重试分支读取")?;
        let calls = browser.script("return window.__zxsHistoryPages.calls.filter(call=>call.path.endsWith('/branches')).length")?.as_u64().ok_or("branch call count missing")?;
        assert_idle_branch_failure(browser, calls)?;
        browser.click(&browser.button("Retry branches", "重试分支读取")?)?;
        check(
            browser,
            "one explicit branch retry",
            &format!(
                "return window.__zxsHistoryPages.calls.filter(call=>call.path.endsWith('/branches')).length==={}&&Array.from(document.querySelectorAll('button')).some(button=>['Retry branches','重试分支读取'].includes(button.textContent.trim())&&!button.disabled)",
                calls + 1
            ),
        )?;
        assert_idle_branch_failure(browser, calls + 1)?;
        browser.script("window.__zxsHistoryPages.branchFailure=null;return true")?;
        browser.click(&browser.button("Retry branches", "重试分支读取")?)?;
        check(
            browser,
            "successful explicit retry restores one branch page",
            "return document.querySelector('.zxs-file-branches .zxs-select').querySelectorAll('[role=option]').length===100&&!Array.from(document.querySelectorAll('button')).some(button=>['Retry branches','重试分支读取'].includes(button.textContent.trim()))",
        )?;
    }
    browser.script("window.__zxsHistoryPages.holdBranches=true;return true")?;
    browser.click(&browser.button("Refresh", "刷新")?)?;
    check(
        browser,
        "branch request held",
        "return window.__zxsHistoryPages.pending.length===1",
    )?;
    browser.script("const p=window.__zxsHistoryPages;p.holdBranches=false;p.root='0000000000000000000000000000000000000000000000000000000000000002';return true")?;
    browser.click(&browser.button("Refresh", "刷新")?)?;
    check(
        browser,
        "new root cancels old request and starts one fresh branch page",
        "const p=window.__zxsHistoryPages;return p.aborted===1&&p.calls.at(-1).root===p.root&&document.querySelector('.zxs-file-branches .zxs-select').querySelectorAll('[role=option]').length===100",
    )?;
    browser.script("window.__zxsHistoryPages.pending[0].release();return true")?;
    check(
        browser,
        "old root response delivered but cannot replace current page",
        "const p=window.__zxsHistoryPages;return p.delivered===1&&document.querySelector('.zxs-file-branches .zxs-select').querySelectorAll('[role=option]').length===100&&document.querySelectorAll('.zxs-version-node').length===50",
    )?;
    Ok(())
}
