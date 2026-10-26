//! Production-WASM RAF lifecycle checks. Only host delivery is controlled;
//! no application hooks, state writes, font replacements or layout overrides.
use super::*;

pub(super) fn install(browser: &Browser) -> Result<()> {
    browser.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":include_str!("tree_frames.js")}),
    )?;
    Ok(())
}

fn evaluate(browser: &Browser, expression: &str) -> Result<Value> {
    let result = browser.cdp(
        "Runtime.evaluate",
        json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
    )?;
    if !result["exceptionDetails"].is_null() {
        return Err(format!("tree frame probe failed: {result}").into());
    }
    Ok(result["result"]["value"].clone())
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    with_cleanup(
        || exercise(browser),
        || {
            browser.script("window.__zxsTreeFrames.restore();return true")?;
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            Ok(())
        },
    )
}

fn exercise(browser: &Browser) -> Result<()> {
    browser.cdp(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":390,"height":844,"deviceScaleFactor":1,"mobile":false}),
    )?;
    evaluate(browser, "window.__zxsTreeFrames.settle()")?;
    evaluate(
        browser,
        r#"(async()=>{
        const p=window.__zxsTreeFrames;
        p.rejectScheduling();
        p.hold=true;
        const {root}=p.burst();
        await p.flush(root);
        return true;
    })()"#,
    )?;
    browser.script(
        r#"
        const p=window.__zxsTreeFrames,root=p.current();
        p.nativeNode=root.querySelector('.zxs-version-node');
        const {width,height}=p.nativeNode.getBoundingClientRect();
        p.beforeNode={width,height};
        p.nativeBefore=p.observers.filter(o=>o.root===root).map(o=>({o,count:o.nativeDeliveries,
            nodeCount:o.nativeTargets.get(p.nativeNode)?.count??0}));
        p.check(p.nativeBefore.length===2,'exact mounted RO/MO pair before native stimulus');
        p.hold=true;
        return true;
    "#,
    )?;
    // Viewport resize alone need not resize max-content canvas or capped nodes.
    // Real note editing inserts the input and a second byte-count row, so both
    // child-list mutation and observed node height must actually change.
    browser.click(&browser.css(".zxs-version-node .zxs-version-note-action")?)?;
    let editor = browser.css(".zxs-version-node .zxs-inline-input")?;
    wait_for("real tree resize and mutation notifications", || {
        if browser.script("return window.__zxsTreeFrames.nativeReady()")? == true {
            Ok(())
        } else {
            let snapshot = browser.script("return window.__zxsTreeFrames.nativeSnapshot()")?;
            Err(
                format!("native RO/MO delivery or edited-node height change missing: {snapshot}")
                    .into(),
            )
        }
    })?;
    println!(
        "PASS native tree note-edit stimulus: {}",
        browser.script("return window.__zxsTreeFrames.nativeSnapshot()")?
    );
    // Repeated delivery is explicit test stimulus, not a claim that Chrome
    // naturally emits eight observer batches in every frame.
    evaluate(
        browser,
        r#"(async()=>{
        const p=window.__zxsTreeFrames,{root}=p.burst();
        await p.flush(root);
        return true;
    })()"#,
    )?;
    let editing = evaluate(browser, include_str!("tree_geometry.js"))?;
    // Cancel the real editor without submitting a synthetic branch/version ID.
    browser.keys(&editor, "\u{e00c}")?;
    wait_for("version note editor canceled", || {
        if browser
            .script("return !document.querySelector('.zxs-version-node .zxs-inline-input')")?
            == true
        {
            Ok(())
        } else {
            Err("version note editor still present".into())
        }
    })?;
    let narrow = evaluate(browser, include_str!("tree_geometry.js"))?;
    evaluate(
        browser,
        r#"(()=>{
        const p=window.__zxsTreeFrames;
        p.check(Math.abs(p.nativeNode.getBoundingClientRect().height-p.beforeNode.height)<.1,
            'cancel restores original node height');
        return true;
    })()"#,
    )?;
    browser.script(
        r#"
        const p=window.__zxsTreeFrames,root=p.current();
        const scroll=root.parentElement;
        scroll.scrollLeft=Math.min(100,scroll.scrollWidth-scroll.clientWidth);
        p.scroll={root,left:scroll.scrollLeft};
        p.hold=true;p.burst();return true;
    "#,
    )?;
    evaluate(
        browser,
        r#"(async()=>{
        const p=window.__zxsTreeFrames;await p.flush(p.scroll.root);
        p.check(Math.abs(p.scroll.root.parentElement.scrollLeft-p.scroll.left)<=1,'refresh preserves user scroll');
        p.hold=true;p.burst();p.old=p.current();
        p.oldFrames=p.pending(p.old);p.oldMeasures=p.roots.get(p.old).measures;
        return true;
    })()"#,
    )?;
    browser.click(&browser.button("Files", "文件")?)?;
    wait_for("tree owner unmounted", || {
        if browser.script("return !window.__zxsTreeFrames.old.isConnected")? == true {
            Ok(())
        } else {
            Err("tree still mounted".into())
        }
    })?;
    evaluate(
        browser,
        r#"(async()=>{
        const p=window.__zxsTreeFrames;
        p.check(p.oldFrames.length===1&&p.oldFrames[0].canceled,'cleanup cancels pending RAF');
        p.check(p.observers.filter(o=>o.root===p.old).every(o=>o.disconnected),'cleanup disconnects both observers');
        // Simulate a host delivering an already-canceled RAF, not a dropped
        // observer closure. The original WASM RAF closure must safely no-op.
        for(const frame of p.oldFrames)frame.callback(performance.now());
        p.check(p.roots.get(p.old).measures===p.oldMeasures,'late RAF cannot measure detached tree');
        p.hold=false;await p.settle();
        p.check(window.__zxsSmokeErrors.length===0,'late RAF cannot access disposed signals');
        return true;
    })()"#,
    )?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    wait_for("remounted wide history fixture", || {
        if browser.script("return document.querySelectorAll('.zxs-version-node').length>=14")?
            == true
        {
            Ok(())
        } else {
            Err("history not remounted".into())
        }
    })?;
    let remount = evaluate(browser, include_str!("tree_geometry.js"))?;
    evaluate(
        browser,
        r#"(()=>{
        const p=window.__zxsTreeFrames,root=p.current(),scroll=root.parentElement;
        p.check(root!==p.old,'new mounted tree identity');
        const tip=root.querySelector('[data-version-depth="0"] .zxs-version-node').getBoundingClientRect();
        const box=scroll.getBoundingClientRect();
        const expected=Math.round(Math.max(0,Math.min(scroll.scrollWidth-scroll.clientWidth,
            tip.left-box.left+scroll.scrollLeft+tip.width/2-scroll.clientWidth/2)));
        p.check(Math.abs(scroll.scrollLeft-expected)<=1,'remount initially centers newest tip');
        return true;
    })()"#,
    )?;
    browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
    let desktop = evaluate(browser, include_str!("tree_geometry.js"))?;
    println!(
        "PASS tree native RO/MO + repeated delivery coalescing, canceled/late RAF, remount/font-ready/resize geometry (wide history synthetic): editing={editing}, narrow={narrow}, remount={remount}, desktop={desktop}"
    );
    Ok(())
}
