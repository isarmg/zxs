//! Inspect real CSS events/CSSOM and fault only stylesheet transport.
use super::{Browser, Result, Value, json, wait_for, with_cleanup};

const OBSERVER: &str = r#"
window.__zxsStylesheets = {events:[],ready:0,failures:[]};
const observeStylesheet = event => {
  const link = event.target;
  if (link instanceof HTMLLinkElement && link.rel === 'stylesheet')
    window.__zxsStylesheets.events.push({path:new URL(link.href).pathname,
      type:event.type,elapsed:performance.now(),readyState:document.readyState,body:!!document.body});
};
document.addEventListener('load', observeStylesheet, true);
document.addEventListener('error', observeStylesheet, true);
document.addEventListener('zxs-startup-ready', () => window.__zxsStylesheets.ready++);
document.addEventListener('zxs-startup-failed', () =>
  window.__zxsStylesheets.failures.push({elapsed:performance.now(),
    readyState:document.readyState,body:!!document.body}));
"#;

pub(super) fn run(browser: &Browser, origin: &str) -> Result<()> {
    let installed = browser.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":OBSERVER}),
    )?;
    let identifier = installed["identifier"]
        .as_str()
        .ok_or("missing stylesheet observer identifier")?;
    with_cleanup(
        || {
            for disabled in [true, false] {
                browser.cdp(
                    "Network.setCacheDisabled",
                    json!({"cacheDisabled":disabled}),
                )?;
                // Prime a complete successful page before the cache-enabled case.
                // Cache hits are not themselves evidence that a stylesheet loaded.
                browser.command("POST", "/url", Some(json!({"url":origin})))?;
                browser.css("input[autocomplete='current-password']")?;
                normal_bundle(browser)?;
                blocked_stylesheets(browser, origin, &["/zxs.css"])?;
                println!("PASS main CSS error and real retry with cacheDisabled={disabled}");
            }
            browser.cdp("Network.setCacheDisabled", json!({"cacheDisabled":true}))?;
            blocked_stylesheets(browser, origin, &["/styles/startup.css"])?;
            blocked_stylesheets(browser, origin, &["/styles/startup.css", "/zxs.css"])?;
            Ok(())
        },
        || {
            let unblock = browser.cdp("Network.setBlockedURLs", json!({"urls":[]}));
            let cache = browser.cdp("Network.setCacheDisabled", json!({"cacheDisabled":false}));
            let observer = browser.cdp(
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({"identifier":identifier}),
            );
            unblock?;
            cache?;
            observer?;
            Ok(())
        },
    )?;
    println!(
        "PASS release CSS events/CSSOM, original fonts, required CSS rejection and manual reload"
    );
    Ok(())
}

fn blocked_stylesheets(browser: &Browser, origin: &str, paths: &[&str]) -> Result<()> {
    let urls: Vec<_> = paths.iter().map(|path| format!("{origin}{path}")).collect();
    browser.cdp("Network.setBlockedURLs", json!({"urls":urls}))?;
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    wait_for("required stylesheet transport rejection", || {
        let state = failure_state(browser)?;
        assert_failure(&state)?;
        if state["stage"] != "runtime" {
            return Err(format!(
                "stylesheet transport error was not rejected immediately: {state}"
            )
            .into());
        }
        for path in ["/styles/startup.css", "/zxs.css"] {
            let expected = if paths.contains(&path) {
                "error"
            } else {
                "load"
            };
            let opposite = if expected == "load" { "error" } else { "load" };
            if event_count(&state, path, expected) != 1 || event_count(&state, path, opposite) != 0
            {
                return Err(format!("missing real {expected} event for {path}: {state}").into());
            }
        }
        Ok(())
    })?;
    browser.cdp("Network.setBlockedURLs", json!({"urls":[]}))?;
    // Exercise the real retry button after transport recovery, not a synthetic
    // readiness event or an injected replacement stylesheet.
    browser.click(&browser.css("#zxs-startup-error button")?)?;
    browser.css("input[autocomplete='current-password']")?;
    normal_bundle(browser)?;
    println!("PASS required CSS {paths:?} rejection, hidden root and real retry");
    Ok(())
}

fn failure_state(browser: &Browser) -> Result<Value> {
    browser.script(
        r#"
const error = document.getElementById('zxs-startup-error');
const main = document.querySelector('link[href="./zxs.css"]');
const startup = document.querySelector('link[href="./styles/startup.css"]');
const root = document.getElementById('root');
const state = {error:!!error,phase:document.documentElement.dataset.zxsStartup,
  runtime:document.documentElement.getAttribute('data-zxs-runtime'),
  ready:document.documentElement.getAttribute('data-zxs-ready'),
  fonts:document.documentElement.dataset.zxsFonts,mainLoaded:!!main?.sheet,
  startupLoaded:!!startup?.sheet,mainRules:main?.sheet ? main.sheet.cssRules.length : null,
  elapsed:performance.now(),observation:window.__zxsStylesheets,
  login:!!document.querySelector('input[autocomplete="current-password"]'),
  rootHidden:!!root && root.hidden && getComputedStyle(root).display==='none'};
if (!error) return state;
const style = getComputedStyle(error), box = error.getBoundingClientRect();
return {...state,stage:error.dataset.stage,box:style.boxSizing,font:style.fontFamily,
  retry:!!error.querySelector('button:not(:disabled)'),text:error.innerText,
  visible:style.display!=='none' && style.visibility==='visible' && box.width>0 && box.height>0};
"#,
    )
}

