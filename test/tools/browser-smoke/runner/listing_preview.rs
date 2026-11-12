//! A listing may not grant a preview that its request never selected.
//! Only this rejected response is synthetic; the request and root listing are real.
use super::*;

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(
        r#"
        const original=window.fetch;
        const p=window.__zxsListingPreview={writes:0,rejected:0,pending:true,
            restore:()=>{window.fetch=original;}};
        window.fetch=async function(...args){
            const input=args[0],url=new URL(typeof input==='string'?input:input.url,location.href);
            const method=(args[1]?.method||input.method||'GET').toUpperCase();
            if(url.origin!==location.origin)return original.apply(this,args);
            if(url.pathname.startsWith('/api/')&&!['GET','HEAD'].includes(method))p.writes++;
            const response=await original.apply(this,args);
            if(method==='GET'&&url.pathname==='/api/file-manager/disks/P01/entries'
                &&url.searchParams.get('path')===''&&response.ok&&p.pending){
                const body=await response.clone().json();
                if(url.searchParams.has('directory_branch_id')||body.slot_id!=='P01'
                    ||body.path!==''||body.writable!==true||body.backup||body.preview)
                    throw new Error('preview rejection requires a real writable root listing');
                body.preview=true;body.writable=false;p.pending=false;p.rejected++;
                return new Response(JSON.stringify(body),{status:200,
                    headers:{'Content-Type':'application/json'}});
            }
            return response;
        };return true;
    "#,
    )?;
    with_cleanup(
        || {
            browser.click(&browser.button("Files", "文件")?)?;
            browser.click(&browser.button("All disks", "所有硬盘")?)?;
            browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
            wait_for("unsolicited listing preview is rejected without installing entries", || {
                if browser.script(
                    r#"
                    const p=window.__zxsListingPreview;
                    return p.rejected===1&&p.writes===0
                        &&document.querySelector('.zxs-error-center')?.textContent
                            .includes('INVALID_DIRECTORY_PAGE')
                        &&document.querySelector('.zxs-file-manager')
                            ?.getAttribute('aria-busy')==='false'
                        &&document.querySelectorAll('.zxs-file-table .zxs-file-name').length===0;
                "#,
                )? == true {
                    Ok(())
                } else {
                    Err("unsolicited preview was not rejected before entry installation".into())
                }
            })?;
            browser.click(&browser.element(
                "xpath",
                "//aside[contains(@class,'zxs-error-center')]/div[contains(@class,'zxs-inline-feedback')][contains(.,'INVALID_DIRECTORY_PAGE')]//button[@aria-label='Dismiss error' or @aria-label='关闭错误提示']",
            )?)?;
            browser.click(&browser.css(".zxs-header-refresh:not([disabled])")?)?;
            Ok(())
        },
        || {
            browser.script(
                "if(window.__zxsListingPreview){window.__zxsListingPreview.restore();delete window.__zxsListingPreview;}return true",
            )?;
            Ok(())
        },
    )?;
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    open_fixture_directory(browser)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?)?;
    println!("PASS unsolicited listing preview is rejected without installing entries or writing");
    Ok(())
}
