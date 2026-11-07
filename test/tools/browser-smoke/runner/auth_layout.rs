//! Real anonymous-page controls, not an HTML mock or substituted WASM.
use super::*;

#[derive(Clone, Copy)]
enum SetupState {
    Existing,
    Empty,
    Unconfirmed,
    RateLimited,
}
impl SetupState {
    fn form(self) -> &'static str {
        match self {
            Self::Existing | Self::RateLimited => {
                ".authentication-card > form[data-auth-form='login']"
            }
            Self::Empty | Self::Unconfirmed => {
                ".authentication-card > form[data-first-account-form]"
            }
        }
    }
    fn user(self) -> &'static str {
        match self {
            Self::Existing | Self::RateLimited => "[name='username']",
            Self::Empty | Self::Unconfirmed => "[data-setup-user]",
        }
    }
    fn password(self) -> &'static str {
        match self {
            Self::Existing | Self::RateLimited => "[name='password']",
            Self::Empty | Self::Unconfirmed => "[data-setup-password]",
        }
    }
    fn final_phase(self) -> &'static str {
        match self {
            Self::Existing => "invalid-login",
            Self::Empty => "keyboard-focus",
            Self::Unconfirmed => "response-lost",
            Self::RateLimited => "status-rejected",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Empty => "empty",
            Self::Unconfirmed => "unconfirmed",
            Self::RateLimited => "rate-limited",
        }
    }
}

pub(super) fn run(browser: &Browser, origin: &str, output: Option<&Path>) -> Result<()> {
    run_with_setup(browser, origin, output, SetupState::Existing)
}

pub(super) fn first_account(browser: &Browser, origin: &str, output: Option<&Path>) -> Result<()> {
    run_with_setup(browser, origin, output, SetupState::Empty)
}

// Inspect the real lost-response fixture without reloading or replacing its
// feedback. Its long transport error and fourth action must remain in flow.
pub(super) fn unconfirmed(browser: &Browser, output: Option<&Path>) -> Result<()> {
    feedback_layout(browser, SetupState::Unconfirmed, output)
}

pub(super) fn rate_limited(browser: &Browser, output: Option<&Path>) -> Result<()> {
    feedback_layout(browser, SetupState::RateLimited, output)
}

fn feedback_layout(browser: &Browser, setup: SetupState, output: Option<&Path>) -> Result<()> {
    with_cleanup(
        || {
            if let Some(dir) = output {
                fs::create_dir_all(dir)?;
            }
            let mut evidence = Vec::new();
            for (width, height) in [(320, 568), (390, 844), (980, 760), (1440, 900)] {
                resize(browser, width, height)?;
                capture(browser, setup, setup.final_phase(), output, &mut evidence)?;
            }
            Ok(())
        },
        || {
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            Ok(())
        },
    )?;
    println!(
        "PASS anonymous {} complete feedback and all actions inside six-row card at four viewports",
        setup.name()
    );
    Ok(())
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
            browser.css(&format!(
                "{} {}:not([disabled]):not([readonly])",
                setup.form(),
                setup.password()
            ))?;
            Ok(())
        },
    )?;
    println!(
        "PASS anonymous {} six-row layout, visible controls, status reload and keyboard focus at four viewports",
        setup.name()
    );
    Ok(())
}

fn geometry(browser: &Browser, setup: SetupState, phase: &str) -> Result<Value> {
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
            "script": include_str!("auth_layout.js"), "args": [setup.name(), phase]
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
    setup: SetupState,
    phase: &str,
    output: Option<&Path>,
    evidence: &mut Vec<Value>,
) -> Result<()> {
    let geometry = geometry(browser, setup, phase)?;
    let width = &geometry["viewport"]["width"];
    let height = &geometry["viewport"]["height"];
    if let Some(dir) = output {
        let name = format!("login-{width}x{height}-{}-{phase}.png", setup.name());
        let data = browser.command("GET", "/screenshot", None)?;
        fs::write(
            dir.join(&name),
            STANDARD.decode(data.as_str().ok_or("missing login screenshot")?)?,
        )?;
    }
    evidence.push(json!({"width":width,"height":height,"setup_state":setup.name(),"phase":phase,"geometry":geometry}));
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
        return Err(format!(
            "anonymous {width}x{height}, setup {} {phase}: {geometry}",
            setup.name()
        )
        .into());
    }
    Ok(())
}

fn check(browser: &Browser, origin: &str, output: Option<&Path>, setup: SetupState) -> Result<()> {
    if let Some(dir) = output {
        fs::create_dir_all(dir)?;
    }
    let mut evidence = Vec::new();
    for (width, height) in [(320, 568), (390, 844), (980, 760), (1440, 900)] {
        browser.command("POST", "/url", Some(json!({"url":origin})))?;
        resize(browser, width, height)?;
        for phase in ["initial", "refreshed", setup.final_phase()] {
            if phase == "refreshed" {
                // Reload remounts the real app and re-reads setup status. Existing
                // accounts no longer expose an unrelated first-setup action.
                browser.command("POST", "/refresh", Some(json!({})))?;
            }
            let username = browser.css(&format!(
                "{} {}:not([disabled]):not([readonly])",
                setup.form(),
                setup.user()
            ))?;
            browser.css(&format!(
                "{} {}:not([disabled]):not([readonly])",
                setup.form(),
                setup.password()
            ))?;
            wait_for("anonymous fonts and viewport", || {
                if browser.script(&format!("return innerWidth==={width} && innerHeight==={height} && document.fonts.status==='loaded'"))? == true { Ok(()) } else { Err("viewport or font loading pending".into()) }
            })?;
            if phase == setup.final_phase() {
                browser.click(&username)?;
                browser.keys(&username, "\u{e004}")?;
                let selector =
                    json_codec::to_string(&format!("{} {}", setup.form(), setup.password()))?;
                if browser.script(&format!("const e=document.activeElement,s=getComputedStyle(e);return e===document.querySelector({selector})&&e.matches(':focus-visible')&&parseFloat(s.borderBottomWidth)===0&&s.borderBottomStyle==='none'&&parseFloat(s.outlineWidth)===0&&parseFloat(s.borderInlineStartWidth)===2&&s.borderInlineStartStyle==='solid'&&!['transparent','rgba(0, 0, 0, 0)'].includes(s.borderInlineStartColor)"))? != true {
                    return Err(format!("anonymous {width}x{height} keyboard focus is invisible").into());
                }
                accessibility::input_surfaces(browser)?;
                if matches!(setup, SetupState::Existing) {
                    // Empty login is local validation, never an authentication request.
                    browser.click(&browser.css(&format!(
                        "{} .zxs-auth-actions > button[type=submit]",
                        setup.form()
                    ))?)?;
                    browser.css(&format!(
                        "{} .zxs-auth-information .zxs-error",
                        setup.form()
                    ))?;
                }
            }
            capture(browser, setup, phase, output, &mut evidence)?;
        }
    }
    Ok(())
}
