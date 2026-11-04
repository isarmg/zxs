//! Real anonymous-page controls, not an HTML mock or substituted WASM.
use super::*;

#[derive(Clone, Copy)]
enum SetupState {
    Existing,
    Empty,
}
impl SetupState {
    fn name(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Empty => "empty",
        }
    }
}

pub(super) fn run(browser: &Browser, origin: &str, output: Option<&Path>) -> Result<()> {
    run_with_setup(browser, origin, output, SetupState::Existing)
}

pub(super) fn first_account(browser: &Browser, origin: &str, output: Option<&Path>) -> Result<()> {
    run_with_setup(browser, origin, output, SetupState::Empty)
}

fn run_with_setup(
    browser: &Browser,
    origin: &str,
    output: Option<&Path>,
    setup: SetupState,
) -> Result<()> {
    with_cleanup(
        || check(browser, origin, output, setup),
        || {
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            browser.command("POST", "/url", Some(json!({"url":origin})))?;
            browser.css("input[autocomplete='current-password']")?;
            Ok(())
        },
    )?;
    println!(
        "PASS anonymous {} setup layout, visible controls, status refresh and login focus at four viewports",
        setup.name()
    );
    Ok(())
}

fn geometry(browser: &Browser, setup: SetupState) -> Result<Value> {
    // Clicking below the fold scrolls the page. Measure after restoring the
    // viewport and allowing ResizeObserver/G4 to reposition real foregrounds.
    browser.script("window.scrollTo(0,0);return true")?;
    thread::sleep(Duration::from_millis(150));
    browser.command(
        "POST",
        "/execute/async",
        Some(json!({
            "script": include_str!("auth_layout.js"), "args": [setup.name()]
        })),
    )
}

fn check(browser: &Browser, origin: &str, output: Option<&Path>, setup: SetupState) -> Result<()> {
    if let Some(dir) = output {
        fs::create_dir_all(dir)?;
    }
    let mut evidence = Vec::new();
    for (width, height) in [(320, 568), (390, 844), (980, 760), (1440, 900)] {
        browser.command("POST", "/url", Some(json!({"url":origin})))?;
        browser.cdp(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
        )?;
        browser.css(".authentication-card > form [name='username']")?;
        browser.css(".authentication-card > section button:not([disabled])")?;
        wait_for("anonymous fonts and viewport", || {
            if browser.script(&format!("return innerWidth==={width} && innerHeight==={height} && document.fonts.status==='loaded'"))? == true { Ok(()) } else { Err("viewport or font loading pending".into()) }
        })?;
        for phase in ["initial", "refreshed", "invalid-login"] {
            let check =
                browser.css("[data-first-account-form] [data-setup-check]:not([disabled])")?;
            if phase == "refreshed" {
                browser.click(&check)?;
                browser.css("[data-first-account-form] [data-setup-check]:not([disabled])")?;
            }
            if phase == "invalid-login" {
                let username = browser.css(".authentication-card > form [name='username']")?;
                browser.click(&username)?;
                browser.keys(&username, "\u{e004}")?;
                if browser.script("const e=document.activeElement,s=getComputedStyle(e);return e.name==='password'&&e.matches(':focus-visible')&&parseFloat(s.borderBottomWidth)===3&&s.borderBottomStyle==='dashed'&&(parseFloat(s.outlineWidth)===0||s.outlineStyle==='none')")? != true {
                    return Err(format!("anonymous {width}x{height} keyboard focus is invisible").into());
                }
                accessibility::input_surfaces(browser)?;
                // Empty submission is local validation, never an authentication request.
                browser
                    .click(&browser.css(".authentication-card > form > button[type=submit]")?)?;
                browser.css(".authentication-card > form > .zxs-error")?;
            }
            let geometry = geometry(browser, setup)?;
            if let Some(dir) = output {
                let name = format!("login-{width}x{height}-{}-{phase}.png", setup.name());
                let data = browser.command("GET", "/screenshot", None)?;
                fs::write(
                    dir.join(&name),
                    STANDARD.decode(data.as_str().ok_or("missing login screenshot")?)?,
                )?;
            }
            evidence.push(json!({"width":width,"height":height,"setup_state":setup.name(),"phase":phase,"geometry":geometry}));
            // Persist real evidence before asserting, so a failed layout still
            // leaves its screenshot and exact measurements in the CI artifact.
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
                return Err(format!(
                    "anonymous {width}x{height}, setup {} {phase}: {geometry}",
                    setup.name()
                )
                .into());
            }
        }
    }
    Ok(())
}
