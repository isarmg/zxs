//! Synthetic device reports on real registered-directory responses. This tests
//! production WASM attribution and unavailable states, not physical SMART access.
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

fn refresh_card(browser: &Browser, slot: &str) -> Result<()> {
    disk_tree::select(browser, slot)?;
    browser.click(&browser.element(
        "xpath",
        &format!(
            "//section[contains(@class,'zxs-disk-management-card')][@data-slot='{slot}']//button[not(@disabled) and (normalize-space(.)='Refresh disk information' or normalize-space(.)='刷新硬盘信息')]"
        ),
    )?)
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(r#"
        const original=window.fetch;
        const p=window.__zxsDiskHealth={mode:'mixed',reads:[],restore:()=>window.fetch=original};
        window.fetch=async function(...args){
            const request=args[0],url=new URL(typeof request==='string'?request:request.url,location.href);
            const method=request.method||args[1]?.method||'GET';
            const response=await original.apply(this,args);
            const match=url.pathname.match(/^\/api\/admin\/disks\/(P01|B01|C01)\/details$/);
            if(url.origin!==location.origin||method!=='GET'||!match||!response.ok)return response;
            const value=await response.json(),slot=match[1];
            if(value.slot_id!==slot)throw new Error('device-health fixture needs the matching real slot');
            p.reads.push({slot,mode:p.mode});
            value.root_path='/synthetic-directory/'+slot;
            if(slot==='B01'){
                value.mapping='multiple';
                value.health={state:'unavailable',reason:'multiple_devices'};
            }else if(slot==='C01'&&p.mode!=='shared'){
                value.mapping='single';
                value.health={state:'unavailable',reason:'permission_denied'};
            }else{
                value.mapping='single';
                value.device_path='/dev/nvme9n1p1';
                value.health={state:'smart',observed_at_unix_ms:'1700000000123',device_path:'/dev/nvme9n1',
                    assessment:p.mode==='failed'?'failed':'passed',temperature_celsius:0,power_on_hours:'0'};
                if(p.mode==='invalid')value.health.assessment='invented';
            }
            return new Response(JSON.stringify(value),{status:200,headers:{'Content-Type':'application/json'}});
        };return true;
    "#)?;
    with_cleanup(
        || {
            browser.click(&browser.button("Files", "文件")?)?;
            AdminPage::Storage.open(browser)?;
            disk_tree::select(browser, "P01")?;
            check(
                browser,
                "per-directory SMART, multiple-device and denied reports",
                r#"
                const card=slot=>document.querySelector('.zxs-disk-management-card[data-slot="'+slot+'"]');
                const one=card('P01');
                const read=slot=>window.__zxsDiskHealth.reads.some(value=>value.slot===slot);
                if(!['P01','B01','C01'].every(read)||!one)return false;
                const health=one.querySelector('.zxs-disk-health');
                return health?.dataset.zxsDiskHealth==='smart'&&health.textContent.includes('/dev/nvme9n1')
                    &&!health.textContent.includes('/dev/nvme9n1p1')&&health.textContent.includes('2023-11-14T22:13:20.123Z')
                    &&health.textContent.includes('0 °C')&&/0 (hours|小时)/.test(health.textContent)
                    &&/SMART reports passed|SMART 报告通过/.test(health.textContent);

            "#,
            )?;
            disk_tree::select(browser, "B01")?;
            check(
                browser,
                "multiple-device report remains on B01",
                r#"
                const health=document.querySelector('.zxs-disk-management-card[data-slot="B01"] .zxs-disk-health');
                return health?.dataset.zxsDiskHealth==='unavailable'&&/multiple devices|多个设备/.test(health.textContent)
                    &&!health.textContent.includes('/dev/nvme9n1');
            "#,
            )?;
            disk_tree::select(browser, "C01")?;
            check(
                browser,
                "permission-denied report remains on C01",
                r#"
                const health=document.querySelector('.zxs-disk-management-card[data-slot="C01"] .zxs-disk-health');
                return health?.dataset.zxsDiskHealth==='unavailable'
                    &&/lacks permission|没有读取设备健康信息的权限/.test(health.textContent);
            "#,
            )?;
            browser.script("window.__zxsDiskHealth.mode='shared';return true")?;
            refresh_card(browser, "C01")?;
            check(
                browser,
                "directories on one disk retain the same queried-device identity",
                r#"
                const health=document.querySelector('.zxs-disk-management-card[data-slot="C01"] .zxs-disk-health');
                return window.__zxsDiskHealth.reads.some(value=>value.slot==='C01'&&value.mode==='shared')
                    &&health?.dataset.zxsDiskHealth==='smart'&&health.textContent.includes('/dev/nvme9n1');
            "#,
            )?;
            disk_tree::select(browser, "P01")?;
            check(
                browser,
                "primary keeps the same shared-device observation",
                r#"
                const health=document.querySelector('.zxs-disk-management-card[data-slot="P01"] .zxs-disk-health');
                return health?.dataset.zxsDiskHealth==='smart'&&health.textContent.includes('/dev/nvme9n1')
                    &&/SMART reports passed|SMART 报告通过/.test(health.textContent);
            "#,
            )?;
            browser.script("window.__zxsDiskHealth.mode='failed';return true")?;
            refresh_card(browser, "P01")?;
            check(
                browser,
                "a failed SMART report is attached only to its refreshed directory",
                r#"
                const one=document.querySelector('.zxs-disk-management-card[data-slot="P01"] .zxs-disk-health');
                return /SMART reports failure|SMART 报告异常/.test(one?.textContent);
            "#,
            )?;
            disk_tree::select(browser, "C01")?;
            check(
                browser,
                "another directory does not inherit the failed report",
                r#"
                const health=document.querySelector('.zxs-disk-management-card[data-slot="C01"] .zxs-disk-health');
                return /SMART reports passed|SMART 报告通过/.test(health?.textContent);
            "#,
            )?;
            browser.script("window.__zxsDiskHealth.mode='invalid';return true")?;
            refresh_card(browser, "P01")?;
            check(
                browser,
                "invalid refresh labels the retained health report as stale",
                r#"
                const card=document.querySelector('.zxs-disk-management-card[data-slot="P01"]');
                return window.__zxsDiskHealth.reads.some(value=>value.slot==='P01'&&value.mode==='invalid')
                    &&/Refresh failed|刷新未成功/.test(card?.textContent)
                    &&/SMART reports failure|SMART 报告异常/.test(card.querySelector('.zxs-disk-health')?.textContent);
            "#,
            )?;
            AdminPage::Diagnostics.open(browser)?;
            check(
                browser,
                "recovery controls are on the diagnostics page",
                r#"
                const section=document.querySelector('section[data-zxs-admin-section="diagnostics"]');
                const recovery=section.querySelector('.zxs-management-recovery');
                const buttons=[...section.querySelectorAll('button')].map(button=>button.textContent.trim());
                return recovery&&recovery.getBoundingClientRect().height>0
                    &&buttons.some(value=>['Check management status','检查管理状态'].includes(value))
                    &&!buttons.some(value=>['Management health','管理健康'].includes(value));
            "#,
            )?;
            Ok(())
        },
        || {
            browser.script(
                "window.__zxsDiskHealth?.restore();delete window.__zxsDiskHealth;return true",
            )?;
            browser.click(&browser.button("Files", "文件")?)?;
            AdminPage::Storage.open(browser)?;
            Ok(())
        },
    )?;
    println!(
        "PASS synthetic per-directory SMART attribution, shared devices, unavailable states, stale reads and diagnostics recovery controls"
    );
    Ok(())
}
