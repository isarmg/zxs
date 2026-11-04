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
    browser.keys(&browser.css(".zxs-file-search input")?, "\u{e007}")?;
    wait_for("real file query for separator layout", || {
        if browser.script(&format!("const page=document.querySelector('.zxs-file-manager'),nav=page.querySelector('.zxs-file-pagination');return page.getAttribute('aria-busy')==='false'&&(page.querySelectorAll('tbody tr').length===0)==={empty}&&![...nav.querySelectorAll('button')].some(e=>['Retry','重试'].includes(e.textContent.trim()))"))? == true { Ok(()) } else { Err("real file query is pending or failed".into()) }
    })
}
pub(super) fn files(browser: &Browser) -> Result<()> {
    let original_theme = browser.script("return document.documentElement.dataset.zxsTheme")?;
    let previews = env::var_os("ZXS_VIEWPORT_PREVIEWS").map(PathBuf::from);
    if let Some(directory) = &previews {
        fs::create_dir_all(directory)?;
    }
    with_cleanup(
        || {
            for empty in [false, true] {
                search(browser, empty)?;
                for theme in ["light", "dark"] {
                    accessibility::select_theme(browser, theme)?;
                    for (width, height) in VIEWS {
                        viewport(browser, width, height)?;
                        accessibility::input_surfaces(browser)?;
                        file_row_geometry(browser)?;
                        if !empty && let Some(directory) = &previews {
                            let data = browser.command("GET", "/screenshot", None)?;
                            fs::write(
                                directory.join(format!("file-list-{theme}-{width}x{height}.png")),
                                STANDARD
                                    .decode(data.as_str().ok_or("missing file-list screenshot")?)?,
                            )?;
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
        "PASS inset file-row and pagination separators with shared G4 row surfaces, nonempty and empty real results in both themes at four viewports"
    );
    Ok(())
}
// Also called with the five-row synthetic page to cover first, middle and last.
pub(super) fn file_row_geometry(browser: &Browser) -> Result<()> {
    wait_for("inset file separators and shared G4 row surfaces", || {
        let geometry = browser.script(include_str!("file_row_layout.js"))?;
        if geometry["valid"] == true {
            Ok(())
        } else {
            Err(format!("file row decoration: {geometry}").into())
        }
    })
}

pub(super) fn file_row_sizes(browser: &Browser) -> Result<()> {
    with_cleanup(
        || {
            let mut overflow = false;
            for (width, height) in VIEWS {
                viewport(browser, width, height)?;
                for end in [false, true, false] {
                    let metrics = browser.script(&format!("const s=document.querySelector('.zxs-file-list-panel .zxs-table-scroll');const maximum=s.scrollWidth-s.clientWidth;s.scrollLeft={end}?maximum:0;return {{maximum,left:s.scrollLeft}}"))?;
                    overflow |= metrics["maximum"].as_f64().is_some_and(|value| value > 1.0);
                    file_row_geometry(browser)?;
                }
            }
            if !overflow {
                return Err("file-row viewport fixture never exercised horizontal overflow".into());
            }
            file_row_resize(browser)?;
            Ok(())
        },
        || {
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            Ok(())
        },
    )
}

// The pagination fixture removes a genuinely wider nowrap size cell through a
// real page/query update. Measure actual table widths instead of assuming that
// fewer rows necessarily make a native table narrower.
pub(super) fn file_row_shrink_start(browser: &Browser) -> Result<Value> {
    viewport(browser, 320, 568)?;
    let before = browser.script("const s=document.querySelector('.zxs-file-list-panel .zxs-table-scroll');s.scrollLeft=s.scrollWidth-s.clientWidth;return {width:s.querySelector('table').getBoundingClientRect().width,left:s.scrollLeft,client:s.clientWidth,viewport:innerWidth}")?;
    if before["left"].as_f64().is_none_or(|left| left <= 0.0) {
        return Err(
            format!("file-row shrink fixture did not start right-scrolled: {before}").into(),
        );
    }
    file_row_geometry(browser)?;
    Ok(before)
}

pub(super) fn file_row_shrink_finish(browser: &Browser, before: Value, action: &str) -> Result<()> {
    // No scroll assignment here: clearing the old page may naturally move to
    // zero before the shorter result arrives. Both zero and the new right edge
    // are valid, but decoration may never retain the old scrollable width.
    // The page scrollbar may disappear with fewer rows; compare the unchanged
    // outer viewport rather than assuming an unchanged content clientWidth.
    wait_for(
        "page/query shrink naturally clamps horizontal scrolling",
        || {
            let after = browser.script(&format!("const s=document.querySelector('.zxs-file-list-panel .zxs-table-scroll'),width=s.querySelector('table').getBoundingClientRect().width,maximum=Math.max(0,width-s.clientWidth),offset=parseFloat(getComputedStyle(s).getPropertyValue('--zxs-file-scroll-left'));return {{valid:width<{}-1&&innerWidth==={}&&s.scrollWidth<=Math.ceil(width)+1&&s.scrollLeft>=0&&s.scrollLeft<=maximum+.5&&s.scrollLeft<{}-.5&&Math.abs(offset-s.scrollLeft)<.01,width,maximum,left:s.scrollLeft,scroll:s.scrollWidth,client:s.clientWidth,offset}}",before["width"],before["viewport"],before["left"]))?;
            if after["valid"] != true {
                return Err(
                    format!("file-row {action} shrink: before={before}, after={after}").into(),
                );
            }
            file_row_geometry(browser)
        },
    )
}

fn file_row_resize(browser: &Browser) -> Result<()> {
    viewport(browser, 320, 568)?;
    if browser.script("const s=document.querySelector('.zxs-file-list-panel .zxs-table-scroll');s.scrollLeft=s.scrollWidth-s.clientWidth;return s.scrollLeft>0")? != true {
        return Err("file-row resize fixture must start at an overflowing right edge".into());
    }
    file_row_geometry(browser)?;
    // Do not set scrollLeft after expanding. Decoration must not retain phantom
    // overflow, and the browser must clamp the real scroll position by itself.
    viewport(browser, 1440, 900)?;
    wait_for(
        "right-scrolled list expands without retained overflow or blank space",
        || {
            let metrics = browser.script("const s=document.querySelector('.zxs-file-list-panel .zxs-table-scroll'),t=s.querySelector('table');return {valid:Math.abs(s.scrollLeft)<.01&&s.scrollWidth<=s.clientWidth+1&&t.getBoundingClientRect().width<=s.clientWidth+1,left:s.scrollLeft,scroll:s.scrollWidth,client:s.clientWidth}")?;
            if metrics["valid"] != true {
                return Err(format!("file-row resize retained scroll overflow: {metrics}").into());
            }
            file_row_geometry(browser)
        },
    )?;
    browser.cdp(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":320,"height":568,"deviceScaleFactor":2,"mobile":false}),
    )?;
    // Device-pixel ratio does not guarantee fractional CSSOM scroll offsets.
    // First accept native quantization but require exact product synchronization.
    file_row_scroll_samples(browser, 1)?;
    // CSS zoom changes the browser's actual scroll coordinate conversion. At
    // 2x, native 0.5 CSS px is a whole layout pixel, even on integer scroll paths.
    // Never replace scrollLeft or dispatch a synthetic scroll event to get it.
    let saved = browser.script("const style=document.querySelector('.zxs-file-list-panel').style;return {value:style.getPropertyValue('zoom'),priority:style.getPropertyPriority('zoom')}")?;
    with_cleanup(
        || {
            browser.script("document.querySelector('.zxs-file-list-panel').style.setProperty('zoom','2');return true")?;
            file_row_scroll_samples(browser, 2)
        },
        || {
            let restored = browser.script(&format!("const style=document.querySelector('.zxs-file-list-panel').style,value={},priority={};if(value)style.setProperty('zoom',value,priority);else style.removeProperty('zoom');return style.getPropertyValue('zoom')===value&&style.getPropertyPriority('zoom')===priority",saved["value"],saved["priority"]))?;
            if restored != true {
                return Err("file-row zoom fixture did not restore the author's zoom".into());
            }
            file_row_geometry(browser)
        },
    )
}

fn file_row_scroll_samples(browser: &Browser, zoom: u32) -> Result<()> {
    let kind = browser.script("return document.querySelector('.zxs-file-table').classList.contains('zxs-disk-table')?'disks':'files'")?;
    let kind = kind.as_str().ok_or("missing scroll fixture kind")?;
    for requested in [0.5, 1.5, 7.5] {
        browser.script(&format!("document.querySelector('.zxs-file-list-panel .zxs-table-scroll').scrollLeft={requested};return true"))?;
        let mut evidence = Value::Null;
        let result = wait_for("native scroll offset and zoomed row geometry", || {
            let metrics = browser.script(&format!("const s=document.querySelector('.zxs-file-list-panel .zxs-table-scroll'),offset=parseFloat(getComputedStyle(s).getPropertyValue('--zxs-file-scroll-left')),fractional=!Number.isInteger(s.scrollLeft);return {{valid:devicePixelRatio===2&&s.currentCSSZoom==={zoom}&&s.scrollLeft>0&&({zoom}===1||fractional)&&Math.abs(offset-s.scrollLeft)<.01,requested:{requested},left:s.scrollLeft,offset,fractional,dpr:devicePixelRatio,zoom:s.currentCSSZoom}}"))?;
            let geometry = browser.script(include_str!("file_row_layout.js"))?;
            evidence = json!({"kind":kind,"metrics":metrics,"geometry":geometry});
            if evidence["metrics"]["valid"] == true && evidence["geometry"]["valid"] == true {
                Ok(())
            } else {
                Err(format!("file-row native scroll sample: {evidence}").into())
            }
        });
        println!("INFO file-row native scroll sample: {evidence}");
        let saved = (|| -> Result<()> {
            let output = env::var_os("ZXS_VIEWPORT_PREVIEWS")
                .map(PathBuf::from)
                .or_else(|| {
                    result
                        .is_err()
                        .then(|| PathBuf::from("artifacts/browser-smoke/diagnostics"))
                });
            if let Some(directory) = output {
                fs::create_dir_all(&directory)?;
                fs::write(
                    directory.join(format!(
                        "row-scroll-{kind}-zoom-{zoom}-requested-{requested}.json"
                    )),
                    json_codec::to_vec_pretty(&evidence)?,
                )?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(save_error) = saved {
                eprintln!("file-row scroll evidence also failed: {save_error}");
            }
            return Err(error);
        }
        saved?;
    }
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
