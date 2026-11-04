//! Synthetic paged responses through the real WASM and DOM. This deliberately
//! tests browser bounds, focus and request lifetime, not engine scale/ordering.
use super::*;

fn check(browser: &Browser, label: &str, expression: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(expression)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}
fn draft(browser: &Browser, query: &str) -> Result<()> {
    browser.script(&format!("const input=document.querySelector('.zxs-file-search input');input.focus();input.value={};input.dispatchEvent(new Event('input',{{bubbles:true}}));return true",json!(query)))?;
    Ok(())
}
fn search(browser: &Browser, query: &str) -> Result<()> {
    draft(browser, query)?;
    browser.keys(&browser.css(".zxs-file-search input")?, "\u{e007}")
}

// Inspect the actual Chrome user-agent shadow control, then use its real hit
// target. Never replace the clear button or assign the input value in this path.
fn native_clear_node(node: &Value) -> Option<&Value> {
    let clear = node["attributes"].as_array().is_some_and(|attributes| {
        attributes
            .chunks_exact(2)
            .any(|pair| pair[0] == "id" && pair[1] == "search-clear")
    });
    if clear {
        return Some(node);
    }
    ["children", "shadowRoots"]
        .into_iter()
        .filter_map(|key| node[key].as_array())
        .flatten()
        .find_map(native_clear_node)
}

fn native_clear(browser: &Browser) -> Result<()> {
    browser.script("const i=document.querySelector('.zxs-file-search input');i.scrollIntoView({block:'center'});i.focus();return true")?;
    let document = browser.cdp("DOM.getDocument", json!({"depth":0,"pierce":true}))?;
    let input = browser.cdp(
        "DOM.querySelector",
        json!({"nodeId":document["root"]["nodeId"],"selector":".zxs-file-search input"}),
    )?;
    let tree = browser.cdp(
        "DOM.describeNode",
        json!({"nodeId":input["nodeId"],"depth":-1,"pierce":true}),
    )?;
    let clear = native_clear_node(&tree["node"])
        .ok_or("Chrome did not expose the native search clear button")?;
    let bounds = browser.cdp(
        "DOM.getBoxModel",
        json!({"backendNodeId":clear["backendNodeId"]}),
    )?;
    let quad = bounds["model"]["content"]
        .as_array()
        .ok_or("native search clear button has no content quad")?;
    if quad.len() != 8 {
        return Err("native search clear button has an invalid content quad".into());
    }
    let coordinate = |index: usize| quad[index].as_f64().ok_or("invalid clear coordinate");
    let x = (coordinate(0)? + coordinate(4)?) / 2.0;
    let y = (coordinate(1)? + coordinate(5)?) / 2.0;
    for kind in ["mousePressed", "mouseReleased"] {
        browser.cdp(
            "Input.dispatchMouseEvent",
            json!({"type":kind,"button":"left","clickCount":1,"x":x,"y":y}),
        )?;
    }
    Ok(())
}

