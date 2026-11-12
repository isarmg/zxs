//! Exercise production WASM naming UI; rejected writes are captured at fetch.
use super::*;

fn enter(browser: &Browser, label: (&str, &str), selector: &str, value: &str) -> Result<String> {
    browser.click(&browser.button(label.0, label.1)?)?;
    let input = browser.css(selector)?;
    browser.keys(&input, value)?;
    browser.keys(&input, "\u{e007}")?;
    Ok(input)
}

fn no_write(browser: &Browser, expected: usize) -> Result<()> {
    if browser.script("return window.__zxsNaming.writes.length")? != json!(expected) {
        return Err("a naming mutation was submitted before explicit confirmation".into());
    }
    Ok(())
}

fn open_scope(browser: &Browser, name: &str) -> Result<()> {
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    browser.click(&browser.element(
        "xpath",
        &format!("//button[contains(@class,'zxs-file-name')][normalize-space(.)='{name}']"),
    )?)?;
    browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']",
    )?;
    Ok(())
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    // The persisted source is established by the private server-side fixture.
    // Naming still exercises the real source identity without retired Web writes.
    open_scope(browser, super::fixture::NAMING_REPOSITORY)?;
    browser.script(r#"
        const original = window.fetch;
        const unrelatedKey='zxs:browser-smoke:recovery-unrelated',unrelatedValue='keep this unrelated record';
        if(localStorage.getItem(unrelatedKey)!==null)throw new Error('Recovery probe key is already occupied');
        localStorage.setItem(unrelatedKey,unrelatedValue);
        window.__zxsNaming = {writes:[], recovery:[], unrelatedKey, unrelatedValue, restore:()=>{
            window.fetch=original;
            if(localStorage.getItem(unrelatedKey)!==unrelatedValue)throw new Error('Unrelated recovery record changed');
            localStorage.removeItem(unrelatedKey);
        }};
        window.fetch = async function(...args) {
            const request=args[0], url=new URL(request.url,location.href);
            if(request.method==='POST' && /^\/api\/file-manager\/disks\/[^/]+\/(namespace|namespace-batch|directory-branches)$/.test(url.pathname)) {
                const body=await request.clone().json();
                if(!url.pathname.endsWith('/namespace-batch')){
                    const keys=[];
                    for(let index=0;index<localStorage.length;index++){
                        const key=localStorage.key(index);
                        if(key?.startsWith('zxs:folder-create:')&&key.endsWith(':'+body.operation_id))keys.push(key);
                    }
                    if(keys.length!==1)throw new Error('Expected one retained original before naming POST');
                    const bytes=localStorage.getItem(keys[0]),retained=JSON.parse(bytes);
                    if(retained.operation!==body.operation_id||typeof retained.principal!=='string'||!retained.principal)
                        throw new Error('Retained naming identity does not match the POST');
                    window.__zxsNaming.recovery.push({key:keys[0],bytes,operation:body.operation_id});
                }
                window.__zxsNaming.writes.push({path:url.pathname,body});
                return new Response(JSON.stringify({error:{code:'NAME_COLLISION',message:'Synthetic naming rejection',retryable:false}}), {status:409,headers:{'Content-Type':'application/json'}});
            }
            return original.apply(this,args);
        }; return true;
    "#)?;
    with_cleanup(
        || {
            for original in [" draft ", "\u{a0}draft", "draft\u{2003}"] {
                let input = enter(
                    browser,
                    ("New folder", "新建文件夹"),
                    ".zxs-inline-input",
                    original,
                )?;
                wait_for("specific unmodified naming rejection", || {
                    if browser.script("const input=document.querySelector('.zxs-inline-input');return input&&!input.disabled&&/Original name:|原名称：/.test(document.querySelector('.zxs-inline-error').textContent)")? == true { Ok(()) } else { Err("name rejection not visible".into()) }
                })?;
                no_write(browser, 0)?;
                browser.keys(&input, "\u{e00c}")?;
            }
            let input = enter(
                browser,
                ("New folder", "新建文件夹"),
                ".zxs-inline-input",
                "e\u{301}",
            )?;
            browser.button("Confirm NFC name", "确认使用 NFC 名称")?;
            no_write(browser, 0)?;
            browser.keys(&input, "\u{e004}")?;
            wait_for(
                "keyboard focus reaches NFC confirmation without cancelling",
                || {
                    if browser.script("return /Confirm NFC name|确认使用 NFC 名称/.test(document.activeElement.textContent)&&!!document.querySelector('.zxs-inline-input')")? == true { Ok(()) } else { Err("NFC confirmation lost keyboard focus".into()) }
                },
            )?;
            browser.keys(
                &browser.button("Confirm NFC name", "确认使用 NFC 名称")?,
                "\u{e00c}",
            )?;
            no_write(browser, 0)?;
            enter(
                browser,
                ("New folder", "新建文件夹"),
                ".zxs-inline-input",
                "e\u{301}",
            )?;
            browser.button("Confirm NFC name", "确认使用 NFC 名称")?;
            browser.click(&browser.css(".zxs-file-search input")?)?;
            wait_for("leaving the naming editor cancels the candidate", || {
                if browser.script("return !document.querySelector('.zxs-inline-input')")? == true {
                    Ok(())
                } else {
                    Err("blur retained an unsubmitted candidate".into())
                }
            })?;
            no_write(browser, 0)?;
            for (index, original, candidate) in
                [(0, "e\u{301}", "é"), (1, "\u{1100}\u{1161}", "가")]
            {
                enter(
                    browser,
                    ("New folder", "新建文件夹"),
                    ".zxs-inline-input",
                    original,
                )?;
                let confirm = browser.button("Confirm NFC name", "确认使用 NFC 名称")?;
                no_write(browser, index)?;
                browser.click(&confirm)?;
                wait_for("only confirmed candidate reaches the request", || {
                    let result = browser
                        .script("return window.__zxsNaming.writes.map(write=>write.body.path)")?;
                    if result.as_array().is_some_and(|paths| {
                        paths.len() == index + 1
                            && paths[index]
                                .as_str()
                                .is_some_and(|path| path.ends_with(&format!("/{candidate}")))
                    }) {
                        Ok(())
                    } else {
                        Err(format!("wrong normalized request: {result}").into())
                    }
                })?;
                wait_for("rejected candidate keeps editable draft", || {
                    if browser.script("const input=document.querySelector('.zxs-inline-input');return input&&!input.disabled")? == true { Ok(()) } else { Err("rejected draft not editable".into()) }
                })?;
                browser.keys(&browser.css(".zxs-inline-input")?, "\u{e00c}")?;
            }
            let input = enter(
                browser,
                ("New folder", "新建文件夹"),
                ".zxs-inline-input",
                "inner space",
            )?;
            wait_for("valid name request preserves bytes", || {
                if browser.script(
                    "return window.__zxsNaming.writes.at(-1)?.body.path.endsWith('/inner space')",
                )? == true
                {
                    Ok(())
                } else {
                    Err("valid original name not submitted".into())
                }
            })?;
            wait_for("valid name rejected without pending fence", || {
                if browser.script("return !document.querySelector('.zxs-inline-input').disabled")?
                    == true
                {
                    Ok(())
                } else {
                    Err("request still pending".into())
                }
            })?;
            browser.keys(&input, "\u{e00c}")?;
            let branch = enter(
                browser,
                ("Create folder branch", "创建目录分支"),
                ".zxs-branch-name",
                "e\u{301}",
            )?;
            browser.button("Confirm NFC name", "确认使用 NFC 名称")?;
            no_write(browser, 3)?;
            browser.click(&browser.button("Confirm NFC name", "确认使用 NFC 名称")?)?;
            wait_for("branch uses only explicitly confirmed candidate", || {
                if browser.script("return window.__zxsNaming.writes.length===4&&window.__zxsNaming.writes[3].body.name==='é'&&!document.querySelector('.zxs-branch-name').disabled")? == true { Ok(()) } else { Err("branch confirmation request missing".into()) }
            })?;
            browser.keys(&branch, "\u{e00c}")?;
            wait_for(
                "rejected originals cleaned without touching unrelated browser storage",
                || {
                    let recovery = browser.script("const p=window.__zxsNaming;return {retained:p.recovery.length,remaining:p.recovery.filter(r=>localStorage.getItem(r.key)!==null).length,unrelated:localStorage.getItem(p.unrelatedKey)===p.unrelatedValue}")?;
                    if recovery["retained"] == 4
                        && recovery["remaining"] == 0
                        && recovery["unrelated"] == true
                    {
                        Ok(())
                    } else {
                        Err(format!("original recovery cleanup was not exact: {recovery}").into())
                    }
                },
            )?;
            println!(
                "PASS inline NFC/Hangul focus, cancellation, explicit folder/branch confirmation and retained-original cleanup"
            );
            let checkbox=browser.element("xpath", "//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']]//input[@type='checkbox']")?;
            browser.click(&checkbox)?;
            browser.click(&browser.button("Rename", "重命名")?)?;
            // The private fixture established this real source. Batch review
            // must reuse its identity rather than create another scope.
            wait_for("registered and selected batch source scope", || {
                let scope = browser.script(r#"
                    const editor=document.querySelector('.zxs-batch-editor');
                    const sources=[...editor.querySelectorAll('.zxs-batch-scopes input')];
                    const input=editor.querySelector('.zxs-batch-entry input');
                    return {ready:input&&!input.disabled&&sources.length===1&&sources[0].checked&&!sources[0].disabled,
                        sources:sources.length,error:editor.querySelector('.zxs-inline-error').textContent};
                "#)?;
                if scope["ready"] == true {
                    Ok(())
                } else {
                    Err(format!("registered source scope is not ready: {scope}").into())
                }
            })?;
            no_write(browser, 4)?;
            println!(
                "PASS persisted source repository and selected-entry read before batch NFC review"
            );
            let codepoints = browser.script(&format!(
                "const input=document.querySelector('.zxs-batch-entry input');input.value={};input.dispatchEvent(new Event('input',{{bubbles:true}}));return Array.from(input.value,ch=>ch.codePointAt(0))",
                json!("e\u{301}")
            ))?;
            if codepoints != json!([0x65, 0x301]) {
                return Err(format!("batch NFC input must remain decomposed: {codepoints}").into());
            }
            browser.click(&browser.button("Apply selected operation", "执行所选操作")?)?;
            browser.button("Confirm NFC name", "确认使用 NFC 名称")?;
            no_write(browser, 4)?;
            browser.click(&browser.button("Confirm NFC name", "确认使用 NFC 名称")?)?;
            wait_for("batch submits only the confirmed target name", || {
                if browser.script("const p=window.__zxsNaming.writes;return p.length===5&&p[4].body.items.length===1&&p[4].body.items[0].source.path==='browser-smoke.txt'&&p[4].body.items[0].target.path==='é'&&!document.querySelector('.zxs-batch-entry input').disabled")? == true { Ok(()) } else { Err("batch confirmation request missing".into()) }
            })?;
            browser.click(&browser.button("Cancel", "取消")?)?;
            // Registering the source refreshes the listing and clears its
            // selection. Cleanup must set a postcondition, not invert it.
            let checkbox = browser.css(".zxs-entry-checkbox")?;
            let checked = browser
                .command("GET", &format!("/element/{checkbox}/selected"), None)?
                .as_bool()
                .ok_or("WebDriver selected response must be a boolean")?;
            println!("INFO naming handoff before cleanup: checked={checked}");
            if checked {
                browser.click(&checkbox)?;
            }
            wait_for("naming hands off an unchecked row and closed batch", || {
                let snapshot = browser.script("const check=document.querySelector('.zxs-entry-checkbox');return {checked:check?.checked,batchOpen:!!document.querySelector('.zxs-batch-editor')}")?;
                if snapshot["checked"] == false && snapshot["batchOpen"] == false {
                    println!("PASS naming handoff: {snapshot}");
                    Ok(())
                } else {
                    Err(format!("naming handoff is not clean: {snapshot}").into())
                }
            })?;
            Ok(())
        },
        || {
            browser.script("window.__zxsNaming.restore();delete window.__zxsNaming;for(const row of document.querySelector('.zxs-error-center')?.children||[]){if(row.textContent.includes('Synthetic naming rejection'))row.querySelector('button').click();}return true")?;
            wait_for("synthetic naming notices dismissed", || {
                if browser.script("return !document.querySelector('.zxs-error-center')?.textContent.includes('Synthetic naming rejection')")? == true { Ok(()) } else { Err("synthetic naming notice remains".into()) }
            })
        },
    )?;
    open_scope(browser, "browser-fixtures")?;
    println!(
        "PASS production WASM preserves invalid input; NFC/Hangul cancel sends no POST and explicit confirmation submits exact candidate (synthetic 409 replies)"
    );
    Ok(())
}
