//! Fault injection at the real WASM readiness boundary, never reactive-state
//! replacement. Interval wrappers forward unchanged to Chrome's native timers.
use super::{Browser, Result, json, wait_for, with_cleanup};

fn observer(event: &str) -> String {
    let action = match event {
        "cached" => "window.dispatchEvent(new PageTransitionEvent('pagehide', {persisted:true}));",
        "unload" => "window.dispatchEvent(new PageTransitionEvent('pagehide', {persisted:false}));",
        "failed" => {
            "document.documentElement.setAttribute('data-zxs-startup','failed');\
             document.dispatchEvent(new Event('zxs-startup-failed'));"
        }
        "pending" => "",
        _ => unreachable!(),
    };
    format!(
        r#"
(() => {{
  const active = new Set(), urls = new Set();
  const nativeCreateURL = URL.createObjectURL, nativeRevokeURL = URL.revokeObjectURL;
  URL.createObjectURL = function(...args) {{
    const url = Reflect.apply(nativeCreateURL, this, args);
    urls.add(url);
    return url;
  }};
  URL.revokeObjectURL = function(...args) {{
    const result = Reflect.apply(nativeRevokeURL, this, args);
    urls.delete(args[0]);
    return result;
  }};
  const nativeSet = window.setInterval;
  const nativeClear = window.clearInterval;
  window.setInterval = function(...args) {{
    const id = Reflect.apply(nativeSet, this, args);
    active.add(id);
    return id;
  }};
  window.clearInterval = function(...args) {{
    const result = Reflect.apply(nativeClear, this, args);
    active.delete(args[0]);
    return result;
  }};
  window.__zxsSmokeLifecycle = {{fired:false}};
  window.__zxsSmokeLifecycle.intervals = () => active.size;
  document.addEventListener('zxs-startup-ready', () => {{
    const record = window.__zxsSmokeLifecycle;
    const root = document.getElementById('root');
    record.before = {{nodes:root.childNodes.length, intervals:active.size, urls:urls.size}};
    {action}
    record.synchronous = {{nodes:root.childNodes.length, intervals:active.size, urls:urls.size}};
    queueMicrotask(() => {{
      record.after = {{nodes:root.childNodes.length, intervals:active.size, urls:urls.size,
        phase:document.documentElement.getAttribute('data-zxs-startup')}};
      record.fired = true;
    }});
  }}, {{once:true}});
}})();
"#
    )
}

