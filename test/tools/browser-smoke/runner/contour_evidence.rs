//! Opt-in paired-build evidence. This is not the normal browser acceptance suite.
//! The caller runs this same binary against each immutable Web distribution.
use super::*;

const PROBE: &str = include_str!("contour_evidence.js");

fn evaluate(browser: &Browser, expression: &str) -> Result<Value> {
    let result = browser.cdp(
        "Runtime.evaluate",
        json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
    )?;
    if !result["exceptionDetails"].is_null() {
        return Err(format!("contour evidence probe failed: {result}").into());
    }
    Ok(result["result"]["value"].clone())
}

fn save_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, json_codec::to_vec_pretty(value)?)?;
    Ok(())
}

fn save_png(path: &Path, encoded: &str) -> Result<()> {
    let bytes = STANDARD.decode(encoded)?;
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("contour evidence image is not a PNG".into());
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn metric_deltas(before: &Value, after: &Value) -> Result<Value> {
    let mut deltas = json!({});
    for name in [
        "LayoutCount",
        "RecalcStyleCount",
        "LayoutDuration",
        "RecalcStyleDuration",
        "ScriptDuration",
        "TaskDuration",
    ] {
        let read = |snapshot: &Value| -> Result<f64> {
            snapshot["metrics"]
                .as_array()
                .and_then(|values| values.iter().find(|entry| entry["name"] == name))
                .and_then(|entry| entry["value"].as_f64())
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("missing finite Chrome metric {name}").into())
        };
        let difference = read(after)? - read(before)?;
        if difference < 0.0 {
            return Err(format!("Chrome metric {name} moved backwards").into());
        }
        deltas[name] = json!(difference);
    }
    Ok(deltas)
}

fn snapshot(browser: &Browser, directory: &Path, label: &str) -> Result<Value> {
    let mut value = evaluate(browser, "window.__zxsContourEvidence.snapshot()")?;
    for image in value["images"]
        .as_array_mut()
        .ok_or("missing raster images")?
    {
        let filename = format!(
            "{label}-{}.png",
            image["name"].as_str().ok_or("missing raster name")?
        );
        let encoded = image["png_base64"].take();
        save_png(
            &directory.join(&filename),
            encoded.as_str().ok_or("missing raster PNG")?,
        )?;
        image["file"] = filename.into();
    }
    let screenshot = browser.cdp(
        "Page.captureScreenshot",
        json!({"format":"png","fromSurface":true,"captureBeyondViewport":false,
            "clip":{"x":0,"y":0,"width":1200,"height":760,"scale":1}}),
    )?;
    let encoded = screenshot["data"]
        .as_str()
        .ok_or("missing fixture screenshot")?;
    let filename = format!("{label}-fixture.png");
    save_png(&directory.join(&filename), encoded)?;
    value["screenshot"] = evaluate(
        browser,
        &format!(
            "window.__zxsContourEvidence.pngSignature({})",
            json!(encoded)
        ),
    )?;
    value["screenshot"]["file"] = filename.into();
    if value["screenshot"]["width"] != 1200 || value["screenshot"]["height"] != 760 {
        return Err("fixture screenshot dimensions differ from its viewport clip".into());
    }
    value["label"] = label.into();
    Ok(value)
}

fn hover(browser: &Browser, enter: bool) -> Result<()> {
    let point = if enter {
        browser.script("const r=document.getElementById('g4-evidence-shape-33').getBoundingClientRect();return {x:Math.floor(r.x+r.width/2),y:Math.floor(r.y+r.height/2)}")?
    } else {
        json!({"x":1190,"y":750})
    };
    browser.command(
        "POST",
        "/actions",
        Some(
            json!({"actions":[{"type":"pointer","id":"g4-evidence-mouse",
            "parameters":{"pointerType":"mouse"},"actions":[{"type":"pointerMove",
            "duration":0,"origin":"viewport","x":point["x"],"y":point["y"]}]}]}),
        ),
    )?;
    evaluate(
        browser,
        &format!("window.__zxsContourEvidence.checkHover({enter})"),
    )?;
    Ok(())
}

