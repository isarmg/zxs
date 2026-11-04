//! Actual page boundaries and row clearance in both themes and responsive sizes.
use super::*;

const VIEWS: [(u32, u32); 4] = [(320, 568), (390, 844), (980, 760), (1440, 900)];

fn viewport(browser: &Browser, width: u32, height: u32) -> Result<()> {
    browser.cdp(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
    )?;
    wait_for("page layout fonts and viewport", || {
        if browser.script(&format!(
            "return innerWidth==={width}&&innerHeight==={height}&&document.fonts.status==='loaded'"
        ))? == true
        {
            Ok(())
        } else {
            Err("page layout not settled".into())
        }
    })?;
    thread::sleep(Duration::from_millis(150));
    Ok(())
}
fn search(browser: &Browser, empty: bool) -> Result<()> {
    let query = if empty {
        "zxs-layout-no-match-739841"
    } else {
        ""
    };
    browser.script(&format!("const input=document.querySelector('.zxs-file-search input');input.value={};input.dispatchEvent(new Event('input',{{bubbles:true}}));return true",json!(query)))?;
    wait_for("real file query for separator layout", || {
        if browser.script(&format!("const page=document.querySelector('.zxs-file-manager'),nav=page.querySelector('.zxs-file-pagination');return page.getAttribute('aria-busy')==='false'&&(page.querySelectorAll('tbody tr').length===0)==={empty}&&![...nav.querySelectorAll('button')].some(e=>['Retry','重试'].includes(e.textContent.trim()))"))? == true { Ok(()) } else { Err("real file query is pending or failed".into()) }
    })
}
pub(super) fn files(browser: &Browser) -> Result<()> {
    let original_theme = browser.script("return document.documentElement.dataset.zxsTheme")?;
    with_cleanup(
        || {
            for empty in [false, true] {
                search(browser, empty)?;
                for theme in ["light", "dark"] {
                    accessibility::select_theme(browser, theme)?;
                    for (width, height) in VIEWS {
                        viewport(browser, width, height)?;
                        accessibility::input_surfaces(browser)?;
                        let geometry=browser.script(r#"
                        const nav=document.querySelector('.zxs-file-pagination'),panel=nav.closest('.zxs-file-list-panel');
                        const table=panel.querySelector('table'),scroll=table.parentElement;
                        const edge=table.querySelector('tbody tr:last-child')||table.querySelector('thead tr:last-child');
                        const line=getComputedStyle(nav);
                        const canvas=document.createElement('canvas');canvas.width=canvas.height=1;const context=canvas.getContext('2d');
                        const color=value=>{context.clearRect(0,0,1,1);context.fillStyle=value;context.fillRect(0,0,1,1);return [...context.getImageData(0,0,1,1).data].join(',');};
                        return {lineWidth:parseFloat(line.borderTopWidth),lineStyle:line.borderTopStyle,lineColor:color(line.borderTopColor),
                            expectedColor:color(line.getPropertyValue('--zxs-block-border').trim()),duplicate:[...edge.children].some(e=>parseFloat(getComputedStyle(e).borderBottomWidth)!==0),
                            gap:nav.getBoundingClientRect().top-scroll.getBoundingClientRect().bottom,
                            documentWidth:document.documentElement.scrollWidth};
                    "#)?;
                        if geometry["lineWidth"] != 1
                            || geometry["lineStyle"] != "solid"
                            || geometry["lineColor"] != geometry["expectedColor"]
                            || geometry["lineColor"] == "0,0,0,0"
                            || geometry["duplicate"] != false
                            || geometry["gap"].as_f64().is_none_or(|gap| gap.abs() > 0.5)
                            || geometry["documentWidth"]
                                .as_u64()
                                .is_none_or(|w| w > u64::from(width) + 1)
                        {
                            return Err(format!(
                                "file separator {theme} {width}x{height} empty={empty}: {geometry}"
                            )
                            .into());
                        }
                    }
                }
            }
            Ok(())
        },
        || {
            let query = search(browser, false);
            let metrics = browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}));
            let theme =
                accessibility::select_theme(browser, original_theme.as_str().unwrap_or("light"));
            query?;
            metrics?;
            theme?;
            Ok(())
        },
    )?;
    println!(
        "PASS single file-pagination separator with nonempty and empty real results in both themes at four viewports"
    );
    Ok(())
}
fn range_menu(browser: &Browser, theme: &str, width: u32) -> Result<()> {
    let trigger = browser.css(".zxs-monitor-range .zxs-select-trigger")?;
    with_cleanup(
        || {
            // Keyboard opening retains the same control and active-option semantics.
            browser.keys(&trigger, "\u{e015}")?;
            wait_for("status range menu border-box width", || {
                let geometry=browser.script(r#"
                    const trigger=document.querySelector('.zxs-monitor-range .zxs-select-trigger');
                    const panel=document.querySelector('.zxs-monitor-range .zxs-select-panel:popover-open');
                    if(!panel)return {valid:false,reason:'menu is not open'};
                    const t=trigger.getBoundingClientRect(),p=panel.getBoundingClientRect(),style=getComputedStyle(panel);
                    const visible=panel.checkVisibility({opacityProperty:true,visibilityProperty:true});
                    return {valid:visible&&style.boxSizing==='border-box'&&Math.abs(t.width-p.width)<=.5&&p.left>=0&&p.right<=innerWidth&&
                        trigger.getAttribute('aria-expanded')==='true'&&document.activeElement===trigger&&
                        !!document.getElementById(trigger.getAttribute('aria-activedescendant')),
                        visible,triggerWidth:t.width,panelWidth:p.width,left:p.left,right:p.right};
                "#)?;
                if geometry["valid"] == true {
                    Ok(())
                } else {
                    Err(format!("status range menu {theme} width={width}: {geometry}").into())
                }
            })
        },
        || {
            browser.keys(&trigger, "\u{e00c}")?;
            Ok(())
        },
    )
}
pub(super) fn status(browser: &Browser) -> Result<()> {
    let original_theme = browser.script("return document.documentElement.dataset.zxsTheme")?;
    with_cleanup(
        || {
            for theme in ["light", "dark"] {
                accessibility::select_theme(browser, theme)?;
                for (width, height) in VIEWS {
                    viewport(browser, width, height)?;
                    range_menu(browser, theme, width)?;
                    let geometry=browser.script(r#"
                    const panels=[...document.querySelectorAll('.zxs-monitor-page article.zxs-frame')];
                    const rows=panels.map(panel=>{
                        const first=panel.querySelector(':scope > .zxs-row');
                        if(!first)return {valid:false,reason:'missing shared row'};
                        const text=document.createTreeWalker(first,NodeFilter.SHOW_TEXT,{acceptNode:n=>n.textContent.trim()?NodeFilter.FILTER_ACCEPT:NodeFilter.FILTER_SKIP}).nextNode();
                        if(!text)return {valid:false,reason:'missing first-row text'};
                        const range=document.createRange();range.selectNodeContents(text);
                        const rowStyle=getComputedStyle(first),frameStyle=getComputedStyle(panel),box=first.getBoundingClientRect();
                        const gap=range.getBoundingClientRect().top-panel.getBoundingClientRect().top-parseFloat(frameStyle.borderTopWidth);
                        const minimum=(parseFloat(rowStyle.minHeight)-parseFloat(rowStyle.lineHeight))/2-1;
                        return {valid:box.height>=36&&gap>=minimum&&minimum>0,gap,minimum,height:box.height,label:panel.getAttribute('aria-label')};
                    });
                    return {valid:rows.length>=3&&rows.every(row=>row.valid)&&document.documentElement.scrollWidth<=innerWidth+1,rows};
                "#)?;
                    if geometry["valid"] != true {
                        return Err(format!(
                            "status first-row clearance {theme} {width}x{height}: {geometry}"
                        )
                        .into());
                    }
                }
            }
            Ok(())
        },
        || {
            let metrics = browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}));
            let theme =
                accessibility::select_theme(browser, original_theme.as_str().unwrap_or("light"));
            metrics?;
            theme?;
            Ok(())
        },
    )?;
    println!(
        "PASS status cards share first-row text clearance and range popup matches trigger width in both themes at four viewports"
    );
    Ok(())
}