pub(super) fn run(browser: &Browser, origin: &str) -> Result<()> {
    for event in ["cached", "unload", "failed"] {
        let installed = browser.cdp(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":observer(event)}),
        )?;
        let identifier = installed["identifier"]
            .as_str()
            .ok_or("missing lifecycle observer identifier")?;
        with_cleanup(
            || {
                browser.command("POST", "/url", Some(json!({"url":origin})))?;
                let record = wait_for("real runtime readiness lifecycle", || {
                    let record = browser.script("return window.__zxsSmokeLifecycle")?;
                    if record["fired"] == true {
                        Ok(record)
                    } else {
                        Err("real WASM has not published readiness".into())
                    }
                })?;
                // Prove we interrupted an actual mounted application with live
                // owner resources, rather than passing on an empty startup page.
                if record["before"]["nodes"].as_u64().unwrap_or(0) == 0
                    || record["before"]["intervals"].as_u64().unwrap_or(0) < 2
                {
                    return Err(format!("missing mounted lifecycle fixture: {record}").into());
                }
                for phase in ["synchronous", "after"] {
                    let state = &record[phase];
                    let retained = state["nodes"].as_u64().unwrap_or(0) > 0
                        && state["intervals"] == 0
                        && state["urls"] == record["before"]["urls"];
                    let disposed =
                        state["nodes"] == 0 && state["intervals"] == 0 && state["urls"] == 0;
                    if !(if event == "cached" {
                        retained
                    } else {
                        disposed
                    }) {
                        return Err(format!("{event} {phase} lifecycle mismatch: {record}").into());
                    }
                }
                if event == "failed" && record["after"]["phase"] != "failed" {
                    return Err(format!("failed startup was revived: {record}").into());
                }
                if event == "cached" {
                    // Retention is useful only if real async session restoration
                    // can still finish and present the anonymous application.
                    browser.css("input[autocomplete='current-password']")?;
                    browser.script("window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true})); window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true})); return true")?;
                    if browser.script("return window.__zxsSmokeLifecycle.intervals()")?
                        != record["before"]["intervals"]
                    {
                        return Err("cached page did not resume exactly one timer pair".into());
                    }
                }
                let errors = browser.script("return window.__zxsSmokeErrors")?;
                if errors != json!([]) {
                    return Err(format!("uncaught {event} lifecycle errors: {errors}").into());
                }
                Ok(())
            },
            || {
                browser.cdp(
                    "Page.removeScriptToEvaluateOnNewDocument",
                    json!({"identifier":identifier}),
                )?;
                Ok(())
            },
        )?;
        println!("PASS synchronous startup-ready {event} lifecycle and owner timer cleanup");
    }
    required_browser_capabilities(browser, origin)?;
    visibility_reads(browser, origin)?;
    pending_session_response(browser, origin)?;
    // Do not leave the browser on the injected failed/disposed page. A normal
    // navigation and reload must still initialize the production application.
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    browser.css("input[autocomplete='current-password']")?;
    browser.command("POST", "/refresh", Some(json!({})))?;
    browser.css("input[autocomplete='current-password']")?;
    if browser.script("return window.__zxsSmokeErrors")? != json!([]) {
        return Err("uncaught error after normal lifecycle recovery navigation".into());
    }
    println!("PASS normal navigation and reload after lifecycle injection removal");
    Ok(())
}

