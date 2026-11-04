//! Real directory history responses through production WASM. Only listing
//! writability and response timing are intercepted; no history nodes are invented.
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

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(r#"
        const original=window.fetch;
        const p=window.__zxsDirectoryHistory={nodes:null,branches:null,writes:0,hold:false,pending:null,lastAnchor:null,
            restore:()=>{window.fetch=original;if(p.pending)p.pending.release();}};
        window.fetch=async function(...args){
            const input=args[0],url=new URL(typeof input==='string'?input:input.url,location.href);
            const method=(args[1]?.method||input.method||'GET').toUpperCase();
            if(url.origin!==location.origin)return original.apply(this,args);
            if(url.pathname.startsWith('/api/')&&!['GET','HEAD'].includes(method))p.writes++;
            const response=await original.apply(this,args);
            if(method==='GET'&&url.pathname.endsWith('/entries')&&response.ok){
                const body=await response.json();body.writable=false;
                return new Response(JSON.stringify(body),{status:200,headers:{'Content-Type':'application/json'}});
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
    let result = with_cleanup(
        || {
            browser.click(&browser.button("Files", "文件")?)?;
            browser.click(&browser.button("All disks", "所有硬盘")?)?;
            browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
            browser.click(&browser.element("xpath","//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-fixtures']]//button[normalize-space(.)='Details' or normalize-space(.)='详情']")?)?;
            check(
                browser,
                "readonly folder renders actual nodes and shared branch heads",
                r#"
            const p=window.__zxsDirectoryHistory,panel=document.querySelector('.zxs-directory-history');
            if(!p.nodes||!p.branches||!panel||panel.getAttribute('aria-busy')!=='false')return false;
            const expected=p.nodes.nodes.map(node=>node.tree_id).sort(),actual=Array.from(panel.querySelectorAll('[data-directory-tree-id]'),node=>node.dataset.directoryTreeId).sort();
            const branches=Array.from(panel.querySelectorAll('[data-directory-branch-id]'),node=>node.dataset.directoryBranchId).sort();
            return expected.length>0&&JSON.stringify(expected)===JSON.stringify(actual)&&new Set(actual).size===actual.length
                &&JSON.stringify(branches)===JSON.stringify(p.branches.branches.map(branch=>branch.directory_branch_id).sort())
                &&!Array.from(document.querySelectorAll('button')).some(button=>['Create folder branch','创建目录分支'].includes(button.textContent.trim()))&&p.writes===0;
        "#,
            )?;
            check(
                browser,
                "directory SVG draws exactly the real parent edges at the measured nodes",
                include_str!("directory_tree_edges.js"),
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
            return p.lastAnchor&&panel.getAttribute('aria-busy')==='false'&&p.branches.branches.some(branch=>branch.head_tree_id===p.lastAnchor)&&!!panel.querySelector('[data-directory-tree-id="'+p.lastAnchor+'"]')&&p.writes===0;
        "#,
            )?;
            check(
                browser,
                "anchored directory SVG still joins the actual parent and child",
                include_str!("directory_tree_edges.js"),
            )?;
            Ok(())
        },
        || {
            browser.script("if(window.__zxsDirectoryHistory){window.__zxsDirectoryHistory.restore();delete window.__zxsDirectoryHistory;}return true")?;
            Ok(())
        },
    );
    result?;
    browser.click(&browser.button("Browse folder branch", "浏览目录分支")?)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    println!(
        "PASS real readonly folder nodes, measured SVG parent connectors, shared heads, branch locator and graph-preserving refresh without writes"
    );
    Ok(())
}