fn event_count(state: &Value, path: &str, kind: &str) -> usize {
    state["observation"]["events"]
        .as_array()
        .expect("invariant: observer records a CSS event array")
        .iter()
        .filter(|event| event["path"] == path && event["type"] == kind)
        .count()
}

fn assert_failure(state: &Value) -> Result<()> {
    if state["phase"] != "failed"
        || state["error"] != true
        || state["observation"]["ready"] != 0
        || !state["observation"]["failures"]
            .as_array()
            .is_some_and(|failures| failures.len() == 1)
        || !state["runtime"].is_null()
        || !state["ready"].is_null()
        || state["login"] != false
        || state["rootHidden"] != true
        || state["retry"] != true
        || state["visible"] != true
        || !state["text"].as_str().is_some_and(|text| !text.is_empty())
    {
        return Err(format!("required-CSS failure state: {state}").into());
    }
    Ok(())
}

fn normal_bundle(browser: &Browser) -> Result<()> {
    wait_for("original regular and bold fonts after CSS bundling", || {
        if browser.script("return document.documentElement.dataset.zxsFonts==='ready' && document.fonts.check('16px ZxsUI') && document.fonts.check('bold 16px ZxsUI')")? == true {
            Ok(())
        } else { Err("original CSS font faces have not loaded".into()) }
    })?;
    let state = browser.script(r#"
const links = Array.from(document.querySelectorAll('link[rel="stylesheet"]'));
const paths = links.map(link => new URL(link.href).pathname);
const main = links.find(link => new URL(link.href).pathname === '/zxs.css');
const flatten = rules => Array.from(rules).flatMap(rule => [rule, ...(rule.cssRules ? flatten(rule.cssRules) : [])]);
const rules = main?.sheet ? flatten(main.sheet.cssRules) : [];
const imports = rules.filter(rule => rule.type === CSSRule.IMPORT_RULE).length;
const faces = rules.filter(rule => rule.type === CSSRule.FONT_FACE_RULE)
  .map(rule => ({family:rule.style.getPropertyValue('font-family').replaceAll('"',''),
    weight:rule.style.getPropertyValue('font-weight'),src:rule.style.getPropertyValue('src')}));
const dataIcons = rules.filter(rule => rule.style?.getPropertyValue('--zxs-block-icon').includes('data:image/svg+xml')).length;
const requestedCSS = performance.getEntriesByType('resource').map(entry => new URL(entry.name))
  .filter(url => url.origin === location.origin && url.pathname.endsWith('.css')).map(url => url.pathname);
return {paths,imports,faces,dataIcons,requestedCSS,ruleCount:rules.length,
  observation:window.__zxsStylesheets,
  ready:document.documentElement.dataset.zxsStartup,errors:!!document.getElementById('zxs-startup-error')};
"#)?;
    if state["paths"] != json!(["/styles/startup.css", "/zxs.css"])
        || state["imports"] != 0
        || state["dataIcons"] != 3
        || state["ready"] != "ready"
        || state["errors"] != false
        || state["observation"]["ready"] != 1
        || state["observation"]["failures"] != json!([])
        || !state["ruleCount"].as_u64().is_some_and(|count| count > 0)
    {
        return Err(format!("unexpected release stylesheet structure: {state}").into());
    }
    let faces = state["faces"].as_array().ok_or("missing CSS font rules")?;
    for (weight, file) in [
        ("400", "interface-regular.woff2"),
        ("700", "interface-bold.woff2"),
    ] {
        if !faces.iter().any(|face| {
            face["family"] == "ZxsUI"
                && face["weight"] == weight
                && face["src"]
                    .as_str()
                    .is_some_and(|source| source.contains(file))
        }) {
            return Err(format!("original {weight} font face missing: {state}").into());
        }
    }
    let requested = state["requestedCSS"]
        .as_array()
        .ok_or("missing CSS resource timings")?;
    for path in ["/styles/startup.css", "/zxs.css"] {
        if event_count(&state, path, "load") != 1 || event_count(&state, path, "error") != 0 {
            return Err(format!("required stylesheet did not load exactly once: {state}").into());
        }
        if !requested.iter().any(|item| item == path) {
            return Err(format!("missing stylesheet resource observation {path}: {state}").into());
        }
    }
    if requested
        .iter()
        .any(|item| item != "/styles/startup.css" && item != "/zxs.css")
    {
        return Err(format!("release still requested imported CSS: {state}").into());
    }
    Ok(())
}