// Fault injection removes one required capability before the actual WASM starts.
// This is startup rejection coverage, not an old-browser compatibility emulator.
fn required_browser_capabilities(browser: &Browser, origin: &str) -> Result<()> {
    for (capability, injection) in [
        ("ResizeObserver", "window.ResizeObserver = undefined;"),
        ("AbortController", "window.AbortController = undefined;"),
        ("AbortSignal.reason", "delete AbortSignal.prototype.reason;"),
        (
            "AbortSignal.reason",
            "const abort = AbortController.prototype.abort; AbortController.prototype.abort = function() { return Reflect.apply(abort, this, []); };",
        ),
        (
            "AbortSignal.reason",
            "Object.defineProperty(AbortSignal.prototype, 'aborted', {get() { return false; }, configurable:true});",
        ),
        (
            "FinalizationRegistry",
            "window.FinalizationRegistry = undefined;",
        ),
        (
            "instantiateStreaming",
            "WebAssembly.instantiateStreaming = undefined;",
        ),
        ("registerProperty", "CSS.registerProperty = undefined;"),
        (
            "encodeInto",
            "TextEncoder.prototype.encodeInto = undefined;",
        ),
        (
            "mask-image",
            "const supports = CSS.supports; CSS.supports = function(...args) { return args[0].includes('mask-image') ? false : Reflect.apply(supports, this, args); };",
        ),
    ] {
        let source = format!(
            "{}\n{}\n{}",
            observer("pending"),
            r#"
window.__zxsSmokeCapabilities = {ready:0, failures:0, errors:[]};
document.addEventListener('zxs-startup-ready', () => window.__zxsSmokeCapabilities.ready++);
document.addEventListener('zxs-startup-failed', () => window.__zxsSmokeCapabilities.failures++);
const nativeError = console.error;
console.error = function(...args) {
  window.__zxsSmokeCapabilities.errors.push(args.map(String).join(' '));
  return Reflect.apply(nativeError, this, args);
};
"#,
            injection,
        );
        let installed = browser.cdp(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":source}),
        )?;
        let identifier = installed["identifier"]
            .as_str()
            .ok_or("missing capability injection identifier")?;
        with_cleanup(
            || {
                browser.command("POST", "/url", Some(json!({"url":origin})))?;
                wait_for("required browser capability rejection", || {
                    if browser.script("return document.documentElement.dataset.zxsStartup === 'failed' && !!document.getElementById('zxs-startup-error')")? == true {
                        Ok(())
                    } else {
                        Err("unsupported browser has not shown startup failure".into())
                    }
                })?;
                let state = browser.script("return {...window.__zxsSmokeCapabilities,phase:document.documentElement.dataset.zxsStartup,runtime:document.documentElement.getAttribute('data-zxs-runtime'),readyAttribute:document.documentElement.getAttribute('data-zxs-ready'),login:!!document.querySelector('input[autocomplete=current-password]'),fallback:!!document.querySelector('[data-zxs-contour-state=fallback]'),intervals:window.__zxsSmokeLifecycle.intervals()}")?;
                if state["ready"] != 0
                    || state["failures"].as_u64().unwrap_or(0) == 0
                    || state["phase"] != "failed"
                    || !state["runtime"].is_null()
                    || !state["readyAttribute"].is_null()
                    || state["login"] != false
                    || state["fallback"] != false
                    || state["intervals"] != 0
                    || !state["errors"].as_array().is_some_and(|errors| {
                        errors.iter().any(|error| {
                            error
                                .as_str()
                                .is_some_and(|error| error.contains(capability))
                        })
                    })
                {
                    return Err(
                        format!("missing {capability} did not fail before mount: {state}").into(),
                    );
                }
                Ok(())
            },
            || {
                browser.cdp(
                    "Page.removeScriptToEvaluateOnNewDocument",
                    json!({"identifier":identifier}),
                )?;
                Ok(())
            },
        )?;
        // A fresh document must recover without retaining the failed admission.
        browser.command("POST", "/refresh", Some(json!({})))?;
        browser.css("input[autocomplete='current-password']")?;
        if browser.script("return document.documentElement.dataset.zxsStartup === 'ready' && !document.getElementById('zxs-startup-error') && window.__zxsSmokeErrors.length === 0")? != true {
            return Err(format!("normal reload failed after restoring {capability}").into());
        }
        println!("PASS missing {capability} rejects startup, then normal reload recovers");
    }
    Ok(())
}

