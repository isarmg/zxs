//! Real DOM regression: checkbox updates retain rows; server queries reset selection.
use super::*;

struct Probe<'a>(&'a Browser);
impl Drop for Probe<'_> {
    fn drop(&mut self) {
        let _ = self.0.script("delete window.__zxsFileRow;return true");
    }
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script("const button=Array.from(document.querySelectorAll('.zxs-file-name')).find(e=>e.textContent.trim()==='browser-smoke.txt');const row=button.closest('tr');window.__zxsFileRow={row,button,check:row.querySelector('.zxs-entry-checkbox')};return true")?;
    let _probe = Probe(browser);
    let checkbox = browser.element("xpath", "//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']]//input[@type='checkbox']")?;
    browser.click(&checkbox)?;
    wait_for("checked file row retains its DOM and focus", || {
        if browser.script("const s=window.__zxsFileRow;return s.row.isConnected&&s.row.querySelector('.zxs-file-name')===s.button&&s.row.querySelector('.zxs-entry-checkbox')===s.check&&s.check.checked&&document.activeElement===s.check")? == true {
            Ok(())
        } else {
            Err("checking a file replaced row DOM or lost keyboard focus".into())
        }
    })?;
    let search = browser.css(".zxs-file-search input")?;
    browser.keys(&search, "browser-smoke")?;
    wait_for(
        "server search resets page-local selection and preserves input focus",
        || {
            if browser.script("const input=document.querySelector('.zxs-file-search input');return input.value==='browser-smoke'&&document.activeElement===input&&document.querySelectorAll('.zxs-file-table tbody tr').length===1&&!document.querySelector('.zxs-entry-checkbox').checked&&!window.__zxsFileRow.row.isConnected&&document.querySelector('.zxs-file-name').textContent.trim()==='browser-smoke.txt'")? == true {
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
