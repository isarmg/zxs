//! Real pointer/keyboard interaction plus screenshot pixels at both horizontal
//! scroll endpoints. Synthetic listing data is still rendered by product WASM.
use super::*;

fn move_pointer(browser: &Browser, point: Value) -> Result<()> {
    browser.command(
        "POST",
        "/actions",
        Some(json!({"actions":[{"type":"pointer","id":"file-row-mouse",
            "parameters":{"pointerType":"mouse"},"actions":[{"type":"pointerMove",
                "duration":0,"origin":"viewport","x":point["x"],"y":point["y"]}]}]})),
    )?;
    Ok(())
}

fn position(browser: &Browser, index: usize, end: bool) -> Result<()> {
    browser.script(&format!(
        "document.activeElement.blur();const row=document.querySelectorAll('.zxs-file-table tbody tr')[{index}];row.scrollIntoView({{block:'center',inline:'nearest',behavior:'instant'}});const s=row.closest('.zxs-table-scroll');s.scrollLeft={end}?s.scrollWidth-s.clientWidth:0;return true"
    ))?;
    // Resize/scroll can invalidate the panel's G4 mask. Do not send a one-shot
    // pointer event against its previous frame's compositor hit-test geometry.
    browser.command(
        "POST",
        "/execute/async",
        Some(json!({"script":"const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))","args":[]})),
    )?;
    Ok(())
}

fn pointer_state(browser: &Browser, index: usize, point: Value) -> Result<Value> {
    browser.command(
        "POST",
        "/execute/sync",
        Some(json!({"script":include_str!("file_row_pointer.js"),"args":[index,point]})),
    )
}

fn pointer_evidence(browser: &Browser, state: &Value, end: bool, failed: bool) -> Result<()> {
    println!("INFO real file-row pointer (end={end}, failed={failed}): {state}");
    let output = env::var_os("ZXS_VIEWPORT_PREVIEWS").map(PathBuf::from);
    if let Some(directory) =
        output.or_else(|| failed.then(|| PathBuf::from("artifacts/browser-smoke/diagnostics")))
    {
        fs::create_dir_all(&directory)?;
        let label = format!(
            "row-pointer-{}-{}-{}-{end}",
            state["kind"].as_str().ok_or("missing pointer row kind")?,
            state["index"],
            state["theme"].as_str().ok_or("missing pointer theme")?
        );
        fs::write(
            directory.join(format!("{label}.json")),
            json_codec::to_vec_pretty(state)?,
        )?;
        if failed {
            // Capture before cleanup changes the viewport, theme, or pointer.
            let screenshot = browser.cdp(
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":false}),
            )?;
            fs::write(
                directory.join(format!("{label}.png")),
                STANDARD.decode(
                    screenshot["data"]
                        .as_str()
                        .ok_or("missing pointer failure screenshot")?,
                )?,
            )?;
        }
    }
    Ok(())
}

fn hover_row(browser: &Browser, index: usize, end: bool) -> Result<()> {
    position(browser, index, end)?;
    let mut state = pointer_state(browser, index, Value::Null)?;
    let result = wait_for("actual hovered row surface", || {
        // Observe the live center each attempt. A layout change must not leave
        // a stale coordinate, and an overlay must fail rather than be bypassed.
        state = pointer_state(browser, index, Value::Null)?;
        if state["visible"] != true || state["hitOnRow"] != true || state["hoverEnabled"] != true {
            return Err(format!("row pointer target or mouse capability: {state}").into());
        }
        let point = state["point"].clone();
        move_pointer(browser, point.clone())?;
        state = pointer_state(browser, index, point)?;
        if state["valid"] == true {
            Ok(())
        } else {
            Err(format!("native pointer did not reveal row surface: {state}").into())
        }
    });
    let evidence = pointer_evidence(browser, &state, end, result.is_err());
    if let Err(error) = result {
        if let Err(save_error) = evidence {
            eprintln!("row pointer evidence also failed: {save_error}");
        }
        return Err(error);
    }
    evidence
}

fn pixels(browser: &Browser, scene: Value) -> Result<()> {
    let screenshot = browser.cdp(
        "Page.captureScreenshot",
        json!({"format":"png","captureBeyondViewport":false,"clip":scene["clip"]}),
    )?;
    // Keep the actual failing screenshot in the existing CI preview artifact.
    let output = env::var_os("ZXS_VIEWPORT_PREVIEWS").map(PathBuf::from);
    if let Some(directory) = &output {
        fs::create_dir_all(directory)?;
        let label = scene["label"].as_str().ok_or("missing screenshot label")?;
        let png = screenshot["data"]
            .as_str()
            .ok_or("missing Chrome screenshot")?;
        fs::write(
            directory.join(format!("{label}.png")),
            STANDARD.decode(png)?,
        )?;
    }
    let result = browser.command(
        "POST",
        "/execute/async",
        Some(json!({
            "script":include_str!("file_row_pixels.js"),
            "args":[screenshot["data"],scene]
        })),
    )?;
    if let Some(directory) = output {
        let label = scene["label"].as_str().ok_or("missing screenshot label")?;
        fs::write(
            directory.join(format!("{label}.json")),
            json_codec::to_vec_pretty(&result)?,
        )?;
    }
    if result["valid"] != true {
        return Err(format!("file-row screenshot pixels: {result}").into());
    }
    Ok(())
}

