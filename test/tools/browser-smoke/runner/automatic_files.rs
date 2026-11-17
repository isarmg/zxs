//! Open the actual Chrome management UI over the exact sealed server binary.
//! One visible node, two manual first-commit actions/attempts, and one
//! protected Unborn draft must never be combined into a fake total.
use super::{Browser, Result, first_commit_tabs, json, wait_for};
use storage_contract::Principal;
use storage_engine::{Engine, Repository};

mod fixture;
pub(super) struct Evidence {
    file: String,
    node: String,
    save: String,
    operation: String,
    published: u64,
    trigger: String,
}
pub(super) fn seed(engine: &mut Engine, repo: &Repository, actor: &Principal) -> Result<Evidence> {
    fixture::seed(engine, repo, actor)
}
fn until(browser: &Browser, label: &'static str, script: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(label.into())
        }
    })
}
fn disk(browser: &Browser, origin: &str, password: &str, slot: &str) -> Result<()> {
    first_commit_tabs::ready(browser, origin, password)?;
    browser.click(&browser.css(&format!(
        ".zxs-disk-entry[data-slot='{slot}']:not([disabled])"
    ))?)?;
    Ok(())
}
fn open_window(browser: &Browser) -> Result<()> {
    browser.css("[data-zxs-automatic-observation]")?;
    if browser.script("return !document.querySelector('[data-zxs-automatic-observation]').open")?
        == true
    {
        browser.click(&browser.css("[data-zxs-automatic-observation] > summary")?)?;
    }
    Ok(())
}
fn window(browser: &Browser, start: u64, end: u64) -> Result<()> {
    let code = format!(
        r#"const p=document.querySelector('[data-zxs-automatic-observation]');
const inputs=p.querySelectorAll('input[type=datetime-local]');
if(inputs.length!==2) return false;
for (const [index,sec] of [{start},{end}].entries()) {{
  inputs[index].value=new Date(sec*1000).toISOString().slice(0,19);
  inputs[index].dispatchEvent(new Event('input',{{bubbles:true}}));
}}
return !p.querySelector('[data-zxs-automatic-publication-page]');"#
    );
    if browser.script(&code)? != true {
        return Err("new UTC inputs must clear previous private observation".into());
    }
    browser.click(&browser.button("Read window", "查询时间窗")?)?;
    browser.css("[data-zxs-automatic-publication-page]")?;
    Ok(())
}
fn exact(browser: &Browser, e: &Evidence) -> Result<()> {
    until(
        browser,
        "real independent published node and source operation only",
        &format!(
            r#"const p=document.querySelector('[data-zxs-automatic-publication-page]');
if(!p)return false;
const status=p.querySelector('[role=status]').textContent;
const files=[...p.querySelectorAll('li details > summary')];
const nodes=[...p.querySelectorAll('li code')];
return /(?:Complete for this window|已完整读取此时间窗)/.test(status)
 && /(?:Stable files|稳定文件): 1(?: |$)/.test(status)
 && /(?:Distinct automatically published nodes|独立自动发表节点): 1(?: |$)/.test(status)
 && files.length===1&&files[0].textContent.includes({file})
 && nodes.length===1&&nodes[0].textContent.includes({node})&&nodes[0].textContent.includes({op})
 && !p.textContent.includes({save})&&!p.textContent.includes({trigger});"#,
            file = json!(e.file),
            node = json!(e.node),
            op = json!(e.operation),
            save = json!(e.save),
            trigger = json!(e.trigger)
        ),
    )
}
pub(super) fn run(
    browser: &Browser,
    origin: &str,
    password: &str,
    slot: &str,
    e: &Evidence,
) -> Result<()> {
    disk(browser, origin, password, slot)?;
    open_window(browser)?;
    // The preceding real browser case also published a manual node in this
    // same pool. The entire interval must still contain only the native one.
    window(browser, 0, 253_402_300_799)?;
    exact(browser, e)?;
    window(
        browser,
        e.published
            .checked_sub(1)
            .ok_or("fixture must have an earlier UTC second")?,
        e.published,
    )?;
    until(
        browser,
        "UTC upper endpoint excludes the actual original",
        r#"const p=document.querySelector('[data-zxs-automatic-publication-page]');return !!p
 && !p.querySelector('li code')&&/(?:Distinct automatically published nodes|独立自动发表节点): 0(?: |$)/.test(p.textContent);"#,
    )?;
    window(
        browser,
        e.published,
        e.published
            .checked_add(1)
            .ok_or("fixture must have a later UTC second")?,
    )?;
    exact(browser, e)?;

    let file=browser.element("xpath","//button[contains(@class,'zxs-file-name') and normalize-space(.)='frequency-protected.txt']")?;
    browser.click(&file)?;
    until(
        browser,
        "real saved Unborn policy publishes a private warning without a node",
        &format!(
            r#"const w=document.querySelector('[data-zxs-automatic-protection-warning]');return !!w&&w.textContent.includes({})&&/(?:Save frequency exceeded|保存过于频繁)/.test(w.textContent)
 && !!document.querySelector('[data-zxs-resume-automatic]:not([disabled])');"#,
            json!(e.trigger)
        ),
    )?;
    browser.click(&browser.css("[data-zxs-resume-automatic]:not([disabled])")?)?;
    until(
        browser,
        "explicit original epoch resume clears only protection",
        r#"const w=document.querySelector('[data-zxs-file-automatic-state]');return !!w&&!w.querySelector('[data-zxs-automatic-protection-warning]')
 && /(?:records no published automatic node|未记录已发表的自动节点)/.test(w.textContent);"#,
    )?;
    disk(browser, origin, password, slot)?;
    open_window(browser)?;
    window(browser, 0, 253_402_300_799)?;
    exact(browser, e)
}
