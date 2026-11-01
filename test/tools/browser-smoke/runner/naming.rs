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

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(r#"
        const original = window.fetch;
        window.__zxsNaming = {writes:[], restore:()=>window.fetch=original};
        window.fetch = async function(...args) {
            const request=args[0], url=new URL(request.url,location.href);
            if(request.method==='POST' && /^\/api\/file-manager\/disks\/[^/]+\/(namespace|namespace-batch|directory-branches)$/.test(url.pathname)) {
                window.__zxsNaming.writes.push({path:url.pathname,body:await request.clone().json()});
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
            println!(
                "PASS inline NFC/Hangul focus, cancellation and explicit folder/branch confirmation"
            );
            let checkbox=browser.element("xpath", "//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='browser-smoke.txt']]//input[@type='checkbox']")?;
            browser.click(&checkbox)?;
            browser.click(&browser.button("Rename", "重命名")?)?;
            // Seeded file commits do not register a directory composition. The
            // earlier branch-name POST was deliberately rejected, so register
            // the source through the real UI before asking for a rename.
            browser.click(&browser.button(
                "Register source folder branch records",
                "登记源目录分支记录",
            )?)?;
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
                "PASS real source-scope registration and selected-entry read before batch NFC review"
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
            browser.click(&checkbox)?;
            Ok(())
        },
        || {
            browser.script("window.__zxsNaming.restore();delete window.__zxsNaming;for(const row of document.querySelector('.zxs-error-center')?.children||[]){if(row.textContent.includes('Synthetic naming rejection'))row.querySelector('button').click();}return true")?;
            wait_for("synthetic naming notices dismissed", || {
                if browser.script("return !document.querySelector('.zxs-error-center')?.textContent.includes('Synthetic naming rejection')")? == true { Ok(()) } else { Err("synthetic naming notice remains".into()) }
            })
        },
    )?;
    println!(
        "PASS production WASM preserves invalid input; NFC/Hangul cancel sends no POST and explicit confirmation submits exact candidate (synthetic 409 replies)"
    );
    Ok(())
}