fn row_pixels(browser: &Browser, index: usize) -> Result<()> {
    page_layout::file_row_geometry(browser)?;
    let scene = browser.script(&format!(
        "const row=document.querySelectorAll('.zxs-file-table tbody tr')[{index}],s=row.closest('.zxs-table-scroll'),b=s.getBoundingClientRect(),r=row.getBoundingClientRect(),css=getComputedStyle(row),kind=row.querySelector('.zxs-file-icon--folder')?'folder':'file';return {{label:'row-'+kind+'-{index}-'+document.documentElement.dataset.zxsTheme+'-'+s.scrollLeft+'-'+row.getAttribute('aria-selected'),kind:'row',panel:css.getPropertyValue('--zxs-bg-panel'),fill:css.getPropertyValue('--zxs-bg-page'),clip:{{x:b.left+scrollX,y:r.top+scrollY,width:s.clientWidth,height:r.height,scale:1}}}}"
    ))?;
    pixels(browser, scene)
}

fn focus_pixels(browser: &Browser, selector: &str) -> Result<()> {
    page_layout::file_row_geometry(browser)?;
    let scene = browser.script(&format!(
        "const c=document.activeElement,s=c.closest('.zxs-table-scroll'),r=c.getBoundingClientRect(),b=s.getBoundingClientRect(),css=getComputedStyle(c),ring=parseFloat(css.outlineWidth),margin=ring+parseFloat(css.outlineOffset),hit=document.elementFromPoint(r.left+r.width/2,r.top+r.height/2);return {{label:'focus-'+(c.closest('tr').querySelector('.zxs-file-icon--folder')?'folder':'file')+'-'+(c.tagName==='INPUT'?'checkbox':'action')+'-'+document.documentElement.dataset.zxsTheme,valid:c.matches({})&&c.matches(':focus-visible')&&ring>0&&css.outlineStyle!=='none'&&r.left-margin>=b.left-.5&&r.right+margin<=b.left+s.clientWidth+.5&&(hit===c||c.contains(hit)),kind:'focus',color:css.outlineColor,ring,clip:{{x:r.left-margin+scrollX,y:r.top-margin+scrollY,width:r.width+margin*2,height:r.height+margin*2,scale:1}}}}",
        json!(selector)
    ))?;
    if scene["valid"] != true {
        return Err(format!("file-row keyboard focus or hit target: {scene}").into());
    }
    pixels(browser, scene)
}

fn check_row(browser: &Browser, index: usize, end: bool, selected: bool) -> Result<()> {
    hover_row(browser, index, end)?;
    row_pixels(browser, index)?;
    move_pointer(browser, json!({"x":0,"y":0}))?;
    page_layout::file_row_geometry(browser)?;
    if selected {
        let checkbox = browser.css(&format!(
            ".zxs-file-table tbody tr:nth-child({}) .zxs-entry-checkbox",
            index + 1
        ))?;
        browser.click(&checkbox)?;
        position(browser, index, end)?;
        move_pointer(browser, json!({"x":0,"y":0}))?;
        row_pixels(browser, index)?;
        browser.click(&checkbox)?;
    }
    Ok(())
}

pub(super) fn run(browser: &Browser, indices: &[usize], kind: &str) -> Result<()> {
    let original_theme = browser.script("return document.documentElement.dataset.zxsTheme")?;
    with_cleanup(
        || {
            browser.cdp(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":320,"height":568,"deviceScaleFactor":1,"mobile":false}),
            )?;
            for theme in ["light", "dark"] {
                accessibility::select_theme(browser, theme)?;
                for &index in indices {
                    if browser.script(&format!("const row=document.querySelectorAll('.zxs-file-table tbody tr')[{index}],s=row.closest('.zxs-table-scroll');return !!row.querySelector('.zxs-file-icon--{kind}')&&s.scrollWidth>s.clientWidth"))? != true {
                        return Err(
                            format!("{kind} row {index} did not exercise horizontal overflow").into(),
                        );
                    }
                    for end in [false, true] {
                        check_row(browser, index, end, index == indices[indices.len() / 2])?;
                    }
                }
                let index = indices[indices.len() / 2];
                position(browser, index, true)?;
                let name = browser.css(&format!(
                    ".zxs-file-table tbody tr:nth-child({}) .zxs-file-name",
                    index + 1
                ))?;
                browser.keys(&name, "\u{e008}\u{e004}\u{e000}")?;
                focus_pixels(browser, ".zxs-entry-checkbox")?;
                position(browser, index, true)?;
                browser.keys(&name, "\u{e004}")?;
                focus_pixels(browser, ".zxs-entry-actions .zxs-button")?;
            }
            Ok(())
        },
        || {
            browser.script("document.activeElement.blur();for(const c of document.querySelectorAll('.zxs-entry-checkbox:checked'))c.click();const s=document.querySelector('.zxs-file-list-panel .zxs-table-scroll');s.scrollLeft=0;return true")?;
            move_pointer(browser, json!({"x":0,"y":0}))?;
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            accessibility::select_theme(browser, original_theme.as_str().unwrap_or("light"))
        },
    )?;
    println!(
        "PASS real {kind} hover/selection screenshot corners and keyboard focus after horizontal scrolling"
    );
    Ok(())
}
