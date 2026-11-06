//! Read real card geometry and use native disclosure controls. This scene does
//! not synthesize history records, change permissions or submit metadata.
use super::*;

pub(super) fn open_first(browser: &Browser) -> Result<()> {
    if browser.script("const node=document.querySelector('.zxs-version-node');if(!node)throw new Error('history card missing');return node.querySelector('.zxs-node-disclosure').open")? != true {
        browser.click(&browser.css(".zxs-version-node .zxs-node-card")?)?;
    }
    Ok(())
}

pub(super) fn open_current(browser: &Browser) -> Result<()> {
    if browser.script("const card=document.querySelector('.zxs-node-card[aria-current=true]');if(!card)throw new Error('current history card missing');return card.parentElement.open")? != true {
        browser.click(&browser.css(".zxs-node-card[aria-current=true]")?)?;
    }
    Ok(())
}

fn check_cards(browser: &Browser) -> Result<()> {
    wait_for(
        "two-row 10ch cards with current G4 paint and historical G4 clip output",
        || {
            if browser.script(include_str!("node_cards.js"))? == true {
                Ok(())
            } else {
                Err(
                    "history card G4 clip, geometry, accessible names or icon-only row differ"
                        .into(),
                )
            }
        },
    )?;
    let summary = browser.css(".zxs-version-node .zxs-node-card")?;
    browser.script(r#"
        const card=document.querySelector('.zxs-version-node .zxs-node-card');
        const original=window.fetch;
        const p=window.__zxsCardBefore={card,detail:card.nextElementSibling,open:card.parentElement.open,writes:0,original};
        window.fetch=function(input,init){
            const method=String(init?.method??(input instanceof Request?input.method:'GET')).toUpperCase();
            if(method!=='GET'&&method!=='HEAD')p.writes++;
            return Reflect.apply(original,this,arguments);
        };
        return true;
    "#)?;
    with_cleanup(
        || {
            browser.keys(&summary, "\u{e007}")?;
            wait_for("Enter toggles native history details", || {
                if browser.script(
                    r#"
                    const p=window.__zxsCardBefore,s=getComputedStyle(p.card);
                    return p.card.parentElement.open!==p.open&&p.card.nextElementSibling===p.detail
                        &&document.activeElement===p.card&&p.card.matches(':focus-visible')
                        &&s.outlineStyle==='solid'&&parseFloat(s.outlineWidth)>=3
                        &&p.card.dataset.zxsContourState===(p.card.getAttribute('aria-current')==='true'?'paint':'clip');
                "#,
                )? == true
                {
                    Ok(())
                } else {
                    Err(
                        "native card did not toggle, preserve controls or show keyboard focus"
                            .into(),
                    )
                }
            })?;
            browser.keys(&summary, "\u{e00d}")?;
            wait_for("Space restores the same node details", || {
                if browser.script("const p=window.__zxsCardBefore;return p.card.parentElement.open===p.open&&p.card.nextElementSibling===p.detail&&p.writes===0")? == true { Ok(()) }
                else { Err("native card did not restore its original disclosure state".into()) }
            })?;
            Ok(())
        },
        || {
            browser.script("const p=window.__zxsCardBefore;window.fetch=p.original;p.card.parentElement.open=p.open;delete window.__zxsCardBefore;return true")?;
            Ok(())
        },
    )
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    let original = browser.script("return document.documentElement.dataset.zxsTheme")?;
    let theme = original.as_str().ok_or("missing current card theme")?;
    with_cleanup(
        || {
            for theme in ["light", "dark"] {
                accessibility::select_theme(browser, theme)?;
                check_cards(browser)?;
            }
            Ok(())
        },
        || accessibility::select_theme(browser, theme),
    )?;
    println!(
        "PASS two-row 10ch history cards, action-only accessible colored icons and native keyboard disclosure; no metadata writes"
    );
    println!(
        "PASS current card G4 paint and historical card G4 clip, visible current ring and keyboard focus in light/dark; no inset fallback"
    );
    Ok(())
}
