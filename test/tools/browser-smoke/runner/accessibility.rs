//! Actual product controls and computed styles, with bounded decoration fault injection.
use super::*;

pub(super) fn run(browser: &Browser) -> Result<()> {
    let original_theme = browser.script("return document.documentElement.dataset.zxsTheme")?;
    let result = check(browser);
    // Restore even on failure; these are isolated smoke-test browser settings.
    let cleanup = restore_decoration(browser);
    let theme_cleanup = select_theme(browser, original_theme.as_str().unwrap_or("light"));
    result?;
    cleanup?;
    theme_cleanup?;
    println!("PASS real input keyboard focus and Select contour fallback visibility in light/dark");
    Ok(())
}

fn restore_decoration(browser: &Browser) -> Result<()> {
    let restored = browser.script(
        "const saved=window.__zxsSmokeDecorationFault;\
         if(!saved)return true;\
         const style=saved.panel.style;\
         if(saved.value)style.setProperty('box-shadow',saved.value,saved.priority);\
         else style.removeProperty('box-shadow');\
         const restored=style.getPropertyValue('box-shadow')===saved.value && \
             style.getPropertyPriority('box-shadow')===saved.priority;\
         if(restored)delete window.__zxsSmokeDecorationFault;\
         return restored",
    )?;
    if restored == true {
        Ok(())
    } else {
        Err("could not restore the original inline box-shadow declaration".into())
    }
}

pub(super) fn select_theme(browser: &Browser, theme: &str) -> Result<()> {
    if browser.script("return document.documentElement.dataset.zxsTheme")? != theme {
        let (en, zh) = if theme == "dark" {
            ("Switch to dark mode", "切换到深色模式")
        } else {
            ("Switch to light mode", "切换到浅色模式")
        };
        browser.click(&browser.button(en, zh)?)?;
    }
    wait_for("selected theme", || {
        if browser.script("return document.documentElement.dataset.zxsTheme")? == theme {
            Ok(())
        } else {
            Err("theme did not change".into())
        }
    })
}

fn check(browser: &Browser) -> Result<()> {
    for theme in ["light", "dark"] {
        select_theme(browser, theme)?;
        browser.click(&browser.button("Account settings", "账号设置")?)?;
        let username = browser.css(".zxs-account-form input[name='username']")?;
        browser.click(&username)?;
        // Real WebDriver Tab input, not focus() or a synthetic DOM event.
        browser.keys(&username, "\u{e004}")?;
        wait_for("visible compact password keyboard focus", || {
            let focus = browser.script(
                "const e=document.querySelector('.zxs-account-form input[name=current_password]');\
                     const s=getComputedStyle(e);\
                     return document.activeElement===e && e.matches(':focus-visible') && \
                     parseFloat(s.borderBottomWidth)===3 && s.borderBottomStyle==='dashed' && \
                     (parseFloat(s.outlineWidth)===0 || s.outlineStyle==='none')",
            )?;
            if focus == true {
                Ok(())
            } else {
                Err(format!("invisible compact input focus in {theme}").into())
            }
        })?;
        input_surface_viewports(browser)?;
        browser.click(&browser.button("History", "历史记录")?)?;
        let trigger = browser.css(".zxs-history-toolbar .zxs-select-trigger")?;
        if browser.script("const root=document.querySelector('.zxs-history-toolbar .zxs-select'),name=document.documentElement.lang==='zh-CN'?'硬盘':'Disk';return !root.querySelector('label')&&[...root.querySelectorAll('[role=combobox],[role=listbox]')].every(e=>e.getAttribute('aria-label')===name&&!e.hasAttribute('aria-labelledby'))")? != true {
            return Err("history disk selector retains a visible label row or lost its accessible name".into());
        }
        browser.click(&trigger)?;
        visible_menu(browser, false, theme, "normal paint")?;
        black_select_borders(browser, theme)?;
        browser.keys(&trigger, "\u{e00c}")?; // Escape.
        browser.keys(&trigger, "\u{e015}")?; // ArrowDown reopens with keyboard focus.
        visible_menu(browser, false, theme, "keyboard reopen")?;
        black_select_borders(browser, theme)?;
        if browser.script("const e=document.querySelector('.zxs-history-toolbar .zxs-select-trigger'),s=getComputedStyle(e);return document.activeElement===e&&e.matches(':focus-visible')&&parseFloat(s.outlineWidth)>0&&s.outlineStyle!=='none'&&!!document.getElementById(e.getAttribute('aria-activedescendant'))")? != true {
            return Err(format!("Select keyboard focus or active option lost in {theme}").into());
        }
        browser.keys(&trigger, "\u{e00c}")?;

        browser.click(&trigger)?;
        visible_menu(browser, false, theme, "before decoration injection")?;
        // The controller owns an inline !important shadow declaration.
        // A stylesheet rule cannot override it. Preserve its exact
        // value/priority and prove the inline fault reaches computed
        // style before waiting for the real controller's fallback.
        let injected = browser.script(
                    "if(window.__zxsSmokeDecorationFault)throw new Error('decoration fault already active');\
                     const panel=document.querySelector('.zxs-history-toolbar .zxs-select-panel:popover-open');\
                     if(!panel)throw new Error('open history Select is missing');\
                     window.__zxsSmokeDecorationFault={panel,\
                         value:panel.style.getPropertyValue('box-shadow'),\
                         priority:panel.style.getPropertyPriority('box-shadow')};\
                     panel.style.setProperty('box-shadow','inset 0 0 1px black','important');\
                     return getComputedStyle(panel).boxShadow",
                )?;
        if !injected
            .as_str()
            .is_some_and(|shadow| shadow.split_whitespace().any(|token| token == "inset"))
        {
            return Err(
                format!("decoration fault did not reach computed box-shadow: {injected}").into(),
            );
        }
        visible_menu(browser, true, theme, "unsupported inset shadow")?;
        black_select_borders(browser, theme)?;
        restore_decoration(browser)?;
        visible_menu(browser, false, theme, "after decoration restoration")?;
        black_select_borders(browser, theme)?;
        browser.keys(&trigger, "\u{e00c}")?;
    }
    Ok(())
}

