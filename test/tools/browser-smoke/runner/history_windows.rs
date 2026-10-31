//! Synthetic bounded protocol pages through production WASM. Real collection
//! identities and real selected IDs seed the fixture; no writes are submitted.
//! Host probes retain old nodes to inspect detachment, not to measure heap usage.
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

fn click(browser: &Browser, english: &str, chinese: &str) -> Result<()> {
    browser.click(&browser.button(english, chinese)?)
}

// A click starts an asynchronous fetch: the old DOM and old fixture expectation
// remain mutually consistent until that response arrives. Require the requested
// window as well, before inspecting nodes or advancing another page.
fn graph(browser: &Browser, expected_index: usize) -> Result<()> {
    check(
        browser,
        &format!("requested bounded history window {expected_index} and retired observer owners"),
        &format!(
            "const expectedIndex={expected_index};{}",
            r#"
        const p=window.__zxsHistoryWindows,root=document.querySelector('.zxs-version-tree-canvas');
        const ids=Array.from(document.querySelectorAll('.zxs-version-node'),node=>node.dataset.nodeId).sort();
        if(p.index!==expectedIndex||!root||!p.expected||ids.length!==32||JSON.stringify(ids)!==JSON.stringify([...p.expected].sort()))return false;
        if(!p.roots.includes(root))p.roots.push(root);
        const records=window.__zxsTreeFrames.observers;
        return document.querySelectorAll('.zxs-version-tree-canvas').length===1
            &&p.roots.filter(old=>old!==root).every(old=>!old.isConnected&&records.filter(record=>record.root===old).length===2&&records.filter(record=>record.root===old).every(record=>record.disconnected))
            &&records.filter(record=>record.root?.isConnected&&!record.disconnected).length===2;
    "#
        ),
    )
}

