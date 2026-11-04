//! Real DOM regression: checkbox updates retain rows; server queries reset selection.
use super::*;

struct Probe<'a>(&'a Browser);
impl Drop for Probe<'_> {
    fn drop(&mut self) {
        let _ = self.0.script("delete window.__zxsFileRow;return true");
    }
}

fn assert_retained_row(snapshot: Value) -> Result<()> {
    if [
        "rowConnected",
        "buttonSame",
        "checkboxSame",
        "checked",
        "rowSelected",
        "focused",
    ]
    .iter()
    .all(|key| snapshot[key] == true)
    {
        Ok(())
    } else {
        Err(format!("checked row conditions failed: {snapshot}").into())
    }
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    // Opening Details retains an entry for that page, not a checked file row.
    // Return through real navigation with the pointer still outside the table.
    browser.click(&browser.element("xpath", "//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']]//button[normalize-space(.)='Details' or normalize-space(.)='详情']")?)?;
    browser.css(".zxs-entry-page")?;
    browser.click(&browser.button("Files", "文件")?)?;
    browser.css(".zxs-entry-checkbox")?;
    wait_for(
        "returning from details does not select an unchecked row",
        || {
            if browser.script("const check=document.querySelector('.zxs-entry-checkbox'),row=check.closest('tr');return !check.checked&&row.getAttribute('aria-selected')==='false'&&!row.matches(':hover')&&getComputedStyle(row).backgroundColor==='rgba(0, 0, 0, 0)'")? == true { Ok(()) } else { Err("detail entry leaked into checkbox selection or resting background".into()) }
        },
    )?;
    let initial = browser.script("const check=document.querySelector('.zxs-entry-checkbox');return {checked:check?.checked,disabled:check?.disabled,batchOpen:!!document.querySelector('.zxs-batch-editor')}")?;
    println!("INFO file_rows entry: {initial}");
    if initial["checked"] != false || initial["disabled"] != false || initial["batchOpen"] != false
    {
        return Err(format!(
            "file_rows requires an unchecked, enabled row without a batch: {initial}"
        )
        .into());
    }
    browser.script("const button=Array.from(document.querySelectorAll('.zxs-file-name')).find(e=>e.textContent.trim()==='browser-smoke.txt');const row=button.closest('tr');window.__zxsFileRow={row,button,check:row.querySelector('.zxs-entry-checkbox')};return true")?;
    let _probe = Probe(browser);
    let checkbox = browser.element("xpath", "//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']]//input[@type='checkbox']")?;
    browser.click(&checkbox)?;
    wait_for("checked file row retains its DOM and focus", || {
        let snapshot = browser.script("const s=window.__zxsFileRow;return {rowConnected:s.row.isConnected,buttonSame:s.row.querySelector('.zxs-file-name')===s.button,checkboxSame:s.row.querySelector('.zxs-entry-checkbox')===s.check,checked:s.check.checked,rowSelected:s.row.getAttribute('aria-selected')==='true',focused:document.activeElement===s.check}")?;
        assert_retained_row(snapshot)
    })?;
    let search = browser.css(".zxs-file-search input")?;
    browser.keys(&search, "browser-smoke")?;
    wait_for(
        "server search resets page-local selection and preserves input focus",
        || {
            if browser.script("const input=document.querySelector('.zxs-file-search input');return input.value==='browser-smoke'&&document.activeElement===input&&document.querySelectorAll('.zxs-file-table tbody tr').length===1&&!document.querySelector('.zxs-entry-checkbox').checked&&document.querySelector('.zxs-entry-checkbox').closest('tr').getAttribute('aria-selected')==='false'&&!window.__zxsFileRow.row.isConnected&&document.querySelector('.zxs-file-name').textContent.trim()==='browser-smoke.txt'")? == true {
            Ok(())
        } else {
            Err("server query did not reset selection or retain search focus".into())
        }
        },
    )?;
    browser.script("const input=document.querySelector('.zxs-file-search input');input.value='';input.dispatchEvent(new Event('input',{bubbles:true}));return true")?;
    wait_for("cleared file query finishes loading", || {
        if browser.script("return document.querySelector('.zxs-file-search input').value===''&&document.querySelectorAll('.zxs-file-table tbody tr').length===1&&document.querySelector('.zxs-file-name').textContent.trim()==='browser-smoke.txt'&&!document.querySelector('[aria-busy=true]')")? == true {
            Ok(())
        } else {
            Err("clearing query has not restored the real row".into())
        }
    })?;
    println!(
        "PASS keyed file-row DOM, checkbox focus, server-query selection reset and input focus"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/file_rows_tests.rs"]
mod tests;
