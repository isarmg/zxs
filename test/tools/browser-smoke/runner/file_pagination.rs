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
fn search(browser: &Browser, query: &str) -> Result<()> {
    browser.script(&format!("const input=document.querySelector('.zxs-file-search input');input.focus();input.value={};input.dispatchEvent(new Event('input',{{bubbles:true}}));return true",json!(query)))?;
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
            const entry=(name,id)=>({...base,entry_id:id,name,path:join(directory,name),repository_path:join(template.context.repository_path,name)});
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
            let next = browser.button("Next page", "下一页")?;
            browser.click(&next)?;
            check(
                browser,
                "next page replaces rows and clears checks",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===5&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-200.txt'&&document.querySelectorAll('.zxs-entry-checkbox:checked').length===0",
            )?;
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
            search(browser, "paged-204")?;
            check(
                browser,
                "search finds an entry outside the loaded page",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===1&&document.querySelector('.zxs-file-name').textContent.trim()==='paged-204.txt'&&window.__zxsPaging.calls.at(-1).cursor===null&&document.activeElement===document.querySelector('.zxs-file-search input')",
            )?;
            search(browser, "slow-query")?;
            check(
                browser,
                "first delayed query is in flight",
                "return window.__zxsPaging.pending.length===1",
            )?;
            search(browser, "paged-001")?;
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
            browser.script("if(window.__zxsPaging){window.__zxsPaging.restore();delete window.__zxsPaging;}return true")?;
            Ok(())
        },
    )?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    open_fixture_directory(browser)?;
    println!(
        "PASS synthetic 205-entry pagination DOM bounds, page selection, global-query request, AbortController and late-query/navigation fences (not engine-scale evidence)"
    );
    Ok(())
}
