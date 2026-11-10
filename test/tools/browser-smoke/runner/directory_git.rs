//! Real manual initialization and an explicit current-repository commit.
use super::*;

mod children;
pub(super) mod fixture;
pub(super) use children::run as children;

pub(super) fn retired_request(url: &str, origin: &str) -> bool {
    let Some(relative) = url
        .strip_prefix(origin)
        .filter(|path| path.starts_with('/'))
    else {
        return false;
    };
    let (path, query) = relative.split_once('?').unwrap_or((relative, ""));
    path.ends_with("/directory-scopes")
        || path.ends_with("/directory-select")
        || (path.ends_with("/entries")
            && query
                .split('&')
                .any(|field| field.split('=').next() == Some("entry")))
}

const OBSERVE: &str = r#"
if(window.__zxsDirectoryGit)throw new Error('Directory repository observer already installed');
const original=window.fetch;
const state=window.__zxsDirectoryGit={writes:[],responses:[],reads:[],unexpected:[],restore:()=>{window.fetch=original;delete window.__zxsDirectoryGit;}};
window.fetch=async function(...args){
    const input=args[0] instanceof Request?args[0].clone():args[0];
    const request=new Request(input,args[1]),url=new URL(request.url,location.href);
    if(url.origin===location.origin&&(/\/(?:directory-scopes|directory-select)$/.test(url.pathname)||
        (url.pathname.endsWith('/entries')&&url.searchParams.has('entry'))))
        throw new Error('Retired directory request: '+url.pathname+url.search);
    const watched=url.origin===location.origin&&request.method==='POST'&&
        (/\/directory-git\//.test(url.pathname)||/\/directory-branches\/[^/]+\/commits$/.test(url.pathname));
    if(watched)state.writes.push({path:url.pathname,body:await request.clone().json()});
    else if(url.origin===location.origin&&url.pathname.startsWith('/api/')&&!['GET','HEAD'].includes(request.method))
        state.unexpected.push({method:request.method,path:url.pathname});
    const response=await Reflect.apply(original,this,args);
    if(watched)state.responses.push({status:response.status,body:await response.clone().json()});
    if(url.origin===location.origin&&request.method==='GET'&&url.pathname.endsWith('/directory-git'))
        state.reads.push({status:response.status,body:await response.clone().json()});
    return response;
};return true;
"#;

pub(super) fn initialize_and_commit(browser: &Browser) -> Result<()> {
    browser.css("[data-directory-git-initialized='false']")?;
    browser.script(OBSERVE)?;
    with_cleanup(
        || {
            let before=browser.script("return {writes:window.__zxsDirectoryGit.writes.length,commit:!!document.querySelector('[data-commit-directory]')}")?;
            if before["writes"] != 0 || before["commit"] != false {
                return Err(
                    "uninitialized directory must not auto-initialize or offer commit".into(),
                );
            }
            browser.css("[data-initialize-directory-git]:not([disabled])")?;
            browser.script("const b=document.querySelector('[data-initialize-directory-git]');b.click();b.click();return true")?;
            browser.css("[data-directory-git-initialized='true']")?;
            let initialized = browser.script("return window.__zxsDirectoryGit.writes")?;
            if initialized.as_array().map(Vec::len) != Some(1)
                || !initialized[0]["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("/directory-git/initialize"))
                || initialized[0]["body"]["path"] != ""
                || !initialized[0]["body"]["expected_namespace_head"].is_string()
            {
                return Err(format!(
                    "manual initialization did not bind one reviewed namespace: {initialized}"
                )
                .into());
            }
            browser.css(".zxs-directory-staging input:not([disabled])")?;
            browser.script("const i=document.querySelector('.zxs-directory-staging input');i.value='字'.repeat(43);i.dispatchEvent(new Event('input',{bubbles:true}));return true")?;
            if browser
                .script("return document.querySelector('[data-commit-directory]').disabled")?
                != true
            {
                return Err("directory commit message must enforce its UTF-8 byte limit".into());
            }
            for message in ["Review\twork", "Review\0work", "Review\u{85}work", "   "] {
                let input = json_codec::to_string(message)?;
                browser.script(&format!(
                    "const i=document.querySelector('.zxs-directory-staging input');i.value={input};i.dispatchEvent(new Event('input',{{bubbles:true}}));return true"
                ))?;
                browser.css("[data-directory-commit-message-error]")?;
                let invalid = browser.script(
                    "const i=document.querySelector('.zxs-directory-staging input');i.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));const b=document.querySelector('[data-commit-directory]');b.click();return {disabled:b.disabled,invalid:i.getAttribute('aria-invalid'),writes:window.__zxsDirectoryGit.writes.length}"
                )?;
                if invalid["disabled"] != true
                    || invalid["invalid"] != "true"
                    || invalid["writes"] != 1
                {
                    return Err(format!(
                        "invalid directory commit text reached publication: {invalid}"
                    )
                    .into());
                }
            }
            browser.script("const i=document.querySelector('.zxs-directory-staging input');i.value='Review initial naming repository';i.dispatchEvent(new Event('input',{bubbles:true}));return true")?;
            browser.css("[data-commit-directory]:not([disabled])")?;
            if browser.script(
                "return !!document.querySelector('[data-directory-commit-message-error]')",
            )? != false
            {
                return Err("corrected directory commit text must clear its input error".into());
            }
            if browser.script("return window.__zxsDirectoryGit.writes.length")? != 1 {
                return Err("editing a commit message must not publish".into());
            }
            browser.script("const b=document.querySelector('[data-commit-directory]');b.click();b.click();return true")?;
            browser.css(
                "[data-directory-git-staged='false'][data-directory-git-initialized='true']",
            )?;
            wait_for("manual directory commit permanent receipt", || {
                let result = browser.script("return window.__zxsDirectoryGit.responses")?;
                if result.as_array().map(Vec::len) == Some(2)
                    && result[0]["status"] == 200
                    && result[1]["status"] == 200
                {
                    Ok(())
                } else {
                    Err(format!("directory mutation receipts are not complete: {result}").into())
                }
            })?;
            let writes = browser.script("return window.__zxsDirectoryGit.writes")?;
            if writes.as_array().map(Vec::len) != Some(2)
                || !writes[1]["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("/commits"))
                || !writes[1]["body"]["expected_index_generation"].is_string()
                || !writes[1]["body"]["expected_tree_id"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("dtree_"))
                || writes[1]["body"]["message"] != "Review initial naming repository"
            {
                return Err(format!(
                    "explicit commit did not pin the reviewed tree and index: {writes}"
                )
                .into());
            }
            println!(
                "PASS manual directory Git initialization, commit text contract, explicit commit, exact tree/index generation, duplicate-click fences"
            );
            Ok(())
        },
        || {
            browser.script("window.__zxsDirectoryGit.restore();return true")?;
            Ok(())
        },
    )
}
