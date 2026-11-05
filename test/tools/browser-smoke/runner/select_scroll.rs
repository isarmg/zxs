//! Real reversing wheel input with synchronous scroll and RAF geometry samples.
//! The monitor has five genuine options; a short viewport creates overflow.
use super::*;

const PANEL: &str = ".zxs-monitor-range .zxs-select-panel:popover-open";
const TRIGGER: &str = ".zxs-monitor-range .zxs-select-trigger";

fn observe(browser: &Browser, operation: &str) -> Result<Value> {
    browser.command(
        "POST",
        "/execute/async",
        Some(json!({"script":include_str!("select_scroll.js"),"args":[operation,PANEL]})),
    )
}

fn gesture(browser: &Browser, theme: &str) -> Result<()> {
    let started = observe(browser, "start")?;
    if started["valid"] != true {
        return Err(format!("Select fast-scroll observer could not start: {started}").into());
    }
    let actions: Vec<Value> = [120, -120, 120, -120, 120, -120, 120, -120]
        .into_iter()
        .map(|delta| {
            json!({"type":"scroll","duration":100,"origin":"viewport",
                "x":started["x"],"y":started["y"],"deltaX":0,"deltaY":delta})
        })
        .collect();
    browser.command(
        "POST",
        "/actions",
        Some(json!({"actions":[{"type":"wheel","id":"select-scroll-wheel","actions":actions}]})),
    )?;
    let evidence = observe(browser, "finish")?;
    let failed = evidence["valid"] != true;
    let output = env::var_os("ZXS_VIEWPORT_PREVIEWS").map(PathBuf::from);
    if let Some(directory) =
        output.or_else(|| failed.then(|| PathBuf::from("artifacts/browser-smoke/diagnostics")))
    {
        fs::create_dir_all(&directory)?;
        fs::write(
            directory.join(format!("select-fast-scroll-{theme}.json")),
            json_codec::to_vec_pretty(&evidence)?,
        )?;
    }
    if failed {
        return Err(format!(
            "Select border separated or no in-motion scroll evidence in {theme}: {evidence}"
        )
        .into());
    }
    palette_select_borders(browser, theme)?;
    println!(
        "PASS Select {theme} fast wheel reversals: {} moving frames, {} native scroll samples; local palette-derived border remains on its outer card",
        evidence["movingFrames"], evidence["trustedScrolls"]
    );
    Ok(())
}

/// Called from the History page after the viewport focus checks have restored
/// normal device metrics. Restore that page, scroll position, and metrics on error.
pub(super) fn run(browser: &Browser, theme: &str) -> Result<()> {
    let origin = browser.script(
        "if(!document.querySelector('.zxs-history-toolbar'))throw new Error('Select scroll check must start on History');return {x:scrollX,y:scrollY}",
    )?;
    with_cleanup(
        || {
            browser.cdp(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":980,"height":260,"deviceScaleFactor":1,"mobile":false}),
            )?;
            browser.click(&browser.button("Status", "状态")?)?;
            let trigger = browser.css(TRIGGER)?;
            browser.script(
                "document.querySelector('.zxs-monitor-range .zxs-select-trigger').scrollIntoView({block:'center',behavior:'instant'});return true",
            )?;
            // Let the native scroll finish before opening: scrolling outside a
            // Select intentionally dismisses it, including a queued scroll event.
            browser.command(
                "POST",
                "/execute/async",
                Some(json!({"script":"const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))","args":[]})),
            )?;
            browser.click(&trigger)?;
            visible_menu(browser, false, theme, "before fast scrolling")?;
            palette_select_borders(browser, theme)?;
            gesture(browser, theme)
        },
        || {
            // Attempt every cleanup even if an earlier one failed.
            let observer = observe(browser, "cleanup").and_then(|result| {
                if result["valid"] == true {
                    Ok(())
                } else {
                    Err(format!("Select scroll observer cleanup failed: {result}").into())
                }
            });
            let close = (|| -> Result<()> {
                if browser.script("return !!document.querySelector('.zxs-monitor-range .zxs-select-panel:popover-open')")? == true {
                    browser.keys(&browser.css(TRIGGER)?, "\u{e00c}")?;
                }
                Ok(())
            })();
            let metrics = browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}));
            let history = (|| browser.click(&browser.button("History", "历史记录")?))();
            let scroll = browser.script(&format!(
                "window.scrollTo({{left:{},top:{},behavior:'instant'}});return true",
                origin["x"], origin["y"]
            ));
            observer?;
            close?;
            metrics?;
            history?;
            scroll?;
            Ok(())
        },
    )
}

#[cfg(test)]
#[path = "../unit/select_scroll_tests.rs"]
mod tests;
