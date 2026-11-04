//! Synthetic write delivery and health faults through production WASM owners.
use super::*;

pub(super) fn run(browser: &Browser) -> Result<()> {
    wait_for("loaded file note editor", || {
        if browser.script("return Array.from(document.querySelectorAll('button')).some(e=>['Edit file note','编辑文件备注'].includes(e.getAttribute('aria-label'))&&!e.disabled)")? == true {
            Ok(())
        } else { Err("file note editor is not ready".into()) }
    })?;
    for (storage, transport_failure) in [("unavailable", false), ("recovery_required", true)] {
        browser.script(&format!(r#"
            const original=window.fetch;
            const probe=window.__zxsNoteRecovery={{writes:[],polls:[],storage:'ready',health:[],reject:false,
                transportFailure:{transport_failure},restore:()=>window.fetch=original}};
            const response=value=>new Response(JSON.stringify(value),{{status:200,headers:{{'Content-Type':'application/json'}}}});
            window.fetch=async function(...args){{
                const request=args[0],url=new URL(request.url,location.href);
                if(request.method==='POST' && /^\/api\/file-manager\/files\/[^/]+\/metadata$/.test(url.pathname)){{
                    probe.writes.push(await request.clone().json());
                    if(probe.transportFailure)throw new TypeError('Synthetic lost metadata response');
                    return response({{}});
                }}
                if(/^\/api\/operations\/[^/]+\/outcome$/.test(url.pathname) && probe.writes.length){{
                    const id=decodeURIComponent(url.pathname.split('/')[3]);
                    probe.polls.push(id);
                    if(!probe.reject)return response({{}});
                    return response({{operation_id:id,operation_kind:'file.metadata',request_digest:'a'.repeat(64),
                        state:'in_doubt',commit_state:'ABORTED',result:{{}},error:null,
                        protection_at_publish:'NOT_APPLICABLE',publication_sequence:'0'}});
                }}
                const result=await original.apply(this,args);
                if(url.pathname==='/api/management/status' && result.ok){{
                    const value=await result.json();
                    value.storage.state=probe.storage;
                    value.storage.retryable=true;
                    value.storage.recovering=false;
                    probe.health.push(probe.storage);
                    return response(value);
                }}
                return result;
            }};return true;
        "#))?;
        with_cleanup(
            || {
                let draft = format!("recover exact note: {storage}");
                browser.click(&browser.button("Edit file note", "编辑文件备注")?)?;
                let input = browser.css(".zxs-inline-input")?;
                browser.keys(&input, &draft)?;
                browser.script("window.__zxsNoteRecovery.input=document.querySelector('.zxs-inline-input');return true")?;
                browser.keys(&input, "\u{e007}")?;
                wait_for("unknown metadata receipt keeps its editor", || {
                    if browser.script("const p=window.__zxsNoteRecovery;return p.writes.length===1&&p.input.isConnected&&p.input.disabled&&p.polls.length>0")? == true {
                        Ok(())
                    } else { Err("unknown operation did not retain its input and original poll".into()) }
                })?;
                browser.script(&format!(
                    "window.__zxsNoteRecovery.storage={};return true",
                    json!(storage)
                ))?;
                wait_for(
                    "storage fault preserves the editor and exposes management recovery",
                    || {
                        if browser.script(r#"
                        const p=window.__zxsNoteRecovery;
                        const retry=Array.from(document.querySelectorAll('button')).find(e=>['Retry opening data storage','重试打开数据存储'].includes(e.textContent.trim()));
                        const health=Array.from(document.querySelectorAll('section')).find(e=>['Storage and authentication status','存储与认证状态'].includes(e.getAttribute('aria-label')));
                        const active=Array.from(document.querySelectorAll('[aria-current=page]')).some(e=>['Details','详情'].includes(e.textContent.trim()));
                        return p.health.includes(p.storage)&&p.input.isConnected&&p.input.disabled
                            &&p.input.value===p.writes[0].note&&p.writes.length===1
                            &&active&&health&&health.getBoundingClientRect().height>0&&retry&&!retry.disabled;
                    "#)? == true { Ok(()) }
                    else { Err("automatic health handling lost the note owner or recovery controls".into()) }
                    },
                )?;
                browser.script("const p=window.__zxsNoteRecovery;p.storage='ready';p.health=[];p.reject=true;return true")?;
                wait_for(
                    "original outcome rejection restores editable exact draft after recovery",
                    || {
                        if browser.script(r#"
                        const p=window.__zxsNoteRecovery;
                        return p.health.includes('ready')&&p.input.isConnected&&!p.input.disabled
                            &&p.input.value===p.writes[0].note&&p.writes.length===1&&p.polls.length>0
                            &&p.polls.every(id=>id===p.writes[0].operation_id);
                    "#)? == true { Ok(()) }
                    else { Err("recovery lost text, changed operation identity, or replayed the write".into()) }
                    },
                )?;
                browser.keys(&input, "\u{e00c}")?;
                wait_for("explicit cancellation after rejection", || {
                    if browser.script("return !document.querySelector('.zxs-inline-input')")?
                        == true
                    {
                        Ok(())
                    } else {
                        Err("settled draft did not accept explicit cancellation".into())
                    }
                })?;
                Ok(())
            },
            || {
                browser.script("window.__zxsNoteRecovery.restore();delete window.__zxsNoteRecovery;return true")?;
                Ok(())
            },
        )?;
    }
    println!(
        "PASS synthetic malformed/lost note receipts retain original owner, exact text and operation polling through unavailable/recovery-required health; no second POST"
    );
    Ok(())
}
