//! Cold browser state only, not fabricated server replies. A valid local
//! description with a different note shares the scoped receipt lookup but
//! must never be reported as the genuinely committed full directory command.
use super::{
    Browser, DirectoryDraftOutcome, DirectoryDraftReceipt, OperationId, Result, with_cleanup,
};
use crate::{
    first_commit_tabs::{install, ready, until},
    json,
};

const TRACE: &str = r#"
(()=>{
const native=window.fetch,s=window.__zxsRecoveredDirectory={gets:[],posts:[]};
window.fetch=async function(...args){
  const r=args[0],u=new URL(r.url||r,location.href),method=args[1]?.method||r.method||'GET';
  if(u.origin!==location.origin||!u.pathname.startsWith('/api/directory-snapshots/commits'))return native.apply(this,args);
  const result=await native.apply(this,args);
  if(method==='GET'&&u.pathname.startsWith('/api/directory-snapshots/commits/'))s.gets.push([u.pathname.split('/').pop(),result.status]);
  if(method==='POST'&&u.pathname==='/api/directory-snapshots/commits'){
    const value=await result.clone().json(),row={status:result.status};if(result.ok)row.node=value.result.snapshot.node_id;else row.code=value.error.code;
    s.posts.push(row);
  }
  return result;
};
})();
"#;

pub(super) fn after_terminal(
    b: &Browser,
    origin: &str,
    password: &str,
    operation: OperationId,
    receipt: &DirectoryDraftReceipt,
) -> Result<()> {
    let node = match receipt.result {
        DirectoryDraftOutcome::Changed { snapshot }
        | DirectoryDraftOutcome::Unchanged { snapshot } => snapshot.node_id.to_string(),
    };
    // Use only this private, non-secret temporary fixture's journal key.
    let captured=b.script(r#"const s=window.__zxsNestedSelection;if(localStorage.getItem(s.key)!==null)return null;
      const altered=JSON.parse(s.original);altered.command.request.message='different valid note';
      const changed=JSON.stringify(altered);localStorage.setItem(s.key,changed);return [s.key,s.original,changed];"#)?;
    let key = captured[0]
        .as_str()
        .ok_or("no previously confirmed local directory key")?;
    let exact = captured[1].as_str().ok_or("no previous original text")?;
    let changed = captured[2]
        .as_str()
        .ok_or("no valid changed local description")?;
    let hook = install(b, TRACE)?;
    with_cleanup(
        || {
            let lookup = || {
                ready(b, origin, password)?;
                until(
                    b,
                    "a fresh page only reads its recovered directory node; no original POST is inferred",
                    &format!(
                        "const s=window.__zxsRecoveredDirectory;return s?.gets.length===1&&s.gets[0][0]==={}&&s.gets[0][1]===200&&s.posts.length===0&&document.querySelector('[data-zxs-directory-scoped-receipt]')?.textContent.includes({})&&!document.querySelector('[data-zxs-directory-snapshot-cleanup]')",
                        json!(operation),
                        json!(node)
                    ),
                )
            };
            lookup()?;
            if b.script(&format!(
                "return localStorage.getItem({})==={}",
                json!(key),
                json!(changed)
            ))? != true
            {
                return Err("a scoped GET cleared a different complete local command".into());
            }
            let retry = "[data-zxs-directory-snapshot-retry]:not([disabled])";
            b.click(&b.css(retry)?)?;
            until(
                b,
                "a real explicit retry rejects the changed command without cleaning it",
                &format!(
                    "const s=window.__zxsRecoveredDirectory;return s.posts.length===1&&s.posts[0].status>=400&&s.posts[0].status<500&&s.posts[0].code==='IDEMPOTENCY_KEY_REUSED'&&localStorage.getItem({})==={}&&!!document.querySelector('[data-zxs-directory-snapshot-recovery] [role=alert]')",
                    json!(key),
                    json!(changed)
                ),
            )?;
            if b.script(&format!("if(localStorage.getItem({0})!=={1})return false;localStorage.setItem({0},{2});return true",json!(key),json!(changed),json!(exact)))? != true {
            return Err("restored directory source no longer owns its changed fixture value".into());
        }
            lookup()?;
            b.click(&b.css(retry)?)?;
            until(
                b,
                "only the exact original explicitly retried returns the same node and clears its bytes",
                &format!(
                    "const s=window.__zxsRecoveredDirectory;return s.posts.length===1&&s.posts[0].status===200&&s.posts[0].node==={}&&localStorage.getItem({})===null",
                    json!(node),
                    json!(key)
                ),
            )
        },
        || {
            b.cdp(
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({"identifier":hook}),
            )?;
            Ok(())
        },
    )
}
