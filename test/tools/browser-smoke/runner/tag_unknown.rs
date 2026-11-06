//! Lose delivery only after a real tag publication, then replay its frozen POST.
//! Repeated synthetic outcome 404s are backed by the same real completed receipt.
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

fn publish_and_lose_delivery(browser: &Browser) -> Result<()> {
    browser.click(&browser.css(".zxs-header-refresh:not([disabled])")?)?;
    check(
        browser,
        "real current history before the lost tag response",
        "return window.__zxsTagUnknown.reads.length>0&&!document.querySelector('.zxs-header-refresh').disabled&&!!document.querySelector('.zxs-version-current')",
    )?;
    browser.script("window.__zxsTagUnknown.capture();return true")?;
    // Reuse the regular helper so compact cards can open their current node.
    phase1_files::edit_current_tags(browser)?;
    let input = browser.css(
        ".zxs-tag-review input[aria-label='Add tag'],.zxs-tag-review input[aria-label='新增标签']",
    )?;
    browser.keys(&input, "恢复原请求")?;
    browser.keys(&input, "\u{e007}")?;
    browser.click(&browser.button("Save tag child nodes", "保存标签子节点")?)?;
    check(
        browser,
        "published tag delivery lost and two completed outcomes hidden by 404",
        r#"
        const p=window.__zxsTagUnknown,panel=document.querySelector('.zxs-tag-review');
        if(!p.uncertain()||!panel)return false;
        const buttons=Array.from(panel.querySelectorAll('button'));
        const save=buttons.find(b=>['Save tag child nodes','保存标签子节点'].includes(b.textContent.trim()));
        const cancel=buttons.find(b=>['Cancel','取消'].includes(b.textContent.trim()));
        return save?.disabled&&cancel?.disabled;
    "#,
    )?;
    browser.script("window.__zxsTagUnknown.stages.push('published response lost; repeated 404 keeps draft frozen');return true")?;
    Ok(())
}

fn recover_after_navigation(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    check(
        browser,
        "navigation keeps the original tag request and the global write fence",
        r#"
        const p=window.__zxsTagUnknown,create=Array.from(document.querySelectorAll('button'))
            .find(b=>['New folder','新建文件夹'].includes(b.textContent.trim()));
        return p.uncertain()&&!!document.querySelector('[data-tag-request-recovery]')
            &&!!document.querySelector('[data-tag-request-retry]:not([disabled])')
            &&!document.querySelector('.zxs-tag-review')
            &&document.querySelector('.zxs-file-manager')?.getAttribute('aria-busy')==='false'
            &&create?.disabled;
    "#,
    )?;
    browser.script(r#"
        const p=window.__zxsTagUnknown;
        for(const b of document.querySelectorAll('button'))
            if(['New folder','新建文件夹'].includes(b.textContent.trim())){b.click();b.click();}
        if(p.writes.length!==1||p.unexpected.length||document.querySelector('.zxs-inline-input'))
            throw new Error('404 or navigation removed the tag write fence');
        p.stages.push('different directory; original request recovery visible; namespace writes fenced');
        return true;
    "#)?;
    browser.click(&browser.css("[data-tag-request-retry]:not([disabled])")?)?;
    check(
        browser,
        "explicit retry receives the original real publication receipt",
        r#"
        const p=window.__zxsTagUnknown;
        return p.replay()&&!!p.writes[1].release&&!p.writes[1].delivered
            &&document.querySelector('[data-tag-request-retry]')?.disabled;
    "#,
    )?;
    browser.script(r#"
        const p=window.__zxsTagUnknown,button=document.querySelector('[data-tag-request-retry]');
        if(!button.disabled||document.visibilityState!=='visible')throw new Error('retry hold is not visible and fenced');
        button.click();button.click();p.heldAt=performance.now();return true;
    "#)?;
    // Cross the real 2-second operation polling interval, without changing it.
    check(
        browser,
        "held original retry excludes repeat POSTs and outcome polling",
        r#"
        const p=window.__zxsTagUnknown;
        return performance.now()-p.heldAt>=2300&&p.replay()&&p.writes.length===2
            &&!p.writes[1].delivered&&document.querySelector('[data-tag-request-retry]')?.disabled;
    "#,
    )?;
    browser.script("const p=window.__zxsTagUnknown;p.stages.push('exact original POST and receipt; duplicate click and poll fenced');p.release();return true")?;
    check(
        browser,
        "only the delivered original receipt clears recovery and write fencing",
        r#"
        const p=window.__zxsTagUnknown,create=Array.from(document.querySelectorAll('button'))
            .find(b=>['New folder','新建文件夹'].includes(b.textContent.trim()));
        return p.replay()&&p.writes[1].delivered&&!document.querySelector('[data-tag-request-recovery]')
            &&!!create&&!create.disabled;
    "#,
    )?;
    open_fixture_directory(browser)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    check(
        browser,
        "fresh original file history has exactly one tag child and unchanged body",
        r#"
        const p=window.__zxsTagUnknown,id=p.verify();if(!id)return false;
        const node=document.querySelector('[data-node-id="'+id+'"]');
        const actual=p.reads.at(-1).nodes.find(n=>n.node_id===id);
        return node?.dataset.nodeKind==='tags'&&node.dataset.bodyVersionId===actual.version_id
            &&!!node.querySelector('.zxs-version-current')
            &&JSON.stringify(Array.from(node.querySelectorAll('.zxs-node-tag'),tag=>tag.textContent))===JSON.stringify(actual.tags);
    "#,
    )?;
    browser.script("window.__zxsTagUnknown.stages.push('fresh history: one tag child; retained old nodes and body');return true")?;
    assert_metadata_only(browser)
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(include_str!("tag_unknown.js"))?;
    with_cleanup(
        || {
            publish_and_lose_delivery(browser)?;
            recover_after_navigation(browser)?;
            // Dismiss only this fault's notices. A real 429 or unrelated error
            // stays visible and remains subject to the runner's final checks.
            browser.script(
                r#"
                const p=window.__zxsTagUnknown;
                for(const row of document.querySelectorAll('.zxs-error-center > div'))
                    if(row.textContent.includes(p.marker))row.querySelector('button').click();
                return true;
            "#,
            )?;
            Ok(())
        },
        || {
            with_cleanup(
                || {
                    phase1_files::evidence(
                        browser,
                        "tag-unknown-recovery",
                        "return window.__zxsTagUnknown.snapshot()",
                    )
                },
                || {
                    browser.script("window.__zxsTagUnknown?.restore();return true")?;
                    Ok(())
                },
            )
        },
    )?;
    println!(
        "PASS real published tag response loss, repeated synthetic outcome 404 fencing, navigation and exact original POST retry; one retained-body tag child, no duplicate publication"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/tag_unknown_tests.rs"]
mod tests;
