//! An offline-prepared primary must open its own empty root before seeded file
//! scenarios can make an unrelated disk mask this transition.
use super::{Browser, Path, Result, wait_for};

pub(super) fn observe(browser: &Browser) -> Result<()> {
    browser.script(include_str!("primary_browse.js"))?;
    Ok(())
}

pub(super) fn restore(browser: &Browser) -> Result<()> {
    browser
        .script("if(window.__zxsPrimaryBrowse)window.__zxsPrimaryBrowse.restore();return true")?;
    Ok(())
}

pub(super) fn run(browser: &Browser, storage: &Path) -> Result<()> {
    let name = storage
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("new primary fixture has no UTF-8 directory name")?;
    let expected = json_codec::to_string(name)?;
    let mutations = browser.script("return window.__zxsPrimaryBrowse.mutations")?;
    browser.click(&browser.button("Files", "文件")?)?;
    wait_for(
        "newly admitted primary in the real disk response and Web list",
        || {
            if browser.script(&format!(
                r#"
const s=window.__zxsPrimaryBrowse;
const rows=[...document.querySelectorAll('.zxs-disk-entry')];
return s.diskReady({expected}) && rows.length===1 && rows[0].dataset.slot==='D01'
  && !rows[0].disabled && rows[0].textContent.trim()==={expected};
"#
            ))? == true
            {
                Ok(())
            } else {
                Err("new primary's actual name/list response not rendered".into())
            }
        },
    )?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='D01']:not([disabled])")?)?;
    wait_for(
        "new primary's real empty root rendered in the file page",
        || {
            if browser.script(&format!(r#"
const s=window.__zxsPrimaryBrowse;
const table=document.querySelector('.zxs-file-table:not(.zxs-disk-table)');
const breadcrumb=[...document.querySelectorAll('.zxs-breadcrumb button')];
const empty=document.querySelector('.zxs-file-pagination .zxs-file-empty');
return s.emptyRootReady() && table!==null && table.closest('[aria-busy]')?.getAttribute('aria-busy')==='false'
  && table.querySelectorAll('tbody tr').length===0 && breadcrumb.length===2
  && breadcrumb[1].textContent.trim()==={expected}
  && ['This folder is empty.','这个文件夹是空的。'].includes(empty?.textContent.trim());
"#))? == true { Ok(()) } else { Err("empty primary root response or actual file page not ready".into()) }
        },
    )?;
    let reads = browser.script("return window.__zxsPrimaryBrowse.diskReads.length")?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    wait_for(
        "return to all disks performs a fresh real list read",
        || {
            if browser.script(&format!(
                r#"
const s=window.__zxsPrimaryBrowse,rows=[...document.querySelectorAll('.zxs-disk-entry')];
return s.diskReads.length>{reads} && s.diskReady({expected})
  && rows.length===1 && rows[0].dataset.slot==='D01' && !rows[0].disabled
  && rows[0].textContent.trim()==={expected} && !document.querySelector('.zxs-breadcrumb');
"#
            ))? == true
            {
                Ok(())
            } else {
                Err("return navigation did not refresh the admitted primary list".into())
            }
        },
    )?;
    if browser.script("return window.__zxsPrimaryBrowse.mutations")? != mutations {
        return Err("new primary browsing sent an unexpected API mutation".into());
    }
    println!(
        "PASS offline-prepared primary to actual Files disk name, empty D01 root and fresh All disks return; no browsing mutations"
    );
    Ok(())
}