fn pending_session_response(browser: &Browser, origin: &str) -> Result<()> {
    let source = format!(
        "{}\n{}",
        observer("pending"),
        r#"
(() => {
  const nativeFetch = window.fetch;
  let release;
  const gate = new Promise(resolve => release = resolve);
  const state = window.__zxsSmokePendingSession = {waiting:false, delivered:false, finished:false};
  state.release = release;
  state.restore = () => {window.fetch = nativeFetch;};
  window.fetch = async function(...args) {
    const response = await Reflect.apply(nativeFetch, this, args);
    const request = args[0];
    const url = typeof request === 'string' ? request : request.url;
    if (new URL(url, location.href).pathname !== '/api/auth/me') return response;
    state.status = response.status;
    state.waiting = true;
    window.fetch = nativeFetch;
    await gate;
    state.delivered = true;
    // Yield through the production fetch/JSON continuations and error event
    // delivery, as in the real-response metadata timing fixture.
    setTimeout(() => {state.finished = true;}, 500);
    return response;
  };
})();
"#
    );
    let installed = browser.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":source}),
    )?;
    let identifier = installed["identifier"]
        .as_str()
        .ok_or("missing pending-session observer identifier")?;
    with_cleanup(
        || {
            browser.command("POST", "/url", Some(json!({"url":origin})))?;
            wait_for(
                "actual session response waiting at mounted application",
                || {
                    if browser.script("return window.__zxsSmokePendingSession?.waiting && window.__zxsSmokeLifecycle?.fired")? == true {
                    Ok(())
                } else {
                    Err("real session response or mounted application is still pending".into())
                }
                },
            )?;
            let before = browser.script("return {nodes:document.getElementById('root').childNodes.length,intervals:window.__zxsSmokeLifecycle.intervals(),status:window.__zxsSmokePendingSession.status}")?;
            if before["nodes"].as_u64().unwrap_or(0) == 0
                || before["intervals"].as_u64().unwrap_or(0) < 2
                || before["status"] != 401
            {
                return Err(format!("invalid real pending-session fixture: {before}").into());
            }
            browser.script("window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:false})); window.__zxsSmokePendingSession.release(); return true")?;
            wait_for(
                "original session response delivery after owner disposal",
                || {
                    if browser.script("return window.__zxsSmokePendingSession.finished")? == true {
                        Ok(())
                    } else {
                        Err("session response has not finished delivery".into())
                    }
                },
            )?;
            let after = browser.script("return {nodes:document.getElementById('root').childNodes.length,intervals:window.__zxsSmokeLifecycle.intervals(),errors:window.__zxsSmokeErrors,delivered:window.__zxsSmokePendingSession.delivered}")?;
            if after["nodes"] != 0
                || after["intervals"] != 0
                || after["errors"] != json!([])
                || after["delivered"] != true
            {
                return Err(
                    format!("late session response escaped disposed owner: {after}").into(),
                );
            }
            Ok(())
        },
        || {
            // Release before removing the new-document hook, including failure paths.
            let release = browser.script("if(window.__zxsSmokePendingSession){window.__zxsSmokePendingSession.release();window.__zxsSmokePendingSession.restore();} return true");
            let remove = browser.cdp(
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({"identifier":identifier}),
            );
            release?;
            remove?;
            Ok(())
        },
    )?;
    println!("PASS original pending session response cannot access a disposed owner");
    Ok(())
}

// Deliberate visibility-event injection against the real WASM/timer lifecycle.
// This tests scheduling deterministically; it is not an OS tab-suspension test.
fn visibility_reads(browser: &Browser, origin: &str) -> Result<()> {
    let installed = browser.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":observer("pending")}),
    )?;
    let identifier = installed["identifier"]
        .as_str()
        .ok_or("missing visibility observer")?;
    with_cleanup(
        || {
            browser.command("POST", "/url", Some(json!({"url":origin})))?;
            browser.css("input[autocomplete='current-password']")?;
            let baseline = browser.script("return window.__zxsSmokeLifecycle.intervals()")?;
            if baseline.as_u64().unwrap_or(0) < 2 {
                return Err("visible document has no polling timer pair".into());
            }
            browser.script("Object.defineProperty(document,'hidden',{configurable:true,value:true}); document.dispatchEvent(new Event('visibilitychange')); document.dispatchEvent(new Event('visibilitychange')); return true")?;
            if browser.script("return window.__zxsSmokeLifecycle.intervals()")? != 0 {
                return Err("hidden document retained background intervals".into());
            }
            browser.script("Object.defineProperty(document,'hidden',{configurable:true,value:false}); document.dispatchEvent(new Event('visibilitychange')); document.dispatchEvent(new Event('visibilitychange')); return true")?;
            if browser.script("return window.__zxsSmokeLifecycle.intervals()")? != baseline {
                return Err("visibility resume duplicated or lost timer ownership".into());
            }
            browser.script("window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:false})); document.dispatchEvent(new Event('visibilitychange')); window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true})); return true")?;
            if browser.script("return window.__zxsSmokeLifecycle.intervals()===0 && document.getElementById('root').childNodes.length===0 && window.__zxsSmokeErrors.length===0")? != true {
                return Err("disposed document resumed its background reads".into());
            }
            Ok(())
        },
        || {
            let restore = browser.script("delete document.hidden; return true");
            let remove = browser.cdp(
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({"identifier":identifier}),
            );
            restore?;
            remove?;
            Ok(())
        },
    )?;
    println!(
        "PASS injected visibility pause/repeated resume and disposed-owner timer cleanup in real WASM"
    );
    Ok(())
}