pub(super) fn run(browser: &Browser, origin: &str, web: &Path, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    if fs::read_dir(output)?.next().transpose()?.is_some() {
        return Err("contour evidence directory must be empty to avoid mixing observations".into());
    }
    diagnostics::install(browser)?;
    browser.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":PROBE}),
    )?;
    browser.cdp(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":1200,"height":800,"deviceScaleFactor":1,"mobile":false}),
    )?;
    browser.cdp(
        "Emulation.setEmulatedMedia",
        json!({"features":[{"name":"prefers-color-scheme","value":"light"}]}),
    )?;
    browser.command(
        "POST",
        "/url",
        Some(json!({"url":format!("{origin}/?lang=en")})),
    )?;
    browser.css("input[autocomplete='current-password']")?;
    let setup = evaluate(browser, "window.__zxsContourEvidence.setup()")?;
    let manifest: Value = json_codec::from_slice(&fs::read(web.join("build-manifest.json"))?)?;
    save_json(
        &output.join("identity.json"),
        &json!({"schema":1,"browser":browser.cdp("Browser.getVersion",json!({}))?,
            "web_build_manifest":manifest,"fixture":setup,
            "probe_source":PROBE,
            "scope":"same harness/server, selected production Web assets; synthetic fixed DOM fixture",
            "timing_scope":"instrumented contour-tagged RAF callback elapsed time, native RAF intervals, whole-page CDP metric deltas; not compositor/presentation time",
            "source_identity":"caller must archive both source SHAs and harness/server hashes alongside these results"}),
    )?;
    let mut visuals = vec![snapshot(browser, output, "initial")?];
    hover(browser, true)?;
    visuals.push(snapshot(browser, output, "hover")?);
    hover(browser, false)?;
    evaluate(browser, "window.__zxsContourEvidence.hideShow()")?;
    visuals.push(snapshot(browser, output, "hide-show")?);
    evaluate(browser, "window.__zxsContourEvidence.scrollSnapshot()")?;
    visuals.push(snapshot(browser, output, "scrolled")?);
    save_json(&output.join("visuals.json"), &json!(visuals))?;

    browser.cdp("Performance.enable", json!({"timeDomain":"timeTicks"}))?;
    let mut timings = Vec::new();
    for workload in ["extent-animation", "author-invalidation", "scroll"] {
        evaluate(
            browser,
            &format!("window.__zxsContourEvidence.prepare({})", json!(workload)),
        )?;
        let before = browser.cdp("Performance.getMetrics", json!({}))?;
        evaluate(
            browser,
            &format!("window.__zxsContourEvidence.measure({})", json!(workload)),
        )?;
        let after = browser.cdp("Performance.getMetrics", json!({}))?;
        let samples = evaluate(
            browser,
            &format!("window.__zxsContourEvidence.result({})", json!(workload)),
        )?;
        let deltas = metric_deltas(&before, &after)?;
        timings.push(json!({"workload":workload,"samples":samples,
            "cdp_before":before,"cdp_after":after,"cdp_deltas":deltas,
            "cdp_units":"Count metrics are counts; Duration metrics are seconds"}));
        // Persist each completed workload even if a later observation fails.
        save_json(&output.join("timings.json"), &json!(&timings))?;
    }
    evaluate(browser, "window.__zxsContourEvidence.finish()")?;
    let errors = browser.script("return window.__zxsSmokeErrors")?;
    if errors != json!([]) {
        return Err(format!("contour evidence browser errors: {errors}").into());
    }
    let console = browser.command("POST", "/log", Some(json!({"type":"browser"})))?;
    save_json(&output.join("console.json"), &console)?;
    for entry in console.as_array().ok_or("invalid browser console log")? {
        // The anonymous page normally receives an HTTP 401 from auth/me.
        // In particular, caught controller failures are console-api errors,
        // not uncaught JavaScript events observed by __zxsSmokeErrors.
        let contour_warning = entry["level"] == "WARNING"
            && entry["source"] == "console-api"
            && entry["message"]
                .as_str()
                .is_some_and(|message| message.to_ascii_lowercase().contains("contour"));
        if (entry["level"] == "SEVERE" && entry["source"] != "network") || contour_warning {
            return Err(format!("contour evidence console error: {}", entry["message"]).into());
        }
    }
    println!(
        "PASS contour evidence captured in {}; paired comparison is required",
        output.display()
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/contour_evidence_tests.rs"]
mod tests;
