//! Read-only navigation of the real seeded B01/C01 namespaces. No path writer,
//! content endpoint or test-owned alternative response is involved.
use super::{Browser, Result, Value, json, phase1_files, wait_for, with_cleanup};

fn mark(browser: &Browser) -> Result<Value> {
    browser.script("return window.__zxsBackupBrowse.mark()")
}

fn check(browser: &Browser, label: &str, expression: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(expression)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}

fn return_root(browser: &Browser) -> Result<()> {
    let after = mark(browser)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    check(
        browser,
        "fresh real All disks response after backup navigation",
        &format!("return window.__zxsBackupBrowse.rootReady({after})"),
    )?;
    browser.script(&format!(
        "return window.__zxsBackupBrowse.recordRoot({after})"
    ))?;
    Ok(())
}

fn click_entry(browser: &Browser, name: &str) -> Result<()> {
    let selector = browser.script(&format!(
        "return '.zxs-file-table:not(.zxs-disk-table) .zxs-file-name[title='+CSS.escape({})+']'",
        json!(name)
    ))?;
    browser.click(&browser.css(selector.as_str().ok_or("entry selector is not a string")?)?)
}

fn open_seed_file(browser: &Browser, slot: &str) -> Result<Value> {
    let mut after = mark(browser)?;
    browser.click(&browser.css(&format!(
        ".zxs-disk-entry[data-slot={}]:not([disabled])",
        json!(slot)
    ))?)?;
    let mut path = String::new();
    // A budget for this test's single-prefix-chain fixture, not a product limit.
    for _ in 0..8 {
        let args = format!("{},{},{after}", json!(slot), json!(path));
        check(
            browser,
            "real disk directory response, breadcrumb and access controls",
            &format!("return window.__zxsBackupBrowse.pageReady({args})"),
        )?;
        let entry = browser.script(&format!("return window.__zxsBackupBrowse.step({args})"))?;
        match entry["kind"].as_str() {
            Some("file") => return Ok(entry),
            Some("directory") => {
                let name = entry["name"].as_str().ok_or("directory name missing")?;
                path = entry["path"]
                    .as_str()
                    .ok_or("actual directory path missing")?
                    .to_owned();
                after = mark(browser)?;
                click_entry(browser, name)?;
            }
            _ => return Err("backup fixture returned an unsupported navigation kind".into()),
        }
    }
    Err("backup fixture exceeded its eight-page navigation budget".into())
}

fn backup_detail(browser: &Browser, slot: &str, file: &Value) -> Result<()> {
    let after = mark(browser)?;
    click_entry(browser, file["name"].as_str().ok_or("file name missing")?)?;
    let args = format!("{},{file},{after}", json!(slot));
    check(
        browser,
        "real backup file identity, history and read-only detail controls",
        &format!("return window.__zxsBackupBrowse.detailReady({args})"),
    )?;
    browser.script(&format!(
        "return window.__zxsBackupBrowse.recordDetail({args})"
    ))?;
    super::assert_metadata_only(browser)?;
    phase1_files::evidence(
        browser,
        &format!("backup-navigation-{slot}"),
        "return window.__zxsBackupBrowse.snapshot()",
    )?;
    browser.click(&browser.button("Files", "文件")?)?;
    browser.css(".zxs-breadcrumb")?;
    return_root(browser)
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(include_str!("backup_browse.js"))?;
    with_cleanup(
        || {
            browser.script("return window.__zxsBackupBrowse.start()")?;
            open_seed_file(browser, "P01")?;
            return_root(browser)?;
            for slot in ["B01", "C01"] {
                let file = open_seed_file(browser, slot)?;
                backup_detail(browser, slot, &file)?;
            }
            // Reads cannot change the file entry or leave a backup selection as
            // the baseline for the existing primary history and tag scenarios.
            open_seed_file(browser, "P01")?;
            return_root(browser)?;
            browser.script("return window.__zxsBackupBrowse.finish()")?;
            phase1_files::evidence(
                browser,
                "backup-navigation-restored",
                "return window.__zxsBackupBrowse.snapshot()",
            )
        },
        || {
            // Keep original responses and failure evidence even if a UI wait
            // fails. Cleanup does not silently replace another fetch owner.
            let saved = phase1_files::evidence(
                browser,
                "backup-navigation-final",
                "return window.__zxsBackupBrowse.snapshot()",
            );
            let restored = browser
                .script("window.__zxsBackupBrowse.restore();return true")
                .map(|_| ());
            with_cleanup(|| saved, || restored)
        },
    )?;
    println!(
        "PASS B01/C01 real root-to-file UI navigation, exact returned paths, same seeded file/version and read-only metadata; fresh All disks return and unchanged primary baseline (not tier-local body reads, SMB or recursive writer proof)"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/backup_browse_tests.rs"]
mod tests;
