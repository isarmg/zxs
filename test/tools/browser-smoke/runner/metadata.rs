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
            const tips=Array.from({length:12},(_,index)=>({...base,
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
