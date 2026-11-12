//! Real-browser assertions for file browsing and metadata management.
use super::*;

pub(super) fn open_fixture_directory(browser: &Browser) -> Result<()> {
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-fixtures']",
    )?)?;
    browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?;
    Ok(())
}

pub(super) fn assert_metadata_only(browser: &Browser) -> Result<()> {
    let forbidden = browser.script(r#"
        const labels = ['Upload files','上传文件','Write to current branch','写入当前分支','Download','下载','Preview','预览','Transfers','传输任务'];
        return { controls: Array.from(document.querySelectorAll('button,a')).filter(e => labels.includes(e.textContent.trim()) || labels.includes(e.getAttribute('aria-label'))).map(e=>e.textContent),
            surfaces: document.querySelectorAll('input[type=file],a[download],.zxs-transfers-page,.zxs-transfer-card,.zxs-preview-dialog,.zxs-file-preview').length };
    "#)?;
    if forbidden["controls"] != json!([]) || forbidden["surfaces"] != 0 {
        return Err(format!("removed Web file-transfer/preview UI remains: {forbidden}").into());
    }
    Ok(())
}

/// Only response delivery is synthetic; no backup configuration writes reach the server.
pub(super) fn backup_configuration_read(browser: &Browser) -> Result<()> {
    browser.script(r#"
        if(document.querySelector('.zxs-backup-review,.zxs-error-center'))
            throw new Error('backup probe requires an idle panel and clean error center');
        const original=window.fetch;
        const p=window.__zxsBackupRead={mode:'valid',seed:null,url:null,slot:null,
            delivered:0,deliveredMode:null,reads:0,writes:0,prefix:'browser-probe-prefix',
            restore:()=>window.fetch=original};
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            const method=request.method||args[1]?.method||'GET';
            if(url.origin!==location.origin||!/^\/api\/repositories\/[^/]+\/backup-configuration$/.test(url.pathname))
                return original.apply(this,args);
            if(method!=='GET'){
                p.writes++;
                return new Response(JSON.stringify({error:{code:'INVALID_REQUEST',message:'Synthetic blocked backup write',retryable:false}}),
                    {status:409,headers:{'Content-Type':'application/json'}});
            }
            if(p.url&&p.url!==url.href)throw new Error('backup probe scope changed');
            const sequence=++p.reads,mode=p.mode;
            if(!p.seed){
                const response=await original.apply(this,args);
                if(!response.ok)throw new Error('real backup configuration seed failed');
                p.seed=await response.json();p.url=url.href;
                const seed=p.seed;
                if(!Number.isInteger(seed.target_copies)||seed.target_copies<2||seed.max_copies<seed.target_copies
                    ||!Array.isArray(seed.backup_routes)||seed.backup_routes.length===0
                    ||seed.backup_routes.some(route=>!Array.isArray(route)||route.length<seed.target_copies)
                    ||!seed.backup_prefixes||Array.isArray(seed.backup_prefixes)||typeof seed.backup_prefixes!=='object')
                    throw new Error('real backup seed must contain a usable selected backup slot');
                p.slot=seed.backup_routes[0][1];
                if(typeof p.slot!=='string'||!p.slot)throw new Error('real backup seed has no target slot');
            }
            const value=JSON.parse(JSON.stringify(p.seed));
            value.backup_prefixes[p.slot]=p.prefix;
            if(mode==='missing')delete value.backup_prefixes;
            else if(mode==='non-string')value.backup_prefixes[p.slot]=42;
            else if(mode!=='valid')throw new Error('unknown backup probe mode');
            p.delivered=sequence;p.deliveredMode=mode;
            return new Response(JSON.stringify(value),{status:200,headers:{'Content-Type':'application/json'}});
        };return true;
    "#)?;
    with_cleanup(
        || {
            for (index, mode) in ["valid", "missing", "non-string"].into_iter().enumerate() {
                browser.script(&format!(
                    "window.__zxsBackupRead.mode={};return true",
                    json!(mode)
                ))?;
                browser.click(&browser.button("Backup settings", "备份设置")?)?;
                wait_for(
                    &format!("backup configuration {mode} read controls"),
                    || {
                        let result = browser.script(&format!(r#"
                        const expected={},mode={},p=window.__zxsBackupRead,panel=document.querySelector('.zxs-backup-review');
                        if(p.delivered!==expected||p.deliveredMode!==mode||!panel||p.writes!==0)return false;
                        const save=Array.from(panel.querySelectorAll('button')).find(button=>['Save backup settings','保存备份设置'].includes(button.textContent.trim()));
                        const error=panel.querySelector('.zxs-inline-error').textContent;
                        if(mode!=='valid')return error.includes('INVALID_RESPONSE')&&(!save||save.disabled);
                        const input=Array.from(panel.querySelectorAll('input')).find(input=>input.getAttribute('aria-label')?.startsWith(p.slot+' '));
                        const copies=panel.querySelector('[role=combobox]');
                        return save&&!save.disabled&&input&&!input.disabled&&input.value===p.prefix
                            &&copies&&!copies.disabled&&copies.textContent.trim().startsWith(String(p.seed.target_copies))&&!error;
                    "#, index + 1, json!(mode)))?;
                        if result == true {
                            Ok(())
                        } else {
                            Err(format!("backup {mode} controls not settled").into())
                        }
                    },
                )?;
                browser.click(&browser.element("xpath", "//section[contains(@class,'zxs-backup-review')]//button[normalize-space(.)='Close' or normalize-space(.)='关闭']")?)?;
                wait_for("closed backup probe panel", || {
                    if browser.script("return !document.querySelector('.zxs-backup-review')")?
                        == true
                    {
                        Ok(())
                    } else {
                        Err("backup panel did not close".into())
                    }
                })?;
            }
            if browser.script(
                "return window.__zxsBackupRead.reads===3&&window.__zxsBackupRead.writes===0",
            )? != true
            {
                return Err(
                    "backup probe did not perform exactly three reads and zero writes".into(),
                );
            }
            Ok(())
        },
        || {
            browser.script(r#"
                if(window.__zxsBackupRead){window.__zxsBackupRead.restore();delete window.__zxsBackupRead;}
                const panel=document.querySelector('.zxs-backup-review');
                if(panel){
                    const close=Array.from(panel.querySelectorAll('button')).find(button=>['Close','关闭'].includes(button.textContent.trim()));
                    if(!close||close.disabled)throw new Error('backup probe panel cannot close');
                    close.click();
                }
                return true;
            "#)?;
            for _ in 0..5 {
                let count = browser
                    .script(
                        "return document.querySelector('.zxs-error-center')?.children.length||0",
                    )?
                    .as_u64()
                    .ok_or("invalid backup probe notice count")?;
                if count == 0 {
                    break;
                }
                browser.click(&browser.css(".zxs-error-center button")?)?;
                wait_for("dismissed backup probe notice", || {
                    if browser.script(&format!("return (document.querySelector('.zxs-error-center')?.children.length||0)<{count}"))? == true { Ok(()) }
                    else { Err("backup probe notice did not dismiss".into()) }
                })?;
            }
            if browser.script("return !document.querySelector('.zxs-backup-review,.zxs-error-center')&&!window.__zxsBackupRead")? != true {
                return Err("backup probe cleanup left UI state".into());
            }
            Ok(())
        },
    )?;
    println!(
        "PASS synthetic backup configuration missing/non-string prefixes fail closed in production WASM; real scope and target slots, valid prefix display, zero writes"
    );
    Ok(())
}

fn disk_note_action(browser: &Browser, english: &str, chinese: &str) -> Result<()> {
    browser.click(&browser.element("xpath", &format!(
        "//section[contains(@class,'zxs-disk-management-card')][@data-slot='P01']//button[normalize-space(.)='{english}' or normalize-space(.)='{chinese}']"
    ))?)
}

fn disk_note_check(browser: &Browser, label: &str, script: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}

/// Notes are written only to the runner's fresh private management fixture.
/// Hardware unavailability is synthetic; identity, CAS and persisted note reads are real.
pub(super) fn disk_notes(browser: &Browser) -> Result<()> {
    let note = format!(
        "browser disk note\n<b data-disk-note-probe>literal <&> text</b>\n{}",
        "多行备注".repeat(40)
    );
    browser.script(&format!(
        "window.__zxsDiskNoteText={};return true",
        json!(note)
    ))?;
    browser.script(r#"
        const original=window.fetch;
        const p=window.__zxsDiskNotes={seed:null,reads:[],writes:[],results:[],pending:[],unexpected:0,
            readRequest:null,noteRequest:null,restore:()=>window.fetch=original};
        p.cleanup=async()=>{
            window.fetch=original;
            if(!p.seed||p.writes.length===0)return;
            const settled=await Promise.allSettled(p.pending);
            if(settled.some(result=>result.status!=='fulfilled'))
                throw new Error('unconfirmed fixture note write prevents verified cleanup');
            const controller=new AbortController(),timer=setTimeout(()=>controller.abort(),10000);
            try{
                const read=()=>original(new Request(p.readRequest.clone(),{signal:controller.signal}));
                const response=await read();
                if(!response.ok)throw new Error('fixture note cleanup read failed');
                const current=await response.json();
                if(current.disk_id!==p.seed.disk_id||current.slot_id!=='P01')
                    throw new Error('fixture disk identity changed during cleanup');
                if(current.note!==p.seed.note){
                    if(current.note!==window.__zxsDiskNoteText)throw new Error('cleanup refuses an unrelated note');
                    const body={expected_disk_id:p.seed.disk_id,expected_note:current.note,note:p.seed.note};
                    const restored=await original(new Request(p.noteRequest.clone(),{body:JSON.stringify(body),signal:controller.signal}));
                    if(!restored.ok)throw new Error('fixture note restoration failed');
                    const verified=await read();
                    if(!verified.ok)throw new Error('fixture note restoration verification failed');
                    const saved=await verified.json();
                    if(saved.disk_id!==p.seed.disk_id||saved.note!==p.seed.note)
                        throw new Error('fixture note was not restored');
                }
            }finally{clearTimeout(timer);}
        };
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            const method=request.method||args[1]?.method||'GET';
            if(url.origin!==location.origin)return original.apply(this,args);
            if(method!=='GET'&&method!=='HEAD'){
                const body=method==='PUT'&&url.pathname==='/api/admin/disks/P01/note'?await request.clone().json():null;
                const index=p.writes.length;
                if(!p.seed||!body||index>1||body.expected_disk_id!==p.seed.disk_id
                    ||body.expected_note!==(index===0?'':window.__zxsDiskNoteText)
                    ||body.note!==(index===0?window.__zxsDiskNoteText:'')){
                    p.unexpected++;
                    return new Response(JSON.stringify({error:{code:'INVALID_REQUEST',message:'Synthetic rejected unexpected fixture write',retryable:false}}),
                        {status:409,headers:{'Content-Type':'application/json'}});
                }
                p.writes.push(body);
                p.noteRequest=request.clone();
                const pending=original.apply(this,args);p.pending.push(pending);
                const response=await pending;
                p.results.push(response.status);
                return response;
            }
            const response=await original.apply(this,args);
            if(method!=='GET'||url.pathname!=='/api/admin/disks/P01/details'||!response.ok)return response;
            const value=await response.json();
            if(value.slot_id!=='P01'||typeof value.disk_id!=='string'||!value.disk_id)
                throw new Error('disk-note probe needs a real registered P01 identity');
            if(!p.seed){
                if(value.note!=='')throw new Error('disk-note probe refuses nonempty preexisting fixture notes');
                p.seed={...value};
                p.readRequest=request.clone();
            }
            if(value.disk_id!==p.seed.disk_id)throw new Error('registered fixture disk changed');
            p.reads.push({disk_id:value.disk_id,note:value.note});
            // Never change the registered identity or note returned by the server.
            for(const field of ['model','serial','capacity_bytes','device_path','filesystem_uuid'])value[field]=null;
            value.mapping='unknown';
            value.health={state:'unavailable',reason:'mapping_unknown'};
            return new Response(JSON.stringify(value),{status:200,headers:{'Content-Type':'application/json'}});
        };return true;
    "#)?;
    with_cleanup(
        || {
            AdminPage::Storage.open(browser)?;
            disk_note_check(
                browser,
                "real disk card remains editable with unknown hardware",
                r#"
                const p=window.__zxsDiskNotes,card=document.querySelector('.zxs-disk-management-card[data-slot="P01"]');
                if(!p.seed||p.reads.length===0||!card)return false;
                const edit=Array.from(card.querySelectorAll('button')).find(button=>['Edit note','编辑备注'].includes(button.textContent.trim()));
                return edit&&!edit.disabled&&/Hardware identity is unavailable|硬件身份暂不可用/.test(card.textContent)
                    &&card.textContent.includes(p.seed.disk_id)&&/Filesystem UUID|文件系统 UUID/.test(card.textContent);
            "#,
            )?;
            disk_note_action(browser, "Edit note", "编辑备注")?;
            browser.keys(
                &browser
                    .css(".zxs-disk-management-card[data-slot='P01'] textarea:not([disabled])")?,
                &note,
            )?;
            disk_note_check(
                browser,
                "multiline disk note keeps exact raw text",
                "return document.querySelector('.zxs-disk-management-card[data-slot=\"P01\"] textarea')?.value===window.__zxsDiskNoteText",
            )?;
            disk_note_action(browser, "Save note", "保存备注")?;
            disk_note_check(
                browser,
                "real note save renders text rather than HTML",
                r#"
                const p=window.__zxsDiskNotes,card=document.querySelector('.zxs-disk-management-card[data-slot="P01"]');
                return p.writes.length===1&&p.results[0]===200&&p.unexpected===0
                    &&!card.querySelector('textarea')&&card.querySelector('.zxs-disk-note')?.textContent===window.__zxsDiskNoteText
                    &&!document.querySelector('[data-disk-note-probe]');
            "#,
            )?;
            let reads = browser.script("return window.__zxsDiskNotes.reads.length")?;
            browser.click(&browser.button("Files", "文件")?)?;
            AdminPage::Storage.open(browser)?;
            disk_note_check(
                browser,
                "new request after reentry reads persisted disk note",
                &format!(
                    r#"
                const p=window.__zxsDiskNotes,card=document.querySelector('.zxs-disk-management-card[data-slot="P01"]');
                return p.reads.length>{reads}&&p.reads.at(-1).note===window.__zxsDiskNoteText
                    &&card.querySelector('.zxs-disk-note')?.textContent===window.__zxsDiskNoteText
                    &&!document.querySelector('[data-disk-note-probe]');
            "#
                ),
            )?;
            disk_note_action(browser, "Edit note", "编辑备注")?;
            browser.keys(
                &browser.css(".zxs-disk-management-card[data-slot='P01'] textarea")?,
                "\nunsaved cancellation",
            )?;
            disk_note_action(browser, "Cancel", "取消")?;
            disk_note_check(
                browser,
                "cancelling a disk note does not write",
                r#"
                const p=window.__zxsDiskNotes,card=document.querySelector('.zxs-disk-management-card[data-slot="P01"]');
                return p.writes.length===1&&p.unexpected===0&&!card.querySelector('textarea')
                    &&card.querySelector('.zxs-disk-note')?.textContent===window.__zxsDiskNoteText;
            "#,
            )?;
            disk_note_action(browser, "Edit note", "编辑备注")?;
            let input = browser.css(".zxs-disk-management-card[data-slot='P01'] textarea")?;
            browser.keys(&input, "\u{e009}a\u{e000}\u{e003}")?;
            disk_note_check(
                browser,
                "empty disk-note input",
                "return document.querySelector('.zxs-disk-management-card[data-slot=\"P01\"] textarea')?.value===''",
            )?;
            disk_note_action(browser, "Save note", "保存备注")?;
            disk_note_check(
                browser,
                "real disk note clear completes",
                r#"
                const p=window.__zxsDiskNotes,card=document.querySelector('.zxs-disk-management-card[data-slot="P01"]');
                return p.writes.length===2&&p.results[1]===200&&p.unexpected===0&&!card.querySelector('textarea')
                    &&['No note','未添加备注'].includes(card.querySelector('.zxs-disk-note')?.textContent);
            "#,
            )?;
            let reads = browser.script("return window.__zxsDiskNotes.reads.length")?;
            disk_note_action(browser, "Refresh disk information", "刷新硬盘信息")?;
            disk_note_check(
                browser,
                "fresh server read confirms persisted empty note",
                &format!(
                    "const p=window.__zxsDiskNotes;return p.reads.length>{reads}&&p.reads.at(-1).note===''&&p.writes.length===2&&p.unexpected===0"
                ),
            )?;
            Ok(())
        },
        || {
            let restored = browser.command(
                "POST",
                "/execute/async",
                Some(json!({"script":r#"
                const done=arguments[arguments.length-1],p=window.__zxsDiskNotes;
                (async()=>{
                    try{if(p)await p.cleanup();return {ok:true};}
                    catch(error){return {ok:false,error:String(error)};}
                    finally{
                        if(p)p.restore();
                        delete window.__zxsDiskNotes;delete window.__zxsDiskNoteText;
                    }
                })().then(done);
            "#,"args":[]})),
            )?;
            browser.script(r#"
                const card=document.querySelector('.zxs-disk-management-card[data-slot="P01"]');
                const cancel=card&&Array.from(card.querySelectorAll('button')).find(button=>['Cancel','取消'].includes(button.textContent.trim()));
                if(cancel&&!cancel.disabled)cancel.click();
                return true;
            "#)?;
            browser.click(&browser.button("Files", "文件")?)?;
            if restored["ok"] != true {
                return Err(format!("disk-note fixture cleanup failed: {restored}").into());
            }
            Ok(())
        },
    )?;
    println!(
        "PASS real temporary management-disk notes: multiline literal text, save and fresh read after reentry, cancel without write, persisted clear; unknown hardware observation is synthetic"
    );
    Ok(())
}

pub(super) fn metadata_edit(browser: &Browser) -> Result<()> {
    // Keep mutations and the selected-entry listing real. Only widen the GET
    // history fixture, so horizontal position is meaningful at desktop widths.
    browser.script(r#"
        const original=window.fetch;
        const probe=window.__zxsDetailRefresh={historyResponses:0,savedListingSeen:false,restore:()=>window.fetch=original};
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            const response=await original.apply(this,args);
            if(!response.ok)return response;
            if(/^\/api\/file-manager\/disks\/[^/]+\/entries$/.test(url.pathname)){
                const listing=await response.clone().json();
                if(listing.selected_entry?.note==='browser metadata note')probe.savedListingSeen=true;
            }
            if(!/^\/api\/file-manager\/files\/[^/]+\/history$/.test(url.pathname))return response;
            const history=await response.json();
            const base=history.nodes.find(node=>node.version_id&&node.node_id);
            if(!base)throw new Error('real node required for detail refresh fixture');
            // Keep the synthetic row wider than the desktop even when all
            // native disclosures are closed and their summaries are 10ch.
            const tips=Array.from({length:24},(_,index)=>({...base,
                node_id:'fmeta_'+(0x1000+index).toString(16).padStart(32,'0'),
                sequence:String(1000000-index),current:false,kind:'tags',note:'',
                parents:{state:'complete'}}));
            history.nodes.push(...tips);
            history.nodes.sort((a,b)=>BigInt(a.sequence)>BigInt(b.sequence)?-1:BigInt(a.sequence)<BigInt(b.sequence)?1:a.node_id.localeCompare(b.node_id));
            history.edges.push(...tips.map(node=>({node_id:node.node_id,parent_node_id:base.node_id,endpoint:'in_window'})));
            if(history.nodes.length>Number(history.identity.limit))throw new Error('wide fixture exceeds the real requested node budget');
            probe.historyResponses++;
            return new Response(JSON.stringify(history),{status:200,headers:{'Content-Type':'application/json'}});
        };return true;
    "#)?;
    with_cleanup(
        || {
            browser.click(&browser.button("Refresh", "刷新")?)?;
            wait_for("wide detail history fixture", || {
                if browser.script("const tree=document.querySelector('.zxs-version-tree-scroll');const note=Array.from(document.querySelectorAll('button')).find(e=>['Edit file note','编辑文件备注'].includes(e.getAttribute('aria-label')));return window.__zxsDetailRefresh.historyResponses>0&&document.querySelectorAll('.zxs-version-node').length>=14&&tree.scrollWidth-tree.clientWidth>100&&note&&!note.disabled")? == true { Ok(()) }
                else { Err("wide detail history fixture not ready".into()) }
            })?;
            tree_frames::run(browser)?;
            if browser.script(r#"
                const probe=window.__zxsDetailRefresh,tree=document.querySelector('.zxs-version-tree-scroll');
                tree.scrollLeft=tree.scrollWidth-tree.clientWidth;
                probe.anchor={tree,canvas:tree.querySelector('.zxs-version-tree-canvas'),
                    node:tree.querySelector('.zxs-version-node'),control:tree.querySelector('.zxs-version-note-action'),
                    left:tree.scrollLeft,histories:probe.historyResponses};
                return probe.anchor.left>100;
            "#)? != true {
                return Err("detail history fixture could not scroll horizontally".into());
            }
            browser.click(&browser.button("Edit file note", "编辑文件备注")?)?;
            let input = browser.css(".zxs-inline-input")?;
            browser.keys(&input, "browser metadata note")?;
            browser.keys(&input, "\u{e007}")?;
            wait_for(
                "file note transaction and selected-entry refresh preserve detail controls",
                || {
                    if browser.script(r#"
                    const probe=window.__zxsDetailRefresh,a=probe.anchor;
                    const note=Array.from(document.querySelectorAll('button')).find(e=>['Edit file note','编辑文件备注'].includes(e.getAttribute('aria-label')));
                    return probe.savedListingSeen&&probe.historyResponses>a.histories&&note&&!note.disabled
                        &&!document.querySelector('.zxs-inline-input')&&note.textContent.includes('browser metadata note')
                        &&a.tree===document.querySelector('.zxs-version-tree-scroll')&&a.canvas.isConnected
                        &&a.node.isConnected&&a.control.isConnected
                        &&a.control===a.node.querySelector('.zxs-version-note-action')
                        &&Math.abs(a.tree.scrollLeft-a.left)<=1;
                "#)? == true { Ok(()) }
                else { Err("metadata refresh replaced detail controls or reset history scrolling".into()) }
                },
            )?;
            Ok(())
        },
        || {
            browser.script("if(window.__zxsDetailRefresh){window.__zxsDetailRefresh.restore();delete window.__zxsDetailRefresh;}return true")?;
            Ok(())
        },
    )?;
    // Navigate away and reload the real metadata from the server.
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    wait_for("persisted file note and version tree", || {
        if browser.script("return document.querySelectorAll('.zxs-version-node').length >= 2 && document.querySelector('.zxs-entry-content').textContent.includes('browser metadata note')")? == true { Ok(()) }
        else { Err("persisted note or seeded version tree missing".into()) }
    })?;
    println!(
        "PASS real metadata save/listing refresh preserves detail controls and wide-tree scroll; persisted note survives navigation (wide history is synthetic)"
    );
    Ok(())
}

/// Limit this assertion to the current service, not browser assets or other origins.
pub(super) fn is_file_body_request(url: &str, origin: &str) -> bool {
    let Some(path) = url.strip_prefix(origin) else {
        return false;
    };
    let path = path.split(['?', '#']).next().unwrap_or_default();
    path.starts_with("/api/") && (path.ends_with("/content") || path.ends_with("/write-sessions"))
}

/// The retained directory read context must not expose the retired write controls.
pub(super) fn directory_controls_retired(browser: &Browser) -> Result<()> {
    browser.click(&browser.css("[data-zxs-current-directory-details]:not([disabled])")?)?;
    browser.css("[data-zxs-current-directory-controls]")?;
    browser.css("[data-directory-git-initialized='true']")?;
    let controls = browser.script(r#"
        return document.querySelectorAll(
            '[data-initialize-directory-git],[data-stage-directory-owned],'+
            '[data-stage-directory-child],[data-commit-directory],'+
            'input[aria-label="Folder commit message"],input[aria-label="目录提交说明"]'
        ).length;
    "#)?;
    if controls != 0 {
        return Err(format!("retired directory write controls remain: {controls}").into());
    }
    browser.css("[data-review-directory-layout]")?;
    browser.click(&browser.css("[data-zxs-directory-files]:not([disabled])")?)?;
    browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?;
    println!(
        "PASS directory write controls retired; existing layout review and file navigation remain"
    );
    Ok(())
}

/// This is a Web-request guard, not a declaration that backend routes are removed.
pub(super) fn is_retired_directory_request(url: &str, origin: &str) -> bool {
    let Some(relative) = url
        .strip_prefix(origin)
        .filter(|path| path.starts_with('/'))
    else {
        return false;
    };
    let (path, query) = relative.split_once('?').unwrap_or((relative, ""));
    let directory_commit = path
        .split_once("/directory-branches/")
        .and_then(|(_, tail)| tail.strip_suffix("/commits"))
        .is_some_and(|branch| !branch.is_empty() && !branch.contains('/'));
    path.ends_with("/directory-scopes")
        || path.ends_with("/directory-select")
        || path.ends_with("/directory-git/initialize")
        || path.ends_with("/directory-git/stage")
        || path.ends_with("/directory-git/children/stage")
        || directory_commit
        || (path.ends_with("/entries")
            && query
                .split('&')
                .any(|field| field.split('=').next() == Some("entry")))
}
