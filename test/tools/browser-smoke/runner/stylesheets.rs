//! Inspect the real release CSSOM and block only the main CSS transport.
use super::{Browser, Result, json, wait_for, with_cleanup};

pub(super) fn run(browser: &Browser, origin: &str) -> Result<()> {
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    browser.css("input[autocomplete='current-password']")?;
    normal_bundle(browser)?;
    browser.cdp("Network.setCacheDisabled", json!({"cacheDisabled":true}))?;
    with_cleanup(
        || {
            browser.cdp(
                "Network.setBlockedURLs",
                json!({"urls":[format!("{origin}/zxs.css")]}),
            )?;
            browser.command("POST", "/url", Some(json!({"url":origin})))?;
            wait_for("independently styled main-CSS failure", || {
                let state = browser.script(
                    r#"
const error = document.getElementById('zxs-startup-error');
if (!error) return null;
const style = getComputedStyle(error);
const main = document.querySelector('link[href="./zxs.css"]');
const startup = document.querySelector('link[href="./styles/startup.css"]');
return {phase:document.documentElement.dataset.zxsStartup,mainLoaded:!!main.sheet,
  startupLoaded:!!startup.sheet,box:style.boxSizing,font:style.fontFamily,
  rootHidden:getComputedStyle(document.getElementById('root')).display==='none',
  retry:!!error.querySelector('button'),width:error.getBoundingClientRect().width};
"#,
                )?;
                if state["phase"] == "failed"
                    && state["mainLoaded"] == false
                    && state["startupLoaded"] == true
                    && state["box"] == "border-box"
                    && state["font"]
                        .as_str()
                        .is_some_and(|font| font.contains("system-ui"))
                    && state["rootHidden"] == true
                    && state["retry"] == true
                    && state["width"].as_f64().is_some_and(|width| width > 0.0)
                {
                    Ok(())
                } else {
                    Err(format!("main-CSS failure state: {state}").into())
                }
            })
        },
        || {
            let unblock = browser.cdp("Network.setBlockedURLs", json!({"urls":[]}));
            let cache = browser.cdp("Network.setCacheDisabled", json!({"cacheDisabled":false}));
            unblock?;
            cache?;
            Ok(())
        },
    )?;
    // Exercise the real retry button after transport recovery, not a synthetic
    // readiness event or an injected replacement stylesheet.
    browser.click(&browser.css("#zxs-startup-error button")?)?;
    browser.css("input[autocomplete='current-password']")?;
    normal_bundle(browser)?;
    println!(
        "PASS release CSSOM without import requests, original fonts, independent startup CSS and retry"
    );
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
  ready:document.documentElement.dataset.zxsStartup,errors:!!document.getElementById('zxs-startup-error')};
"#)?;
    if state["paths"] != json!(["/styles/startup.css", "/zxs.css"])
        || state["imports"] != 0
        || state["dataIcons"] != 3
        || state["ready"] != "ready"
        || state["errors"] != false
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