fn cleared_draft_stays_local(browser: &Browser) -> Result<()> {
    // Observe beyond the former 250ms debounce window; checking only the first
    // frame would miss a native-clear/search-event path that still submits later.
    let unchanged = browser.command(
        "POST",
        "/execute/async",
        Some(json!({"script":"const done=arguments[arguments.length-1];setTimeout(()=>{const i=document.querySelector('.zxs-file-search input'),s=window.__zxsPaging;done(i.value===''&&i.matches(':placeholder-shown')&&document.activeElement===i&&s.calls.length===s.clearCalls&&s.calls.at(-1).query==='paged-204'&&document.querySelectorAll('.zxs-file-table tbody tr').length===1&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-204.txt')},350)","args":[]})),
    )?;
    if unchanged != true {
        return Err("clearing changed the listing or sent a request before Enter".into());
    }
    Ok(())
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(r#"
        const original=window.fetch;
        const state=window.__zxsPaging={calls:[],pending:[],aborted:0,delivered:0,template:null,restore:()=>{window.fetch=original;for(const p of state.pending)p.release();}};
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            if(!/^\/api\/file-manager\/disks\/[^/]+\/entries$/.test(url.pathname))return original.apply(this,args);
            const query=url.searchParams.get('query')||'',cursor=url.searchParams.get('cursor');
            state.calls.push({query,cursor,limit:url.searchParams.get('limit')});
            if(state.staleNext&&cursor==='synthetic-next'){state.staleNext=false;return new Response(JSON.stringify({code:'LISTING_CHANGED',message:'Synthetic folder changed',retryable:false}),{status:409,headers:{'Content-Type':'application/json'}});}
            if(!state.template){const response=await original.apply(this,args);if(!response.ok)throw new Error('real listing seed failed');state.template=await response.json();}
            const template=state.template,base=template.entries[0],directory=url.searchParams.get('path');
            if(!base||url.searchParams.get('limit')!=='200')throw new Error('invalid pagination fixture request');
            const join=(parent,name)=>parent?parent+'/'+name:name;
            // One synthetic 999 PiB size label widens only the 200-entry page. Navigation/search
            // remove it through normal product rendering, without injected CSS.
            const entry=(name,id)=>({...base,entry_id:id,name,path:join(directory,name),repository_path:join(template.context.repository_path,name),size:id==='fixture-199'?'1124774006935781376':base.size});
            const all=Array.from({length:205},(_,i)=>entry('paged-'+String(i).padStart(3,'0')+'.txt','fixture-'+i));
            const found=query.startsWith('slow')?[entry('stale-response.txt','fixture-0')]:all.filter(e=>e.name.includes(query));
            const offset=cursor==='synthetic-next'?200:0;
            const body={...template,entries:found.slice(offset,offset+200),snapshot:'synthetic-snapshot',total_entries:String(found.length),directory_file_count:String(all.length),previous_cursor:offset?'synthetic-first':null,next_cursor:found.length>offset+200?'synthetic-next':null,selected_entry:null};
            if(query.startsWith('slow')){
                let release;const gate=new Promise(resolve=>release=resolve);
                const pending={release,signal:request.signal};state.pending.push(pending);
                request.signal.addEventListener('abort',()=>{state.aborted++;},{once:true});
                // Intentionally deliver even after abort to exercise the generation fence.
                await gate;state.delivered++;
            }
            return new Response(JSON.stringify(body),{status:200,headers:{'Content-Type':'application/json'}});
        };return true;
    "#)?;
    with_cleanup(
        || {
            browser.click(&browser.button("Refresh", "刷新")?)?;
            check(
                browser,
                "bounded first page",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===200&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-000.txt'",
            )?;
            browser.click(&browser.css(".zxs-file-table thead input[type=checkbox]")?)?;
            check(
                browser,
                "select all is page-local",
                "return document.querySelectorAll('.zxs-entry-checkbox:checked').length===200",
            )?;
            browser.script(
                "window.__zxsPaging.draftCalls=window.__zxsPaging.calls.length;return true",
            )?;
            let input = browser.css(".zxs-file-search input")?;
            browser.keys(&input, "paged-204")?;
            let unchanged = browser.command(
                "POST",
                "/execute/async",
                Some(json!({"script":"const done=arguments[arguments.length-1];setTimeout(()=>done(window.__zxsPaging.calls.length===window.__zxsPaging.draftCalls&&document.querySelectorAll('.zxs-entry-checkbox:checked').length===200&&document.querySelectorAll('.zxs-file-table tbody tr').length===200),350)","args":[]})),
            )?;
            if unchanged != true {
                return Err("typing submitted a query or changed the loaded selection".into());
            }
            browser.click(&browser.button("Refresh", "刷新")?)?;
            check(
                browser,
                "refresh uses the committed query without replacing draft text",
                "return window.__zxsPaging.calls.at(-1).query===''&&document.querySelectorAll('.zxs-file-table tbody tr').length===200&&document.querySelector('.zxs-file-search input').value==='paged-204'&&!document.querySelector('[aria-busy=true]')",
            )?;
            let before_page = page_layout::file_row_shrink_start(browser)?;
            let next = browser.button("Next page", "下一页")?;
            browser.click(&next)?;
            check(
                browser,
                "next page replaces rows and clears checks",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===5&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-200.txt'&&document.querySelectorAll('.zxs-entry-checkbox:checked').length===0",
            )?;
            check(
                browser,
                "pagination uses the committed query and preserves its draft",
                "return window.__zxsPaging.calls.at(-1).query===''&&document.querySelector('.zxs-file-search input').value==='paged-204'",
            )?;
            page_layout::file_row_shrink_finish(browser, before_page, "pagination")?;
            // Synthetic response, real product DOM: first/middle/last rows at
            // desktop and narrow widths, with no cloned or restyled elements.
            page_layout::file_row_sizes(browser)?;
            file_row_visual::run(browser, &[0, 2, 4], "file")?;
            let previous = browser.button("Previous page", "上一页")?;
            browser.click(&previous)?;
            check(
                browser,
                "previous cursor restores first page",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===200&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-000.txt'",
            )?;
            browser.script("window.__zxsPaging.staleNext=true;return true")?;
            browser.click(&browser.button("Next page", "下一页")?)?;
            check(
                browser,
                "changed snapshot restarts the first page",
                "return !window.__zxsPaging.staleNext&&window.__zxsPaging.calls.at(-1).cursor===null&&document.querySelectorAll('.zxs-file-table tbody tr').length===200&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-000.txt'",
            )?;
            let before_query = page_layout::file_row_shrink_start(browser)?;
            search(browser, "paged-204")?;
            check(
                browser,
                "search finds an entry outside the loaded page",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===1&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-204.txt'&&window.__zxsPaging.calls.at(-1).cursor===null&&document.activeElement===document.querySelector('.zxs-file-search input')",
            )?;
            page_layout::file_row_shrink_finish(browser, before_query, "search")?;
            for native in [false, true] {
                if native {
                    search(browser, "paged-204")?;
                    check(
                        browser,
                        "restore the submitted filter before testing native clear",
                        "return document.querySelector('.zxs-file-search input').value==='paged-204'&&document.querySelectorAll('.zxs-file-table tbody tr').length===1&&!document.querySelector('[aria-busy=true]')",
                    )?;
                }
                browser.script(
                    "window.__zxsPaging.clearCalls=window.__zxsPaging.calls.length;return true",
                )?;
                if native {
                    native_clear(browser)?;
                } else {
                    browser.keys(&input, "\u{e009}a\u{e000}\u{e003}")?;
                }
                cleared_draft_stays_local(browser)?;
            }
            browser.keys(&input, "\u{e007}")?;
            check(
                browser,
                "Enter on an empty draft restores page one",
                "return window.__zxsPaging.calls.at(-1).query===''&&window.__zxsPaging.calls.at(-1).cursor===null&&document.querySelectorAll('.zxs-file-table tbody tr').length===200&&document.activeElement===document.querySelector('.zxs-file-search input')",
            )?;
            search(browser, "slow-query")?;
            check(
                browser,
                "first delayed query is in flight",
                "return window.__zxsPaging.pending.length===1",
            )?;
            draft(browser, "paged-001")?;
            check(
                browser,
                "editing does not abort the already submitted query",
                "return window.__zxsPaging.aborted===0&&window.__zxsPaging.calls.at(-1).query==='slow-query'&&document.querySelector('.zxs-file-search input').value==='paged-001'",
            )?;
            browser.keys(&input, "\u{e007}")?;
            check(
                browser,
                "superseded fetch signal aborts and replacement renders",
                "return window.__zxsPaging.aborted===1&&document.querySelector('.zxs-file-name')?.textContent.trim()==='paged-001.txt'",
            )?;
            browser.script("window.__zxsPaging.pending[0].release();return true")?;
            check(
                browser,
                "late response delivered",
                "return window.__zxsPaging.delivered===1",
            )?;
            check(
                browser,
                "late response cannot overwrite current query",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===1&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-001.txt'",
            )?;
            search(browser, "slow-navigation")?;
            check(
                browser,
                "navigation query is in flight",
                "return window.__zxsPaging.pending.length===2",
            )?;
            draft(browser, "unsubmitted-navigation")?;
            browser.click(&browser.button("All disks", "所有硬盘")?)?;
            check(
                browser,
                "leaving directory aborts fetch",
                "return window.__zxsPaging.aborted===2&&document.querySelectorAll('.zxs-disk-entry').length===3",
            )?;
            browser.script("window.__zxsPaging.pending[1].release();return true")?;
            check(
                browser,
                "navigation response delivered",
                "return window.__zxsPaging.delivered===2",
            )?;
            check(
                browser,
                "late response cannot repopulate disk root",
                "return document.querySelectorAll('.zxs-disk-entry').length===3&&!document.querySelector('.zxs-entry-checkbox')",
            )?;
            Ok(())
        },
        || {
            let restore = browser.script("if(window.__zxsPaging){window.__zxsPaging.restore();delete window.__zxsPaging;}return true");
            let metrics = browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}));
            restore?;
            metrics?;
            Ok(())
        },
    )?;
    // The real disk-directory rows have no pagination footer.
    page_layout::file_row_sizes(browser)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    open_fixture_directory(browser)?;
    check(
        browser,
        "directory navigation clears committed and unsubmitted search text",
        "const i=document.querySelector('.zxs-file-search input');return i.value===''&&i.matches(':placeholder-shown')&&document.querySelector('.zxs-file-name').textContent.trim()==='browser-smoke.txt'",
    )?;
    println!(
        "PASS synthetic 205-entry pagination DOM bounds, page selection, global-query request, AbortController and late-query/navigation fences (not engine-scale evidence)"
    );
    Ok(())
}
