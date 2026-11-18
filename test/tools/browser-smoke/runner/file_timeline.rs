//! Real packaged-browser reader: a server-backed same-FILE merge input locates
//! one off-window card only after the original authorized observation returns.
use super::{Browser, Result, first_commit_tabs::{ready,until}, json, with_cleanup};
mod fixture;
const FINAL:&str="timeline-final.txt";
pub(super) struct Evidence {
    slot:String,file:String,old:String,one:String,two:String,older:String,newer:String,observation:String,
}
pub(super) fn seed(temp:&tempfile::TempDir)->Result<Evidence>{fixture::seed(temp)}
pub(super) fn run(b:&Browser, origin:&str, password:&str, e:&Evidence)->Result<()>{
    ready(b,origin,password)?;
    b.click(&b.css(&format!(".zxs-disk-entry[data-slot='{}']:not([disabled])",e.slot))?)?;
    let name=b.element("xpath",&format!("//button[contains(@class,'zxs-file-name') and normalize-space(.)='{FINAL}']"))?;
    b.click(&name)?;
    b.css("[data-zxs-lifecycle-load]:not([disabled])")?;
    b.script(&format!("window.__zxsTimelineExpected = {}; {}",
        json!({"file":e.file,"old":e.old,"one":e.one,"two":e.two,"older":e.older,"newer":e.newer,"observation":e.observation}),
        include_str!("file_timeline/probe.js")))?;
    with_cleanup(|| {
        let load="[data-zxs-lifecycle-load]:not([disabled])";
        let jump=format!("[data-zxs-lifecycle-operation='{}'] [data-zxs-related-node='{}']:not([disabled])",e.older,e.old);
        b.click(&b.css(load)?)?;
        until(b,"two original FILE merges; true edge, open page end and off-window pinned input",
            "return window.__zxsTimeline.originalProjection()")?;
        b.script("const t=window.__zxsTimeline;t.remember();return true;")?;
        b.click(&b.css(load)?)?;
        until(b,"same admitted page preserves exact event, node card and timeline scroll",
            "return window.__zxsTimeline.sameProjection()")?;
        b.click(&b.css(&jump)?)?;
        until(b,"second read is the unchanged original route after the old content returned",
            "return window.__zxsTimeline.held()")?;
        if b.script("return window.__zxsTimeline.installed()")? == true {
            return Err("anchored card installed before its original source was reauthorized".into());
        }
        b.click(&b.css("[data-zxs-lifecycle-dismiss]:not([disabled])")?)?;
        b.script("window.__zxsTimeline.release();return true;")?;
        until(b,"dismissed response neither installs nor focuses old node",
            "return window.__zxsTimeline.dismissed()")?;
        b.click(&b.css(load)?)?;
        b.click(&b.css(&jump)?)?;
        until(b,"independent second original pair has not yet changed content window",
            "return window.__zxsTimeline.held() && !window.__zxsTimeline.installed()")?;
        b.script("window.__zxsTimeline.release();return true;")?;
        until(b,"exact real original FILE content card alone receives native focus",
            "return window.__zxsTimeline.done()")?;
        Ok(())
    },|| { b.script("window.__zxsTimeline.restore();return true;")?; Ok(()) })
}
