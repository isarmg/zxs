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

pub(super) fn metadata_edit(browser: &Browser) -> Result<()> {
    // Keep mutations and the selected-entry listing real. Only widen the GET
    // history fixture, so horizontal position is meaningful at desktop widths.
    browser.script(r#"
        const original=window.fetch;
        const probe=window.__zxsDetailRefresh={historyResponses:0,savedListingSeen:false,roots:[],requests:[],restore:()=>window.fetch=original};
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            const response=await original.apply(this,args);
            if(!response.ok)return response;
            if(/^\/api\/file-manager\/disks\/[^/]+\/entries$/.test(url.pathname)){
                const listing=await response.clone().json();
                if(listing.selected_entry?.note==='browser metadata note')probe.savedListingSeen=true;
            }
            if(!/^\/api\/file-manager\/files\/[^/]+\/history$/.test(url.pathname))return response;
            probe.requests.push(url.searchParams.get('history_root'));
            const history=await response.json();
            probe.roots.push(history.history_root);
            const base=history.nodes.find(node=>node.version_id&&node.node_id);
            if(!base)throw new Error('real node required for detail refresh fixture');
            history.nodes.push(...Array.from({length:12},(_,index)=>({...base,node_id:'detail-refresh-tip-'+index,
                version_id:'detail-refresh-tip-'+index,sequence:String(BigInt(base.sequence)+BigInt(index)+1n),
                parents:{status:'complete',edges:[{node_id:base.node_id,in_window:true}],cursor:null},current:false,kind:'content'})));
            history.nodes.sort((a,b)=>BigInt(a.sequence)===BigInt(b.sequence)?a.node_id.localeCompare(b.node_id):BigInt(a.sequence)>BigInt(b.sequence)?-1:1);
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
                probe.anchor={detail:document.querySelector('.zxs-entry-content'),root:probe.roots.at(-1),tree,canvas:tree.querySelector('.zxs-version-tree-canvas'),
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
                "file note transaction reads a fresh root and releases the previous tree owner",
                || {
                    if browser.script(r#"
                    const probe=window.__zxsDetailRefresh,a=probe.anchor;
                    const note=Array.from(document.querySelectorAll('button')).find(e=>['Edit file note','编辑文件备注'].includes(e.getAttribute('aria-label')));
                    return probe.savedListingSeen&&probe.historyResponses>a.histories&&note&&!note.disabled
                        &&!document.querySelector('.zxs-inline-input')&&note.textContent.includes('browser metadata note')
                        &&a.detail===document.querySelector('.zxs-entry-content')
                        &&probe.roots.at(-1)!==a.root&&probe.requests.at(-1)===null
                        &&a.tree!==document.querySelector('.zxs-version-tree-scroll')&&!a.canvas.isConnected
                        &&!a.node.isConnected&&!a.control.isConnected
                        &&document.querySelectorAll('.zxs-version-node').length===14;
                "#)? == true { Ok(()) }
                else { Err("metadata refresh did not install a fresh root or release the previous tree".into()) }
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
        "PASS real note save discovers a fresh root without HISTORY_CHANGED and releases old tree ownership; persisted note survives navigation (wide history is synthetic)"
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
