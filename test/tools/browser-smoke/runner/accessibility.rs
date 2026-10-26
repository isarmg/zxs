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

fn select_theme(browser: &Browser, theme: &str) -> Result<()> {
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
                     parseFloat(s.outlineWidth)>0 && s.outlineStyle!=='none'",
            )?;
            if focus == true {
                Ok(())
            } else {
                Err(format!("invisible compact input focus in {theme}").into())
            }
        })?;
        browser.click(&browser.button("History", "历史记录")?)?;
        let trigger = browser.css(".zxs-history-toolbar .zxs-select-trigger")?;
        browser.click(&trigger)?;
        visible_menu(browser, false, theme, "normal paint")?;
        browser.keys(&trigger, "\u{e00c}")?; // Escape.

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
        restore_decoration(browser)?;
        visible_menu(browser, false, theme, "after decoration restoration")?;
        browser.keys(&trigger, "\u{e00c}")?;
    }
    Ok(())
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
