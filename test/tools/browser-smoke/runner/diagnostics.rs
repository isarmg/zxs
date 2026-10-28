//! Read-only evidence for a real-browser failure. No application state access,
//! synthetic events, request interception, or response replacement.
use super::{Browser, Result, STANDARD, fs, json};
use base64::Engine as _;
use std::path::Path;

const OBSERVER: &str = r#"
window.__zxsSmokeErrors = [];
addEventListener('error', e => window.__zxsSmokeErrors.push(String(e.message)));
addEventListener('unhandledrejection', e => window.__zxsSmokeErrors.push(String(e.reason)));
"#;

pub(super) fn install(browser: &Browser) -> Result<()> {
    browser.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":OBSERVER}),
    )?;
    Ok(())
}
pub(super) fn failure(browser: &Browser, directory: &Path, password: &str) {
    if let Err(error) = fs::create_dir_all(directory) {
        eprintln!("failure artifact directory unavailable: {error}");
    }
    let save = |name: &str, result: Result<json_codec::Value>| match result {
        Ok(value) => {
            let text = json_codec::to_string_pretty(&value)
                .unwrap_or_default()
                .replace(password, "[REDACTED]");
            eprintln!("--- browser failure {name} ---\n{text}");
            if let Err(error) = fs::write(directory.join(name), text) {
                eprintln!("cannot save {name}: {error}");
            }
        }
        Err(error) => eprintln!("browser failure {name} unavailable: {error}"),
    };
    save("page.json", browser.script("return {url:location.href,title:document.title,text:document.body.innerText,errors:window.__zxsSmokeErrors||[],inputs:[...document.querySelectorAll('input[type=file]')].map(e=>({multiple:e.multiple,filesLength:e.files?.length,files:[...(e.files||[])].map(f=>({name:f.name,size:f.size}))})),requests:performance.getEntriesByType('resource').filter(e=>e.name.includes('/api/')).map(e=>({name:e.name,start:e.startTime,duration:e.duration}))}"));
    save("dom.json", browser.script("const root=document.documentElement.cloneNode(true);root.querySelectorAll('input[type=password]').forEach(e=>e.removeAttribute('value'));return root.outerHTML"));
    save(
        "console.json",
        browser.command("POST", "/log", Some(json!({"type":"browser"}))),
    );
    // Network summaries deliberately exclude headers, cookies, and request or
    // response bodies (the login request contains the throwaway test password).
    let network = browser.command("POST", "/log", Some(json!({"type":"performance"}))).map(|logs| {
        let rows: Vec<_> = logs.as_array().into_iter().flatten().filter_map(|entry| {
            let value: json_codec::Value = json_codec::from_str(entry["message"].as_str()?).ok()?;
            let message = &value["message"];
            let params = &message["params"];
            match message["method"].as_str()? {
                "Network.requestWillBeSent" => Some(json!({"event":"request","id":params["requestId"],"url":params["request"]["url"],"method":params["request"]["method"]})),
                "Network.responseReceived" => Some(json!({"event":"response","id":params["requestId"],"url":params["response"]["url"],"status":params["response"]["status"]})),
                "Network.loadingFailed" => Some(json!({"event":"failed","id":params["requestId"],"error":params["errorText"],"blockedReason":params["blockedReason"],"canceled":params["canceled"],"type":params["type"]})),
                _ => None,
            }
        }).collect();
        json!(rows)
    });
    save("network.json", network);
    match browser
        .command("GET", "/screenshot", None)
        .and_then(|value| Ok(STANDARD.decode(value.as_str().ok_or("missing screenshot")?)?))
    {
        Ok(png) => {
            if let Err(error) = fs::write(directory.join("failure.png"), png) {
                eprintln!("cannot save failure screenshot: {error}");
            }
        }
        Err(error) => eprintln!("failure screenshot unavailable: {error}"),
    }
}
