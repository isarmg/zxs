//! Real anonymous-page controls, not an HTML mock or substituted WASM.
use super::*;

const LOGIN_FORM: &str = ".authentication-card > form[data-auth-form='login']";
const USERNAME: &str = "[name='username']";
const PASSWORD: &str = "[name='password']";

pub(super) fn run(browser: &Browser, origin: &str, output: Option<&Path>) -> Result<()> {
    with_cleanup(
        || check(browser, origin, output),
        || {
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            browser.command("POST", "/url", Some(json!({"url":origin})))?;
            browser.css(&format!(
                "{} {}:not([disabled]):not([readonly])",
                LOGIN_FORM, PASSWORD
            ))?;
            Ok(())
        },
    )?;
    println!(
        "PASS anonymous existing six-row layout, visible controls, status reload and keyboard focus at four viewports"
    );
    Ok(())
}

fn geometry(browser: &Browser, phase: &str) -> Result<Value> {
    // Clicking below the fold scrolls the page. Measure after restoring the
    // viewport and allowing ResizeObserver/G4 to reposition real foregrounds.
    browser.script("window.scrollTo(0,0);return true")?;
    // This capture verifies the resting surfaces. A prior click can leave the
    // pointer over a now legitimately highlighted row; move it without changing
    // focus, form state or product DOM before measuring the transparent rows.
    browser.command(
        "POST",
        "/actions",
        Some(
            json!({"actions":[{"type":"pointer","id":"auth-layout-mouse",
            "parameters":{"pointerType":"mouse"},"actions":[{"type":"pointerMove",
                "duration":0,"origin":"viewport","x":0,"y":0}]}]}),
        ),
    )?;
    thread::sleep(Duration::from_millis(150));
    browser.command(
        "POST",
        "/execute/async",
        Some(json!({
            "script": include_str!("auth_layout.js"), "args": ["existing", phase]
        })),
    )
}

fn resize(browser: &Browser, width: u32, height: u32) -> Result<()> {
    browser.cdp(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
    )?;
    wait_for("anonymous fonts and viewport", || {
        if browser.script(&format!("return innerWidth==={width} && innerHeight==={height} && document.fonts.status==='loaded'"))? == true {
            Ok(())
        } else {
            Err("viewport or font loading pending".into())
        }
    })
}

fn capture(
    browser: &Browser,
    phase: &str,
    output: Option<&Path>,
    evidence: &mut Vec<Value>,
) -> Result<()> {
    let geometry = geometry(browser, phase)?;
    let width = &geometry["viewport"]["width"];
    let height = &geometry["viewport"]["height"];
    if let Some(dir) = output {
        let name = format!("login-{width}x{height}-existing-{phase}.png");
        let data = browser.command("GET", "/screenshot", None)?;
        fs::write(
            dir.join(&name),
            STANDARD.decode(data.as_str().ok_or("missing login screenshot")?)?,
        )?;
    }
    evidence.push(json!({"width":width,"height":height,"setup_state":"existing","phase":phase,"geometry":geometry}));
    // Persist before asserting so failed containment still retains its evidence.
    if let Some(dir) = output {
        fs::write(
            dir.join("login-layout.json"),
            json_codec::to_vec_pretty(
                &json!({"renderer":"production WASM with real temporary server; CSS viewport sizes, not physical mobile devices","checks":evidence}),
            )?,
        )?;
    }
    accessibility::input_surfaces(browser)?;
    if geometry["valid"] != true {
        return Err(
            format!("anonymous {width}x{height}, setup existing {phase}: {geometry}").into(),
        );
    }
    Ok(())
}

fn check(browser: &Browser, origin: &str, output: Option<&Path>) -> Result<()> {
    if let Some(dir) = output {
        fs::create_dir_all(dir)?;
    }
    let mut evidence = Vec::new();
    for (width, height) in [(320, 568), (390, 844), (980, 760), (1440, 900)] {
        browser.command("POST", "/url", Some(json!({"url":origin})))?;
        resize(browser, width, height)?;
        for phase in ["initial", "refreshed", "invalid-login"] {
            if phase == "refreshed" {
                // Reload remounts the real app and re-reads setup status. Existing
                // accounts no longer expose an unrelated first-setup action.
                browser.command("POST", "/refresh", Some(json!({})))?;
            }
            let username = browser.css(&format!(
                "{} {}:not([disabled]):not([readonly])",
                LOGIN_FORM, USERNAME
            ))?;
            browser.css(&format!(
                "{} {}:not([disabled]):not([readonly])",
                LOGIN_FORM, PASSWORD
            ))?;
            wait_for("anonymous fonts and viewport", || {
                if browser.script(&format!("return innerWidth==={width} && innerHeight==={height} && document.fonts.status==='loaded'"))? == true { Ok(()) } else { Err("viewport or font loading pending".into()) }
            })?;
            if phase == "invalid-login" {
                browser.click(&username)?;
                browser.keys(&username, "\u{e004}")?;
                let selector = json_codec::to_string(&format!("{} {}", LOGIN_FORM, PASSWORD))?;
                if browser.script(&format!("const e=document.activeElement,s=getComputedStyle(e);return e===document.querySelector({selector})&&e.matches(':focus-visible')&&parseFloat(s.borderBottomWidth)===0&&s.borderBottomStyle==='none'&&parseFloat(s.outlineWidth)===0&&parseFloat(s.borderInlineStartWidth)===2&&s.borderInlineStartStyle==='solid'&&!['transparent','rgba(0, 0, 0, 0)'].includes(s.borderInlineStartColor)"))? != true {
                    return Err(format!("anonymous {width}x{height} keyboard focus is invisible").into());
                }
                accessibility::input_surfaces(browser)?;
                // Empty login is local validation, never an authentication request.
                browser.click(&browser.css(&format!(
                    "{} .zxs-auth-actions > button[type=submit]",
                    LOGIN_FORM
                ))?)?;
                browser.css(&format!("{} .zxs-auth-information .zxs-error", LOGIN_FORM))?;
            }
            capture(browser, phase, output, &mut evidence)?;
        }
    }
    Ok(())
}