/// Observe real text controls without adding, disabling or restyling product DOM.
pub(super) fn input_surfaces(browser: &Browser) -> Result<()> {
    wait_for(
        "native text fields follow their scoped line and focus policy with no stale G4 paint",
        || {
            let evidence = browser.script(include_str!("input_surfaces.js"))?;
            if evidence["valid"] == true {
                Ok(())
            } else {
                Err(format!("native input surfaces: {evidence}").into())
            }
        },
    )
}

pub(super) fn input_surface_viewports(browser: &Browser) -> Result<()> {
    with_cleanup(
        || {
            for (width, height) in [(320, 568), (980, 760)] {
                browser.cdp(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
                )?;
                input_surfaces(browser)?;
            }
            Ok(())
        },
        || {
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            Ok(())
        },
    )
}

fn visible_menu(browser: &Browser, fallback: bool, theme: &str, mode: &str) -> Result<()> {
    wait_for("visible completed Select contour", || {
        let state = browser.script(
            "const p=document.querySelector('.zxs-select-panel:popover-open');\
             if(!p)return null;const s=getComputedStyle(p);const r=p.getBoundingClientRect();\
             return {state:p.dataset.zxsContourState,reason:p.dataset.zxsContourReason,\
                     visible:s.visibility==='visible'&&s.display!=='none'&&r.width>0&&r.height>0}",
        )?;
        let expected = if fallback { "fallback" } else { "clip" };
        let expected_reason = match mode {
            "unsupported inset shadow" => Some("inset-or-unsupported-shadow"),
            _ => None,
        };
        if state["state"] == expected
            && state["visible"] == true
            && expected_reason.is_none_or(|reason| state["reason"] == reason)
        {
            Ok(())
        } else {
            Err(format!(
                "Select {theme}/{mode}: expected visible {expected} ({expected_reason:?}), got {state}"
            )
            .into())
        }
    })
}

fn black_select_borders(browser: &Browser, theme: &str) -> Result<()> {
    wait_for("actual pure-black Select border paint", || {
        let evidence = browser.command(
            "POST",
            "/execute/async",
            Some(json!({"script": include_str!("select_borders.js"), "args": []})),
        )?;
        if evidence["valid"] == true {
            Ok(())
        } else {
            Err(format!("Select border paint in {theme}: {evidence}").into())
        }
    })
}

#[cfg(test)]
#[path = "../unit/select_borders_tests.rs"]
mod tests;