enum Choice<'a> {
    WindowNodeId(&'a str),
    ExactLabel(&'a str),
}

fn choose(browser: &Browser, english: &str, chinese: &str, choice: Choice<'_>) -> Result<()> {
    let (kind, value) = match choice {
        Choice::WindowNodeId(id) => ("window_node_id", id),
        Choice::ExactLabel(label) => ("exact_label", label),
    };
    check(
        browser,
        "enabled bounded select trigger",
        &format!(
            r#"
        const labels={},trigger=Array.from(document.querySelectorAll('[role=combobox]')).find(button=>labels.includes(document.getElementById(button.getAttribute('aria-labelledby'))?.textContent.trim()));
        if(!trigger||trigger.disabled)return false;
        window.__zxsHistoryWindows.selectPanel=trigger.getAttribute('aria-controls');trigger.click();return true;
    "#,
            json!([english, chinese])
        ),
    )?;
    check(
        browser,
        "bounded select option",
        &format!(
            r#"
        const p=window.__zxsHistoryWindows,target={},panel=document.getElementById(p.selectPanel);
        const options=Array.from(panel.querySelectorAll('[role=option]'));
        let index;
        if(target.kind==='window_node_id'){{
            // Both node controls preserve the admitted page order; every shared node has a body version.
            if(!p.expected||p.expected.length!==32||new Set(p.expected).size!==32||options.length!==p.expected.length)return false;
            index=p.expected.indexOf(target.value);
            if(index<0)return false;
        }}else{{
            const matches=options.flatMap((option,index)=>option.textContent.trim()===target.value?[index]:[]);
            if(matches.length!==1)return false;
            index=matches[0];
        }}
        const option=options[index];
        if(option.getAttribute('aria-disabled')==='true')return false;
        p.selectOption=option.id;option.click();return true;
    "#,
            json!({"kind":kind,"value":value})
        ),
    )?;
    check(
        browser,
        "bounded select accepts exactly the target option",
        r#"
        const p=window.__zxsHistoryWindows,panel=document.getElementById(p.selectPanel);
        const selected=Array.from(panel.querySelectorAll('[role=option][aria-selected=true]'));
        return selected.length===1&&selected[0].id===p.selectOption&&selected[0].getAttribute('aria-disabled')==='false';
    "#,
    )
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(r#"
        const original=window.fetch;
        const p=window.__zxsHistoryWindows={roots:[],pending:[],calls:[],seed:null,actualRoot:null,root:null,expected:null,reviewing:false,
            fieldText:'first-fragment\n'+'文'.repeat(12000)+'\nlast-fragment',
            restore:()=>{window.fetch=original;for(const pending of p.pending)pending.release();}};
        const copy=value=>structuredClone(value),encoder=new TextEncoder(),decoder=new TextDecoder('utf-8',{fatal:true});
        const encode=value=>Array.from(encoder.encode(JSON.stringify(value)),byte=>byte.toString(16).padStart(2,'0')).join('');
        const decode=value=>JSON.parse(decoder.decode(Uint8Array.from(value.match(/../g),byte=>parseInt(byte,16))));
        const cursor=(identity,anchor,direction,position)=>encode({format:'history_window_v1',history_root:identity.history_root,
            history_tree_id:identity.context.history_tree_id,file_id:identity.file_id,collection:identity.collection,
            query_hash:identity.query_hash,limit:identity.limit,direction,position,anchor_node_id:anchor});
        const ordered=nodes=>nodes.sort((a,b)=>BigInt(a.sequence)>BigInt(b.sequence)?-1:BigInt(a.sequence)<BigInt(b.sequence)?1:a.node_id.localeCompare(b.node_id));
        const position=node=>({kind:'node',sequence:node.sequence,node_id:node.node_id});
        const pages=()=>{
            const current=p.seed.nodes.find(node=>node.node_id===p.seed.identity.context.current_node_id);
            const older=p.seed.nodes.find(node=>node.node_id!==current.node_id);
            if(!current||!older)throw new Error('two real nodes required for bounded history fixture');
            p.current=current.node_id;p.older=older.node_id;
            const fake=(offset,count,sequence)=>Array.from({length:count},(_,index)=>({...copy(current),
                node_id:'fmeta_'+(offset+index).toString(16).padStart(32,'0'),kind:'tags',sequence:sequence(index),
                current:false,note:'',parents:{state:'complete'}}));
            return [ordered(fake(0x1000,32,index=>String(BigInt(current.sequence)+100n-BigInt(index)))),
                ordered([...fake(0x2000,31,()=>current.sequence),copy(current)]),
                ordered([copy(older),...fake(0x3000,31,()=>'0')])];
        };
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            if(request.method!=='GET'||!/^\/api\/file-manager\/files\/[^/]+\/history(?:\/|$)/.test(url.pathname))return original.apply(this,args);
            const incoming=url.searchParams.get('cursor'),payload=incoming?decode(incoming):null;
            const requestedRoot=url.searchParams.get('history_root')||payload?.history_root;
            if(requestedRoot&&p.root&&requestedRoot!==p.root)return new Response(JSON.stringify({code:'HISTORY_CHANGED',message:'Synthetic history root changed',retryable:false}),{status:409,headers:{'Content-Type':'application/json'}});
            const anchor=url.searchParams.get('anchor_node_id')||payload?.anchor_node_id||null;
            const seedURL=new URL(url);seedURL.searchParams.delete('cursor');
            if(anchor)seedURL.searchParams.set('anchor_node_id',anchor);
            if(p.actualRoot&&seedURL.searchParams.has('history_root'))seedURL.searchParams.set('history_root',p.actualRoot);
            const response=await original.call(this,new Request(seedURL,request));
            if(!response.ok)return response;
            const body=await response.json();
            p.actualRoot=body.identity.history_root;p.root??=p.actualRoot;
            body.identity.history_root=p.root;
            p.calls.push({kind:body.kind,anchor,url:url.href,root:p.root});
            if(body.kind==='nodes'){
                if(!p.seed)p.seed=copy(body);
                if(body.identity.limit!==32)throw new Error('history fixture expects the real default 32-node budget');
                const windows=pages(),owners=new Map(windows.flatMap((nodes,index)=>nodes.map(node=>[node.node_id,index])));
                let index=anchor===p.current?1:anchor===p.older?2:0;
                if(payload)index=owners.get(payload.position.node_id)+(payload.direction==='forward'?1:-1);
                if(!windows[index])throw new Error('synthetic history cursor did not advance to a window');
                body.nodes=windows[index];
                const ids=new Set(body.nodes.map(node=>node.node_id));
                for(const node of body.nodes){node.current=node.node_id===body.identity.context.current_node_id;node.fields.message.byte_length=String(encoder.encode(p.fieldText).length);}
                body.edges=p.seed.edges.filter(edge=>ids.has(edge.node_id)).map(edge=>({...edge,endpoint:ids.has(edge.parent_node_id)?'in_window':'outside_window'}));
                body.page={previous_cursor:index?cursor(body.identity,anchor,'backward',position(body.nodes[0])):null,
                    next_cursor:index<2?cursor(body.identity,anchor,'forward',position(body.nodes.at(-1))):null};
                body.context.note='window-root-'+p.root[0];
                if(!p.reviewing){p.expected=body.nodes.map(node=>node.node_id);p.index=index;}
            }else if(body.kind==='node_field'&&body.field==='message'){
                const bytes=encoder.encode(p.fieldText),offset=Number(payload?.position.byte_offset||0);
                let end=Math.min(offset+body.identity.limit,bytes.length);
                while(end<bytes.length&&(bytes[end]&0xc0)===0x80)end--;
                body.total_bytes=String(bytes.length);body.byte_offset=String(offset);body.text=decoder.decode(bytes.slice(offset,end));
                body.end_of_field=end===bytes.length;
                body.next_cursor=body.end_of_field?null:cursor(body.identity,null,'forward',{kind:'field',byte_offset:String(end)});
                p.lastField={node:body.identity.collection.node_id,field:body.field,text:body.text,offset,end,total:bytes.length};
            }
            if(body.kind==='node_field'&&p.holdField){
                p.holdField=false;
                let release;const gate=new Promise(resolve=>release=resolve);
                const pending={signal:request.signal,release,delivered:false};p.pending.push(pending);await gate;pending.delivered=true;
            }
            return new Response(JSON.stringify(body),{status:200,headers:{'Content-Type':'application/json'}});
        };return true;
    "#)?;
    with_cleanup(
        || exercise(browser),
        || {
            browser.script("if(window.__zxsHistoryWindows){window.__zxsHistoryWindows.restore();delete window.__zxsHistoryWindows;}return true")?;
            Ok(())
        },
    )?;
    click(browser, "Files", "文件")?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    check(
        browser,
        "real history restored after window fixtures",
        "return document.querySelectorAll('.zxs-version-node').length===2&&!document.querySelector('.zxs-header-refresh').disabled",
    )?;
    println!(
        "PASS synthetic 32-node window replacement/observer disposal, real-ID tag and merge basis review, replace-only UTF-8 fields and stale auxiliary delivery (no writes submitted)"
    );
    Ok(())
}

fn exercise(browser: &Browser) -> Result<()> {
    click(browser, "Refresh", "刷新")?;
    graph(browser, 0)?;
    check(
        browser,
        "independent current is outside the first window",
        "return window.__zxsHistoryWindows.index===0&&document.querySelectorAll('.zxs-version-current').length===0&&Array.from(document.querySelectorAll('button')).some(button=>['Locate current node','定位当前节点'].includes(button.textContent.trim())&&!button.disabled)",
    )?;
    click(browser, "Locate current node", "定位当前节点")?;
    graph(browser, 1)?;
    check(
        browser,
        "real current and explicit unloaded parent",
        "const p=window.__zxsHistoryWindows;return p.index===1&&document.querySelectorAll('.zxs-version-current').length===1&&document.querySelector('[data-parent-endpoint=outside_window][data-parent-node=\"'+p.older+'\"]')!==null",
    )?;
    browser.script("const p=window.__zxsHistoryWindows;document.querySelector('[data-parent-endpoint=outside_window][data-parent-node=\"'+p.older+'\"] button').click();return true")?;
    graph(browser, 2)?;
    for _ in 0..4 {
        click(browser, "Previous history page", "上一页历史")?;
        graph(browser, 1)?;
        click(browser, "Next history page", "下一页历史")?;
        graph(browser, 2)?;
    }
    browser.script("const p=window.__zxsHistoryWindows;document.querySelector('[data-node-id=\"'+p.older+'\"] .zxs-node-checkbox').click();return true")?;
    click(browser, "Previous history page", "上一页历史")?;
    graph(browser, 1)?;
    browser.script("const p=window.__zxsHistoryWindows;document.querySelector('[data-node-id=\"'+p.current+'\"] .zxs-node-checkbox').click();return true")?;
    click(browser, "Edit selected node tags", "编辑所选节点标签")?;
    check(
        browser,
        "both real tag sources retained across pages",
        "const p=window.__zxsHistoryWindows,text=document.querySelector('.zxs-tag-review')?.textContent||'';return text.includes(p.current)&&text.includes(p.older)&&/2\\s*(nodes selected|个节点已选择)/.test(document.querySelector('.zxs-entry-version-history').textContent)",
    )?;
    let tag = browser.css(
        ".zxs-tag-review input[aria-label='Add tag'],.zxs-tag-review input[aria-label='新增标签']",
    )?;
    browser.keys(&tag, "window-proof")?;
    browser.keys(&tag, "\u{e007}")?;
    browser.script("window.__zxsHistoryWindows.root='d'.repeat(64);return true")?;
    click(browser, "Refresh", "刷新")?;
    check(
        browser,
        "root change closes stale tag confirmation",
        "const panel=document.querySelector('.zxs-tag-review');return panel?.textContent.includes('window-proof')&&Array.from(panel.querySelectorAll('button')).some(button=>['Save tag child nodes','保存标签子节点'].includes(button.textContent.trim())&&button.disabled)",
    )?;
    browser.script("window.__zxsHistoryWindows.reviewing=true;return true")?;
    click(
        browser,
        "Load every selected node and branch for review",
        "读取全部节点及分支以重新审阅",
    )?;
    check(
        browser,
        "tag review requires explicit acceptance",
        "const panel=document.querySelector('.zxs-tag-review'),buttons=Array.from(panel.querySelectorAll('button'));return buttons.some(button=>['Accept every reviewed base','接受全部重新审阅的依据'].includes(button.textContent.trim())&&!button.disabled)&&buttons.some(button=>['Save tag child nodes','保存标签子节点'].includes(button.textContent.trim())&&button.disabled)",
    )?;
    click(
        browser,
        "Accept every reviewed base",
        "接受全部重新审阅的依据",
    )?;
    check(
        browser,
        "tag confirmation opens only after the complete explicit review",
        "const p=window.__zxsHistoryWindows,panel=document.querySelector('.zxs-tag-review');return panel.textContent.includes(p.current)&&panel.textContent.includes(p.older)&&Array.from(panel.querySelectorAll('button')).some(button=>['Save tag child nodes','保存标签子节点'].includes(button.textContent.trim())&&!button.disabled)",
    )?;
    browser.script("window.__zxsHistoryWindows.reviewing=false;Array.from(document.querySelectorAll('.zxs-tag-review button')).find(button=>['Cancel','取消'].includes(button.textContent.trim())).click();return true")?;
    merge_review(browser)?;
    fields_and_cancellation(browser)
}

fn merge_review(browser: &Browser) -> Result<()> {
    click(browser, "Merge file branches", "合并文件分支")?;
    check(
        browser,
        "genuine branch basis is available",
        "const panel=document.querySelector('.zxs-merge-review'),inputs=Array.from(panel?.querySelectorAll('fieldset input[type=checkbox]')||[]),input=inputs.find(input=>input.checked&&!input.disabled)||inputs.find(input=>!input.disabled);if(!input)return false;if(!input.checked){input.click();return false;}if(inputs.filter(input=>input.checked).length!==1)return false;window.__zxsHistoryWindows.branchName=input.closest('label').querySelector('span').textContent.trim();return true",
    )?;
    let older = browser
        .script("return window.__zxsHistoryWindows.older")?
        .as_str()
        .unwrap()
        .to_owned();
    let branch = browser
        .script("return window.__zxsHistoryWindows.branchName")?
        .as_str()
        .unwrap()
        .to_owned();
    let anchor = browser.css(".zxs-merge-review input[aria-label='Exact history node ID'],.zxs-merge-review input[aria-label='精确历史节点 ID']")?;
    browser.keys(&anchor, &older)?;
    click(browser, "Locate node", "定位节点")?;
    graph(browser, 2)?;
    choose(
        browser,
        "Result node (any node of this file)",
        "结果节点（可选此文件任一节点）",
        Choice::WindowNodeId(&older),
    )?;
    choose(
        browser,
        "Surviving branch",
        "合并后保留的分支",
        Choice::ExactLabel(&branch),
    )?;
    click(browser, "Previous history page", "上一页历史")?;
    graph(browser, 1)?;
    check(
        browser,
        "off-page merge result and selected branch remain captured",
        "const p=window.__zxsHistoryWindows,text=document.querySelector('.zxs-merge-review').textContent;return text.includes(p.older)&&text.includes(p.branchName)&&/1\\s*(branches selected|个分支已选择)/.test(text)",
    )?;
    captured_merge_selection(browser)?;
    browser.script("window.__zxsHistoryWindows.root='e'.repeat(64);return true")?;
    click(browser, "Refresh", "刷新")?;
    check(
        browser,
        "new root closes stale merge confirmation",
        "return Array.from(document.querySelectorAll('.zxs-merge-review button')).some(button=>['Confirm merge of selected branches','确认合并所选分支'].includes(button.textContent.trim())&&button.disabled)",
    )?;
    browser.script("window.__zxsHistoryWindows.reviewing=true;return true")?;
    click(
        browser,
        "Load all selected bases for review",
        "读取全部所选依据以重新审阅",
    )?;
    check(
        browser,
        "merge review still requires explicit acceptance",
        "const buttons=Array.from(document.querySelectorAll('.zxs-merge-review button'));return buttons.some(button=>['Accept every reviewed base','接受重新审阅的全部依据'].includes(button.textContent.trim())&&!button.disabled)&&buttons.some(button=>['Confirm merge of selected branches','确认合并所选分支'].includes(button.textContent.trim())&&button.disabled)",
    )?;
    click(
        browser,
        "Accept every reviewed base",
        "接受重新审阅的全部依据",
    )?;
    check(
        browser,
        "merge root basis is accepted without submitting a write",
        "return Array.from(document.querySelectorAll('.zxs-merge-review button')).some(button=>['Confirm merge of selected branches','确认合并所选分支'].includes(button.textContent.trim())&&!button.disabled)",
    )?;
    captured_merge_selection(browser)?;
    browser.script("window.__zxsHistoryWindows.reviewing=false;Array.from(document.querySelectorAll('.zxs-merge-review button')).find(button=>['Cancel','取消'].includes(button.textContent.trim())).click();return true")?;
    Ok(())
}

fn captured_merge_selection(browser: &Browser) -> Result<()> {
    check(
        browser,
        "captured merge result and survivor retain their exact identities",
        r#"
        const p=window.__zxsHistoryWindows,panel=document.querySelector('.zxs-merge-review');
        const control=labels=>Array.from(panel.querySelectorAll('[role=combobox]')).find(button=>labels.includes(document.getElementById(button.getAttribute('aria-labelledby'))?.textContent.trim()));
        const result=control(['Result node (any node of this file)','结果节点（可选此文件任一节点）']);
        const survivor=control(['Surviving branch','合并后保留的分支']);
        if(!result||!survivor)return false;
        const resultSelected=Array.from(document.getElementById(result.getAttribute('aria-controls')).querySelectorAll('[role=option][aria-selected=true]'));
        const survivorSelected=Array.from(document.getElementById(survivor.getAttribute('aria-controls')).querySelectorAll('[role=option][aria-selected=true]'));
        return resultSelected.length===1&&['Selected node · '+p.older,'所选节点 · '+p.older].includes(resultSelected[0].textContent.trim())
            &&result.querySelector('.zxs-select-value').textContent.trim()===resultSelected[0].textContent.trim()
            &&survivorSelected.length===1&&survivorSelected[0].textContent.trim()===p.branchName
            &&survivor.querySelector('.zxs-select-value').textContent.trim()===p.branchName
            &&/1\s*(branches selected|个分支已选择)/.test(panel.textContent);
    "#,
    )
}

fn fields_and_cancellation(browser: &Browser) -> Result<()> {
    click(browser, "Locate current node", "定位当前节点")?;
    graph(browser, 1)?;
    let current = browser
        .script("return window.__zxsHistoryWindows.current")?
        .as_str()
        .unwrap()
        .to_owned();
    choose(
        browser,
        "Node on this page",
        "本页节点",
        Choice::WindowNodeId(&current),
    )?;
    click(browser, "Message", "提交说明")?;
    check(
        browser,
        "first bounded UTF-8 field fragment",
        "const p=window.__zxsHistoryWindows,panel=document.querySelector('.zxs-history-field');return p.lastField?.node===p.current&&p.lastField.field==='message'&&['Message · '+p.current,'提交说明 · '+p.current].includes(panel?.querySelector('h3').textContent.trim())&&panel.querySelector('pre').textContent===p.lastField.text&&panel.querySelector('pre').textContent.startsWith('first-fragment')&&panel.querySelector('[data-field-complete=false]')!==null",
    )?;
    click(browser, "Next field fragment", "下一片字段")?;
    check(
        browser,
        "last fragment replaces rather than accumulates earlier bytes",
        "const p=window.__zxsHistoryWindows,panel=document.querySelector('.zxs-history-field'),text=panel?.querySelector('pre').textContent||'';return document.querySelectorAll('.zxs-history-field').length===1&&p.lastField.node===p.current&&p.lastField.field==='message'&&['Message · '+p.current,'提交说明 · '+p.current].includes(panel.querySelector('h3').textContent.trim())&&p.lastField.offset>0&&text===p.lastField.text&&!text.includes('first-fragment')&&text.endsWith('last-fragment')&&panel.querySelector('[data-field-complete=false]')!==null&&Array.from(panel.querySelectorAll('button')).some(button=>['Next field fragment','下一片字段'].includes(button.textContent.trim())&&button.disabled)",
    )?;
    click(browser, "Close field", "关闭字段")?;
    browser.script("window.__zxsHistoryWindows.holdField=true;return true")?;
    browser.click(&browser.css(&format!(
        "[data-node-id='{current}'] [data-history-field=created_at]"
    ))?)?;
    check(
        browser,
        "auxiliary A held",
        "const p=window.__zxsHistoryWindows.pending;return p.length===1&&!p[0].signal.aborted",
    )?;
    click(browser, "Files", "文件")?;
    click(browser, "Refresh", "刷新")?;
    click(browser, "Details", "详情")?;
    check(
        browser,
        "main refresh aborts old auxiliary A",
        "return window.__zxsHistoryWindows.pending[0].signal.aborted&&!document.querySelector('.zxs-header-refresh').disabled",
    )?;
    click(browser, "Locate current node", "定位当前节点")?;
    graph(browser, 1)?;
    browser.script("window.__zxsHistoryWindows.holdField=true;return true")?;
    browser.click(&browser.css(&format!(
        "[data-node-id='{current}'] [data-history-field=created_at]"
    ))?)?;
    check(
        browser,
        "new auxiliary B held",
        "const p=window.__zxsHistoryWindows.pending;return p.length===2&&!p[1].signal.aborted",
    )?;
    browser.script("window.__zxsHistoryWindows.pending[0].release();return true")?;
    check(
        browser,
        "late auxiliary A cannot install or release B",
        "const p=window.__zxsHistoryWindows.pending,fields=document.querySelector('[aria-label=\"History fields and body sources\"],[aria-label=\"历史字段与正文来源\"]');return p[0].delivered&&!p[1].signal.aborted&&!document.querySelector('.zxs-history-field')&&fields.querySelector('[role=combobox]').disabled",
    )?;
    click(browser, "Files", "文件")?;
    click(browser, "Refresh", "刷新")?;
    click(browser, "Details", "详情")?;
    check(
        browser,
        "B remains owned and is aborted by replacement",
        "return window.__zxsHistoryWindows.pending[1].signal.aborted&&!document.querySelector('.zxs-header-refresh').disabled",
    )?;
    browser.script("window.__zxsHistoryWindows.pending[1].release();return true")?;
    check(
        browser,
        "late auxiliary B cannot restore a retired fragment",
        "return window.__zxsHistoryWindows.pending[1].delivered&&!document.querySelector('.zxs-history-field')",
    )?;
    graph(browser, 0)
}
