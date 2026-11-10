//! Real registered chain, grid geometry, selection and retained note drafts.
use super::*;

pub(super) fn select(browser: &Browser, slot: &str) -> Result<()> {
    browser.click(&browser.css(&format!(".zxs-disk-tree-node[data-slot='{slot}']"))?)?;
    wait_for("selected disk detail card", || {
        let shown = browser.script(&format!(
            r#"
            const slot={},cards=[...document.querySelectorAll('.zxs-disk-management-card')];
            const selected=document.querySelector('.zxs-disk-tree-node[aria-pressed="true"]');
            return cards.length===1&&cards[0].dataset.slot===slot&&selected?.dataset.slot===slot
                &&cards[0].getBoundingClientRect().height>0;
        "#,
            json!(slot)
        ))?;
        if shown == true {
            Ok(())
        } else {
            Err("selected disk is not the sole visible detail card".into())
        }
    })
}

fn geometry(browser: &Browser) -> Result<()> {
    wait_for("upward backup tree geometry", || {
        let observed=browser.script(r#"
            const scroll=document.querySelector('.zxs-disk-tree-scroll');
            const nodes=[...document.querySelectorAll('.zxs-disk-tree-node')];
            const edges=[...document.querySelectorAll('.zxs-disk-tree-edge')];
            if(!scroll||nodes.length!==3||edges.length!==2)return {ready:false};
            const bySlot=new Map(nodes.map(node=>[node.dataset.slot,node]));
            const tierLabels=nodes.every(node=>{
                const level=Number(node.dataset.level);
                const label=node.querySelector('[data-disk-tier-label]')?.textContent;
                return (level===1?['主盘 · 第 1 层','Primary · Tier 1']:
                    [`备份盘 · 第 ${level} 层`,`Backup disk · Tier ${level}`]).includes(label);
            });
            const expected=[['P01','B01'],['B01','C01']];
            const valid=expected.every(([source,target])=>{
                const from=bySlot.get(source),to=bySlot.get(target);
                const edge=edges.find(edge=>edge.dataset.sourceSlot===source&&edge.dataset.targetSlot===target);
                if(!from||!to||!edge)return false;
                const a=from.getBoundingClientRect(),b=to.getBoundingClientRect(),e=edge.getBoundingClientRect();
                const line=edge.querySelector('.zxs-disk-tree-line'),arrow=edge.querySelector('.zxs-disk-tree-arrow');
                const end=line?.getPointAtLength(line.getTotalLength()),start=line?.getPointAtLength(0);
                return Number(from.dataset.level)+1===Number(to.dataset.level)&&a.top>b.bottom
                    &&Math.abs((a.left+a.right-b.left-b.right)/2)<1
                    &&Math.abs(e.top-b.bottom)<1&&Math.abs(e.bottom-a.top)<1
                    &&start.y>end.y&&arrow?.getAttribute('d')==='M2 10 L6 2 L10 10 Z'
                    &&getComputedStyle(edge).pointerEvents==='none';
            });
            return {ready:true,valid:valid&&tierLabels,documentWidth:document.documentElement.scrollWidth,
                clientWidth:document.documentElement.clientWidth,
                scrollWidth:scroll.scrollWidth,scrollClient:scroll.clientWidth,
                nodes:nodes.map(node=>({slot:node.dataset.slot,level:node.dataset.level,rect:node.getBoundingClientRect().toJSON()}))};
        "#)?;
        if observed["ready"] == true
            && observed["valid"] == true
            && observed["documentWidth"].as_u64().unwrap_or(u64::MAX)
                <= observed["clientWidth"].as_u64().unwrap_or(0) + 1
            && observed["scrollWidth"].as_u64().unwrap_or(u64::MAX)
                <= observed["scrollClient"].as_u64().unwrap_or(0) + 1
        {
            Ok(())
        } else {
            Err(format!("backup tree geometry is not ready or invalid: {observed}").into())
        }
    })
}

fn keyboard_selection(browser: &Browser) -> Result<()> {
    let top = browser.css(".zxs-disk-tree-node[data-slot='C01']")?;
    browser.click(&top)?;
    browser.keys(&top, "\u{e004}")?;
    if browser.script(
        r#"
        const node=document.activeElement,style=getComputedStyle(node);
        return node?.dataset.slot==='B01'&&node.matches(':focus-visible')
            &&parseFloat(style.outlineWidth)>0&&!['none','hidden'].includes(style.outlineStyle);
    "#,
    )? != true
    {
        return Err("backup-tree Tab order or visible keyboard focus is missing".into());
    }
    let node = browser.css(".zxs-disk-tree-node[data-slot='B01']")?;
    browser.keys(&node, "\u{e007}")?;
    wait_for("keyboard selects the backup detail card", || {
        if browser.script(
            "return document.querySelector('.zxs-disk-management-card')?.dataset.slot==='B01'",
        )? == true
        {
            Ok(())
        } else {
            Err("Enter did not select B01".into())
        }
    })
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    AdminPage::Storage.open(browser)?;
    browser.css(".zxs-header-refresh:not([disabled])")?;
    with_cleanup(
        || {
            for (width, height) in [(320, 568), (390, 844), (1440, 900)] {
                browser.cdp(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":false}),
                )?;
                geometry(browser)?;
            }
            keyboard_selection(browser)?;
            select(browser, "P01")?;
            browser.click(&browser.element("xpath","//section[contains(@class,'zxs-disk-management-card')]//button[normalize-space(.)='Edit note' or normalize-space(.)='编辑备注']")?)?;
            let input = browser.css(".zxs-disk-management-card textarea")?;
            let original = browser.script(
                "return document.querySelector('.zxs-disk-management-card textarea').value",
            )?;
            browser.keys(&input, "\nunsaved tree selection probe")?;
            let edited = browser.script(
                "return document.querySelector('.zxs-disk-management-card textarea').value",
            )?;
            if original == edited {
                return Err("disk-tree note fixture did not change the local draft".into());
            }
            select(browser, "B01")?;
            select(browser, "C01")?;
            select(browser, "P01")?;
            let retained = browser.script(
                "return document.querySelector('.zxs-disk-management-card textarea')?.value",
            )?;
            if retained != edited {
                return Err("changing tree selection discarded the unsaved note".into());
            }
            browser.click(&browser.element("xpath","//section[contains(@class,'zxs-disk-management-card')]//button[normalize-space(.)='Cancel' or normalize-space(.)='取消']")?)?;
            // Keep selection through a real list/detail refresh and page re-entry.
            select(browser, "C01")?;
            browser.click(&browser.css(".zxs-header-refresh:not([disabled])")?)?;
            browser.css(".zxs-header-refresh:not([disabled])")?;
            if browser.script(
                "return document.querySelector('.zxs-disk-management-card')?.dataset.slot",
            )? != "C01"
            {
                return Err("disk refresh replaced the selected tree node".into());
            }
            browser.click(&browser.button("Files", "文件")?)?;
            AdminPage::Storage.open(browser)?;
            geometry(browser)?;
            Ok(())
        },
        || {
            browser.cdp("Emulation.clearDeviceMetricsOverride", json!({}))?;
            select(browser, "P01")?;
            // Never save the test draft, including when a preceding assertion failed.
            if browser
                .script("return !!document.querySelector('.zxs-disk-management-card textarea')")?
                == true
            {
                browser.click(&browser.element("xpath","//section[contains(@class,'zxs-disk-management-card')]//button[normalize-space(.)='Cancel' or normalize-space(.)='取消']")?)?;
            }
            browser.click(&browser.button("Files", "文件")?)?;
            Ok(())
        },
    )?;
    println!(
        "PASS real backup-tree tier order, upward source arrows, narrow layouts, disk selection and retained note drafts"
    );
    Ok(())
}
