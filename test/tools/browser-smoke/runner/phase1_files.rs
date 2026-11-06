//! Phase-one file UI checks. Only the disk-response delivery failure is
//! synthetic; tag writes and subsequent history reads use the real service.
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

fn evidence(browser: &Browser, name: &str, expression: &str) -> Result<()> {
    let directory = Path::new("artifacts/browser-smoke/diagnostics/phase1-files");
    fs::create_dir_all(directory)?;
    fs::write(
        directory.join(format!("{name}.json")),
        json_codec::to_vec_pretty(&browser.script(expression)?)?,
    )?;
    let encoded = browser.command("GET", "/screenshot", None)?;
    let png = STANDARD.decode(encoded.as_str().ok_or("missing phase-one screenshot")?)?;
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("phase-one screenshot is not a PNG".into());
    }
    fs::write(directory.join(format!("{name}.png")), png)?;
    Ok(())
}

pub(super) fn disk_read(browser: &Browser) -> Result<()> {
    browser.script(include_str!("phase1_disk_read.js"))?;
    with_cleanup(
        || {
            browser.script(r#"
                const p=window.__zxsPhase1DiskRead;
                p.rows=()=>Array.from(document.querySelectorAll('.zxs-disk-table tbody tr'),row=>{
                    const button=row.querySelector('.zxs-disk-entry');
                    return {slot:button.dataset.slot,name:button.textContent.trim(),
                        disabled:button.disabled,online:row.dataset.online,
                        access:row.cells[1].textContent.trim(),space:row.cells[2].textContent.trim()};
                });
                p.cached=p.rows();
                if(p.cached.length!==3||document.querySelector('[data-disk-list-read]'))
                    throw new Error('disk probe needs the real current three-disk list');
                return true;
            "#)?;
            check(
                browser,
                "one real health response held before manual disk retry",
                "return window.__zxsPhase1DiskRead.statusHeld",
            )?;
            browser.click(&browser.css(".zxs-header-refresh:not([disabled])")?)?;
            check(
                browser,
                "disk refresh marks retained rows as previous results",
                r#"
                const p=window.__zxsPhase1DiskRead;
                return p.reads.length===1&&p.reads[0].status===200&&!!p.reads[0].release
                    &&document.querySelector('[data-disk-list-read="refreshing-cached"] [role=status]')
                    &&!document.querySelector('[data-disk-list-retry]')
                    &&JSON.stringify(p.rows())===JSON.stringify(p.cached)
                    &&document.querySelector('.zxs-header-refresh').disabled;
            "#,
            )?;
            browser.script("window.__zxsPhase1DiskRead.release(0,true);return true")?;
            check(
                browser,
                "failed disk delivery retains rows and exposes explicit retry",
                r#"
                const p=window.__zxsPhase1DiskRead;
                return p.reads.length===1&&p.reads[0].delivered
                    &&!!document.querySelector('[data-disk-list-read="unconfirmed-cached"] [role=status]')
                    &&!!document.querySelector('[data-disk-list-retry]:not([disabled])')
                    &&JSON.stringify(p.rows())===JSON.stringify(p.cached)
                    &&document.querySelector('.zxs-error-center')?.textContent.includes(p.failure);
            "#,
            )?;
            evidence(
                browser,
                "disk-unconfirmed",
                "const p=window.__zxsPhase1DiskRead;return {scope:'synthetic delivery failure after real HTTP 200',rows:p.rows(),requests:p.reads.map(r=>({status:r.status,delivered:r.delivered})),marker:document.querySelector('[data-disk-list-read]').dataset.diskListRead}",
            )?;
            browser.script("window.__zxsPhase1DiskRead.retry=document.querySelector('[data-disk-list-retry]');return true")?;
            browser.click(&browser.css("[data-disk-list-retry]:not([disabled])")?)?;
            check(
                browser,
                "the retry button starts one real held GET",
                r#"
                const p=window.__zxsPhase1DiskRead;
                return p.reads.length===2&&p.reads[1].status===200&&!!p.reads[1].release
                    &&!!document.querySelector('[data-disk-list-read="refreshing-cached"]')
                    &&!document.querySelector('[data-disk-list-retry]')
                    &&JSON.stringify(p.rows())===JSON.stringify(p.cached);
            "#,
            )?;
            // The actual button is now detached/disabled. Its old event closure
            // still must not admit another request while the current GET owns it.
            browser.script(
                "const p=window.__zxsPhase1DiskRead;p.retry.click();p.retry.click();return true",
            )?;
            if browser.script("return window.__zxsPhase1DiskRead.reads.length===2")? != true {
                return Err("repeated disk retry admitted a duplicate GET".into());
            }
            browser.script("window.__zxsPhase1DiskRead.release(1);return true")?;
            check(
                browser,
                "real retry refreshes the list without erasing its diagnostic",
                r#"
                const p=window.__zxsPhase1DiskRead,rows=p.rows(),disks=p.reads[1].body.disks;
                return p.reads.length===2&&p.reads[1].delivered
                    &&!document.querySelector('[data-disk-list-read],[data-disk-list-retry]')
                    &&!document.querySelector('.zxs-header-refresh').disabled
                    &&rows.length===disks.length&&rows.every((row,index)=>{
                        const disk=disks[index];
                        return row.slot===disk.slot_id&&row.name===disk.name&&row.disabled===!disk.online
                            &&row.online===String(disk.online)
                            &&(disk.online?(disk.writable?['Read and write','可读写']:['Read only','只读']):['Offline','离线']).includes(row.access);
                    })&&document.querySelector('.zxs-error-center')?.textContent.includes(p.failure);
            "#,
            )?;
            evidence(
                browser,
                "disk-retried",
                "const p=window.__zxsPhase1DiskRead;return {scope:'explicit button followed by original real response',rows:p.rows(),requests:p.reads.map(r=>({status:r.status,delivered:r.delivered})),diagnosticRetained:document.querySelector('.zxs-error-center').textContent.includes(p.failure)}",
            )?;
            browser.click(&browser.element("xpath", "//aside[contains(@class,'zxs-error-center')]/div[contains(.,'Browser smoke: phase-one disk response delivery interrupted')]//button")?)?;
            check(
                browser,
                "only the explicit dismissal removes the disk diagnostic",
                "return !document.querySelector('.zxs-error-center')?.textContent.includes(window.__zxsPhase1DiskRead.failure)",
            )?;
            Ok(())
        },
        || {
            browser.script("window.__zxsPhase1DiskRead?.restore();return true")?;
            Ok(())
        },
    )?;
    println!(
        "PASS disk delivery failure retains cached rows and access flags; explicit Read again sends one real GET and retains the diagnostic until dismissal (delivery failure is synthetic)"
    );
    Ok(())
}

fn edit_current_tags(browser: &Browser) -> Result<()> {
    browser.click(&browser.element("xpath", "//article[contains(@class,'zxs-version-node')][.//span[contains(@class,'zxs-version-current')]]//button[normalize-space(.)='Edit tags' or normalize-space(.)='编辑标签']")?)?;
    browser.css(".zxs-tag-review input:not([disabled])")?;
    Ok(())
}

fn enter_tag(browser: &Browser) -> Result<()> {
    let input = browser.css(
        ".zxs-tag-review input[aria-label='Add tag'],.zxs-tag-review input[aria-label='新增标签']",
    )?;
    browser.keys(&input, "一期已审核")?;
    browser.keys(&input, "\u{e007}")
}

fn saved_render(browser: &Browser, index: usize) -> Result<()> {
    check(
        browser,
        "real tag receipt and retained body reach the displayed history",
        &format!(
            r#"
        const p=window.__zxsPhase1Tags,id=p.verify({index});
        if(!id||document.querySelector('.zxs-tag-review')||document.querySelector('.zxs-header-refresh').disabled)return false;
        const node=document.querySelector('[data-node-id="'+id+'"]');
        const saved=p.snapshots[{index}+1].nodes.find(node=>node.node_id===id);
        return node?.dataset.nodeKind==='tags'&&node.dataset.bodyVersionId===saved.version_id
            &&!!node.querySelector('.zxs-version-current')
            &&JSON.stringify(Array.from(node.querySelectorAll('.zxs-node-tag'),tag=>tag.textContent))===JSON.stringify(saved.tags);
    "#
        ),
    )
}

fn publish_tags(browser: &Browser, index: usize) -> Result<()> {
    browser.click(&browser.button("Save tag child nodes", "保存标签子节点")?)?;
    check(
        browser,
        "actual tag publication is held before response delivery",
        &format!(
            "const p=window.__zxsPhase1Tags;return p.writes.length==={index}+1&&!!p.writes[{index}].release&&p.writes[{index}].status===200&&!p.writes[{index}].delivered"
        ),
    )?;
    // Stimulate the real disabled controls, never the API or private Rust model.
    browser.script(r#"
        const panel=document.querySelector('.zxs-tag-review');
        const save=Array.from(panel.querySelectorAll('button')).find(button=>['Save tag child nodes','保存标签子节点'].includes(button.textContent.trim()));
        const cancel=Array.from(panel.querySelectorAll('button')).find(button=>['Cancel','取消'].includes(button.textContent.trim()));
        if(!save?.disabled||!cancel?.disabled)throw new Error('tag mutation does not fence repeat save/cancel');
        save.click();save.click();cancel.click();return true;
    "#)?;
    if browser.script(&format!("return window.__zxsPhase1Tags.writes.length==={index}+1&&!!document.querySelector('.zxs-tag-review')"))? != true {
        return Err("duplicate tag interaction escaped the pending publication fence".into());
    }
    browser.script("window.__zxsPhase1Tags.release();return true")?;
    saved_render(browser, index)?;
    let after = browser.script("return window.__zxsPhase1Tags.reads.length")?;
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    check(
        browser,
        "navigation rereads the same persisted tag history",
        &format!("return !!window.__zxsPhase1Tags.reread({index},{after})"),
    )?;
    saved_render(browser, index)
}

/// Run after the original two-node window fixtures. This only mutates the
/// runner's own seeded file, preserving its notes, versions and all old nodes.
pub(super) fn tags(browser: &Browser) -> Result<()> {
    browser.script(include_str!("phase1_tags.js"))?;
    with_cleanup(
        || {
            browser.click(&browser.css(".zxs-header-refresh:not([disabled])")?)?;
            check(
                browser,
                "real current history before editing tags",
                "const p=window.__zxsPhase1Tags;return p.reads.length>0&&!document.querySelector('.zxs-header-refresh').disabled&&!!document.querySelector('.zxs-version-current')",
            )?;
            browser.script("window.__zxsPhase1Tags.capture();return true")?;
            edit_current_tags(browser)?;
            enter_tag(browser)?;
            browser.click(&browser.element("xpath", "//section[contains(@class,'zxs-tag-review')]//button[normalize-space(.)='Cancel' or normalize-space(.)='取消']")?)?;
            check(
                browser,
                "cancelling tags does not publish",
                "const p=window.__zxsPhase1Tags;return !document.querySelector('.zxs-tag-review')&&p.writes.length===0&&p.unexpected.length===0",
            )?;
            edit_current_tags(browser)?;
            enter_tag(browser)?;
            publish_tags(browser, 0)?;
            evidence(
                browser,
                "tag-added",
                "const p=window.__zxsPhase1Tags,{release,...write}=p.writes[0];return {scope:'real tag publication and navigation reread',write,history:p.snapshots[1],unexpected:p.unexpected}",
            )?;
            edit_current_tags(browser)?;
            browser.click(&browser.css(".zxs-tag-review button[aria-label='Remove tag 一期已审核'],.zxs-tag-review button[aria-label='移除标签 一期已审核']")?)?;
            publish_tags(browser, 1)?;
            evidence(
                browser,
                "tag-removed",
                "const p=window.__zxsPhase1Tags,{release,...write}=p.writes[1];return {scope:'real removal child and navigation reread',write,history:p.snapshots[2],unexpected:p.unexpected}",
            )?;
            assert_metadata_only(browser)?;
            Ok(())
        },
        || {
            browser.script("window.__zxsPhase1Tags?.restore();return true")?;
            Ok(())
        },
    )?;
    println!(
        "PASS real Chrome tag cancel/add/remove, exact publication receipts, duplicate-click fencing and fresh history after navigation; original nodes and content versions retained, no Web body transfer"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/phase1_files_tests.rs"]
mod tests;
