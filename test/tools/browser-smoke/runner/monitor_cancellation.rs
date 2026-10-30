//! Synthetic delayed reads through production WASM; not backend telemetry evidence.
use super::*;

fn choose(browser: &Browser, range: &str) -> Result<()> {
    browser.click(&browser.css(".zxs-monitor-range .zxs-select > button")?)?;
    browser.click(&browser.element(
        "xpath",
        &format!("//*[contains(concat(' ',normalize-space(@class),' '),' zxs-monitor-range ')]//*[@role='option'][normalize-space(.)='{range}']"),
    )?)
}

// The metric header and G4 contour decorations also contain SVGs. Only the
// chart owns the selected range label and plotted history.
const CURRENT_CHART: &str = "document.querySelector('[data-metric=cpu] .zxs-metric-chart svg[role=img][aria-label=\"CPU · 24h\"] .zxs-chart-line')!==null&&document.querySelector('.zxs-monitor-range .zxs-select > button').textContent.trim()==='24h'";

fn check(browser: &Browser, label: &str, expression: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(expression)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(r#"
        const original=window.fetch;
        const probe=window.__zxsMonitorReads={aborted:0,delivered:0,pending:null,restore:()=>{window.fetch=original;if(probe.pending)probe.pending.release();}};
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            if(url.pathname!=='/api/system/history')return original.apply(this,args);
            const range=url.searchParams.get('range');
            if(range!=='30min'&&range!=='24h')return original.apply(this,args);
            if(range==='30min'){
                let release;const gate=new Promise(resolve=>release=resolve);
                probe.pending={release,signal:request.signal};
                request.signal.addEventListener('abort',()=>probe.aborted++,{once:true});
                // Ignore abort deliberately to exercise the late-response fence too.
                await gate;probe.delivered++;
            }
            const from='2026-01-01T00:00:00Z',to='2026-01-01T00:00:05Z';
            const samples=range==='24h'?[
                {collected_at:from,cpu:{usage_percent:25}},
                {collected_at:to,cpu:{usage_percent:75}}
            ]:[];
            return new Response(JSON.stringify({from,to,interval_seconds:5,samples}),{status:200,headers:{'Content-Type':'application/json'}});
        };return true;
    "#)?;
    with_cleanup(
        || {
            choose(browser, "30min")?;
            check(
                browser,
                "monitor read held",
                "return window.__zxsMonitorReads.pending!==null",
            )?;
            choose(browser, "24h")?;
            check(
                browser,
                "monitor replacement aborts old fetch and renders",
                &format!("return window.__zxsMonitorReads.aborted===1&&window.__zxsMonitorReads.pending.signal.aborted&&{CURRENT_CHART}"),
            )?;
            browser.script("window.__zxsMonitorReads.pending.release();return true")?;
            check(
                browser,
                "late monitor response delivered",
                "return window.__zxsMonitorReads.delivered===1",
            )?;
            let result = browser.cdp("Runtime.evaluate", json!({
                "expression":format!("(async()=>{{for(let i=0;i<8;i++)await new Promise(requestAnimationFrame);return {CURRENT_CHART};}})()"),
                "awaitPromise":true,"returnByValue":true,
            }))?;
            if !result["exceptionDetails"].is_null() || result["result"]["value"] != true {
                return Err(
                    format!("late monitor read replaced the active range: {result}").into(),
                );
            }
            Ok(())
        },
        || {
            browser.script(
                "window.__zxsMonitorReads.restore();delete window.__zxsMonitorReads;return true",
            )?;
            choose(browser, "1min")?;
            Ok(())
        },
    )?;
    println!("PASS monitor range replacement aborts fetch and rejects late data");
    Ok(())
}
