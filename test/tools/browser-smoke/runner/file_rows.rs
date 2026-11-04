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
        "rowSurfaceShown",
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

fn search_surface(browser: &Browser) -> Result<()> {
    for position in [0.03, 0.5, 0.97] {
        let point = browser.script(&format!(
            "document.activeElement.blur();const f=document.querySelector('.zxs-file-search');f.scrollIntoView({{block:'center'}});const r=f.getBoundingClientRect();return {{x:r.left+r.width*{position},y:r.top+r.height/2}}"
        ))?;
        for kind in ["mousePressed", "mouseReleased"] {
            browser.cdp(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"button":"left","clickCount":1,"x":point["x"],"y":point["y"]}),
            )?;
        }
        if browser.script("const i=document.querySelector('.zxs-file-search input');return document.activeElement===i&&i.matches(':placeholder-shown')&&i.getAttribute('aria-label')===i.placeholder")? != true {
            return Err("search surface click did not focus its empty named input".into());
        }
    }
    accessibility::input_surfaces(browser)
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
    let pointer = browser.script("const row=document.querySelector('.zxs-entry-checkbox').closest('tr');row.scrollIntoView({block:'center'});const r=row.getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height/2}")?;
    browser.cdp(
        "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":pointer["x"],"y":pointer["y"]}),
    )?;
    wait_for("pointer hover shows the shared rounded row surface", || {
        if browser.script("const row=document.querySelector('.zxs-entry-checkbox').closest('tr'),surface=row.querySelector('.zxs-file-row-surface');return row.matches(':hover')&&getComputedStyle(surface).opacity==='1'&&surface.dataset.zxsContourState==='paint'")? == true {
            Ok(())
        } else {
            Err("file hover did not reveal G4 row surface".into())
        }
    })?;
    page_layout::file_row_geometry(browser)?;
    browser.cdp(
        "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":0,"y":0}),
    )?;
    wait_for("leaving an unchecked row clears its hover", || {
        if browser.script("const row=document.querySelector('.zxs-entry-checkbox').closest('tr');return !row.matches(':hover')&&getComputedStyle(row.querySelector('.zxs-file-row-surface')).opacity==='0'")? == true {
            Ok(())
        } else {
            Err("file row retained a stale hover".into())
        }
    })?;
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
        let snapshot = browser.script("const s=window.__zxsFileRow;return {rowConnected:s.row.isConnected,buttonSame:s.row.querySelector('.zxs-file-name')===s.button,checkboxSame:s.row.querySelector('.zxs-entry-checkbox')===s.check,checked:s.check.checked,rowSelected:s.row.getAttribute('aria-selected')==='true',rowSurfaceShown:getComputedStyle(s.row.querySelector('.zxs-file-row-surface')).opacity==='1',focused:document.activeElement===s.check}")?;
        assert_retained_row(snapshot)
    })?;
    search_surface(browser)?;
    let search = browser.css(".zxs-file-search input")?;
    browser.keys(&search, "browser-smoke")?;
    // Composition-confirming Enter and held Enter are not search submissions.
    // These two events exercise the guard; the actual submission below uses WebDriver.
    browser.script("const i=document.querySelector('.zxs-file-search input');i.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',isComposing:true,bubbles:true,cancelable:true}));i.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',repeat:true,bubbles:true,cancelable:true}));return true")?;
    let retained = browser.command(
        "POST",
        "/execute/async",
        Some(json!({"script":"const done=arguments[arguments.length-1];setTimeout(()=>{const i=document.querySelector('.zxs-file-search input'),s=window.__zxsFileRow;done(i.value==='browser-smoke'&&!i.matches(':placeholder-shown')&&i.getAttribute('aria-label')===i.placeholder&&document.activeElement===i&&s.row.isConnected&&s.check.checked&&s.row.getAttribute('aria-selected')==='true')},350)","args":[]})),
    )?;
    if retained != true {
        return Err("editing or guarded Enter replaced the selected row before submission".into());
    }
    accessibility::input_surfaces(browser)?;
    browser.keys(&search, "\u{e007}")?;
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
    browser.keys(&search, "\u{e007}")?;
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
