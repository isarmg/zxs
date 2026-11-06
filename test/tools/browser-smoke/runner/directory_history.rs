//! Real directory previews and history responses through production WASM. Only
//! the rejected unsolicited-preview response and response timing are synthetic;
//! the admitted listing, history nodes, branch identities and parents are real.
use super::*;

fn check(browser: &Browser, label: &str, script: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}

fn install_response_probe(browser: &Browser) -> Result<()> {
    browser.script(r#"
        const original=window.fetch;
        const p=window.__zxsDirectoryHistory={nodes:null,branches:null,writes:0,readonlyPages:0,rejectedPages:0,unexpectedPreview:true,hold:false,pending:null,lastAnchor:null,
            restore:()=>{window.fetch=original;if(p.pending)p.pending.release();}};
        window.fetch=async function(...args){
            const input=args[0],url=new URL(typeof input==='string'?input:input.url,location.href);
            const method=(args[1]?.method||input.method||'GET').toUpperCase();
            if(url.origin!==location.origin)return original.apply(this,args);
            if(url.pathname.startsWith('/api/')&&!['GET','HEAD'].includes(method))p.writes++;
            const response=await original.apply(this,args);
            if(method==='GET'&&url.pathname==='/api/file-manager/disks/P01/entries'&&url.searchParams.get('path')===''&&response.ok&&p.unexpectedPreview){
                const body=await response.clone().json();
                if(url.searchParams.has('directory_branch_id')||body.slot_id!=='P01'||body.path!==''||body.writable!==true||body.backup||body.preview)
                    throw new Error('unsolicited-preview negative requires the real writable primary root listing');
                // This response must fail admission: the request selected no
                // directory branch. It is never reused as the read-only fixture.
                body.preview=true;body.writable=false;p.unexpectedPreview=false;p.rejectedPages++;
                return new Response(JSON.stringify(body),{status:200,headers:{'Content-Type':'application/json'}});
            }
            if(method==='GET'&&url.pathname==='/api/file-manager/disks/P01/entries'&&url.searchParams.has('directory_branch_id')&&response.ok){
                const body=await response.clone().json();
                if(!url.searchParams.get('expected_tree_id')||body.preview!==true||body.writable!==false||body.backup)
                    throw new Error('selected real directory preview must be read-only');
                p.readonlyPages++;p.previewTree=url.searchParams.get('expected_tree_id');
            }
            if(method==='GET'&&/\/directories\/[^/]+\/history(?:\/branches)?$/.test(url.pathname)){
                if(!response.ok)throw new Error('real folder history request failed');
                const body=await response.clone().json();
                if(body.identity.collection==='nodes'){p.nodes=body;p.lastAnchor=url.searchParams.get('anchor_tree_id');}
                else p.branches=body;
                if(p.hold&&body.identity.collection==='nodes'){
                    p.hold=false;return new Promise(resolve=>{p.pending={signal:args[1]?.signal||input.signal,release:()=>{p.pending=null;resolve(response);}};});
                }
            }
            return response;
        };return true;
    "#)?;
    Ok(())
}

fn open_readonly_child_history(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    check(
        browser,
        "unsolicited directory preview fails closed without installing entries",
        r#"
        const p=window.__zxsDirectoryHistory;
        return p.rejectedPages===1&&document.querySelector('.zxs-error-center')?.textContent.includes('INVALID_DIRECTORY_PAGE')
            &&document.querySelector('.zxs-file-manager')?.getAttribute('aria-busy')==='false'
            &&document.querySelectorAll('.zxs-file-table .zxs-file-name').length===0&&p.writes===0;
    "#,
    )?;
    browser.click(&browser.element("xpath", "//aside[contains(@class,'zxs-error-center')]/div[contains(@class,'zxs-inline-feedback')][contains(.,'INVALID_DIRECTORY_PAGE')]//button[@aria-label='Dismiss error' or @aria-label='关闭错误提示']")?)?;
    browser.click(&browser.css(".zxs-header-refresh:not([disabled])")?)?;
    browser.click(&browser.element(
        "xpath",
        &format!(
            "//button[contains(@class,'zxs-file-name')][normalize-space(.)='{}']",
            fixture::DIRECTORY_REPOSITORY
        ),
    )?)?;
    let combo = browser.element("xpath", "//div[contains(@class,'zxs-select')][./label[normalize-space(.)='Browse folder branch' or normalize-space(.)='浏览目录分支']]/button[@role='combobox' and not(@disabled)]")?;
    let panel = browser.command(
        "GET",
        &format!("/element/{combo}/attribute/aria-controls"),
        None,
    )?;
    let panel = panel.as_str().ok_or("folder combobox lacks a listbox")?;
    browser.click(&combo)?;
    browser.click(&browser.element(
        "xpath",
        &select_option_xpath(panel, fixture::DIRECTORY_PREVIEW),
    )?)?;
    check(
        browser,
        "real parent preview settled before selecting its child",
        "return window.__zxsDirectoryHistory.readonlyPages>0&&!!document.querySelector('.zxs-file-manager[aria-busy=\"false\"]')",
    )?;
    browser.click(&browser.element("xpath", &format!("//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='{}']]//button[normalize-space(.)='Details' or normalize-space(.)='详情']", fixture::DIRECTORY_PATH))?)?;
    Ok(())
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    install_response_probe(browser)?;
    let result = with_cleanup(
        || {
            open_readonly_child_history(browser)?;
            check(
                browser,
                "readonly folder renders actual nodes and shared branch heads",
                r#"
            const p=window.__zxsDirectoryHistory,panel=document.querySelector('.zxs-directory-history');
            if(!p.readonlyPages||!p.nodes||!p.branches||!panel||panel.getAttribute('aria-busy')!=='false')return false;
            const expected=p.nodes.nodes.map(node=>node.tree_id).sort(),actual=Array.from(panel.querySelectorAll('[data-directory-tree-id]'),node=>node.dataset.directoryTreeId).sort();
            const branches=Array.from(panel.querySelectorAll('[data-directory-branch-id]'),node=>node.dataset.directoryBranchId).sort();
            return expected.length>=3&&JSON.stringify(expected)===JSON.stringify(actual)&&new Set(actual).size===actual.length
                &&JSON.stringify(branches)===JSON.stringify(p.branches.branches.map(branch=>branch.directory_branch_id).sort())
                &&p.branches.branches.length>=2&&new Set(p.branches.branches.map(branch=>branch.committed_tree_id)).size===1
                &&!Array.from(document.querySelectorAll('button')).some(button=>['Create folder branch','创建目录分支'].includes(button.textContent.trim()))&&p.writes===0;
        "#,
            )?;
            node_cards::run(browser)?;
            node_cards::open_first(browser)?;
            check(
                browser,
                "directory SVG draws exactly the real parent edges at the measured nodes",
                include_str!("directory_tree_edges.js"),
            )?;
            check(
                browser,
                "folder history titles use one compact row without doubled text insets",
                include_str!("history_title_rows.js"),
            )?;
            browser.script("const p=window.__zxsDirectoryHistory;p.originalNode=document.querySelector('[data-directory-tree-id]');p.originalEdge=document.querySelector('path[data-parent-tree][data-child-tree]');p.hold=true;return true")?;
            browser.click(&browser.button("Refresh folder history", "刷新目录历史")?)?;
            check(
                browser,
                "refresh keeps actual folder graph without loading text",
                r#"
            const p=window.__zxsDirectoryHistory,panel=document.querySelector('.zxs-directory-history');
            return p.pending&&p.originalNode.isConnected&&panel.contains(p.originalNode)&&p.originalEdge.isConnected&&p.originalEdge.getTotalLength()>0&&!panel.textContent.includes('正在加载')&&!panel.textContent.includes('Loading')&&p.writes===0;
        "#,
            )?;
            browser.script("window.__zxsDirectoryHistory.pending.release();return true")?;
            check(
                browser,
                "folder refresh completes on real same-root data",
                "return document.querySelector('.zxs-directory-history').getAttribute('aria-busy')==='false'&&window.__zxsDirectoryHistory.writes===0",
            )?;
            check(
                browser,
                "refreshed directory SVG still has the real parent edges",
                include_str!("directory_tree_edges.js"),
            )?;
            browser.click(&browser.element("xpath","//section[contains(@class,'zxs-directory-history-branches')]//button[normalize-space(.)='Locate branch node' or normalize-space(.)='定位分支节点']")?)?;
            check(
                browser,
                "branch locator reads its real tree and never writes",
                r#"
            const p=window.__zxsDirectoryHistory,panel=document.querySelector('.zxs-directory-history');
            return p.lastAnchor&&panel.getAttribute('aria-busy')==='false'&&p.branches.branches.some(branch=>branch.committed_tree_id===p.lastAnchor)&&!!panel.querySelector('[data-directory-tree-id="'+p.lastAnchor+'"]')&&p.writes===0;
        "#,
            )?;
            check(
                browser,
                "anchored directory SVG still joins the actual parent and child",
                include_str!("directory_tree_edges.js"),
            )?;
            let target = browser.script(r#"
                const p=window.__zxsDirectoryHistory;
                const node=p.nodes.nodes.find(node=>node.tree_id!==p.nodes.current.committed_tree_id);
                if(!node)throw new Error('historical snapshot fixture requires a non-current committed node');
                p.selectedSnapshot=node.tree_id;
                const selector='[data-directory-tree-id="'+CSS.escape(node.tree_id)+'"]';
                const owner=document.querySelector(selector);
                const detail=owner?.querySelector(':scope > .zxs-node-disclosure');
                const button=owner?.querySelector('[data-directory-snapshot="'+CSS.escape(node.tree_id)+'"]');
                if(!detail||!button||button.disabled)throw new Error('explicit historical snapshot is unavailable');
                return {selector,open:detail.open};
            "#)?;
            let selector = target["selector"]
                .as_str()
                .ok_or("historical snapshot target lacks its real tree selector")?;
            // The branch locator may replace the window with closed cards.
            // Open this exact real snapshot through its native summary first.
            if target["open"] != true {
                browser.click(&browser.css(&format!(
                    "{selector} > .zxs-node-disclosure > .zxs-node-card"
                ))?)?;
            }
            browser.click(&browser.css(&format!("{selector} [data-directory-snapshot]"))?)?;
            check(
                browser,
                "explicit historical snapshot loads read-only without checkout authority",
                r#"
                const p=window.__zxsDirectoryHistory;
                return p.previewTree===p.selectedSnapshot&&!!document.querySelector('[data-directory-snapshot-readonly]')
                    &&document.querySelector('.zxs-file-manager')?.getAttribute('aria-busy')==='false'
                    &&document.querySelector('[data-zxs-set-directory-default]')?.disabled===true
                    &&!document.querySelector('[data-commit-directory]')&&p.writes===0;
            "#,
            )?;
            Ok(())
        },
        || {
            browser.script("if(window.__zxsDirectoryHistory){window.__zxsDirectoryHistory.restore();delete window.__zxsDirectoryHistory;}return true")?;
            Ok(())
        },
    );
    result?;
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    open_fixture_directory(browser)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    println!(
        "PASS unsolicited preview fails closed; real read-only folder preview and history: measured SVG parent connectors, shared heads, branch locator, graph-preserving refresh and explicit immutable snapshot without checkout or writes"
    );
    Ok(())
}
