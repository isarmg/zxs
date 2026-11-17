//! A second independently logged-in view selects the deliberately unconsumed
//! fixture file. Its actual UI must queue on the first account's original; it
//! never receives a fabricated server result and must never send its own POST.
use super::{Browser, Result, SKIP, choose, with_cleanup};
use crate::{
    first_commit_tabs::{ready, switch, until},
    json,
};

pub(super) struct Peer {
    first: String,
    second: String,
}

pub(super) fn with_peer(
    b: &Browser,
    origin: &str,
    password: &str,
    slot: &str,
    exercise: impl FnOnce(&Peer) -> Result<()>,
) -> Result<()> {
    let first = b
        .command("GET", "/window", None)?
        .as_str()
        .ok_or("missing chooser tab")?
        .to_owned();
    let second = b.command("POST", "/window/new", Some(json!({"type":"tab"})))?["handle"]
        .as_str()
        .ok_or("missing competing tab")?
        .to_owned();
    let peer = Peer { first, second };
    with_cleanup(
        || {
            switch(b, &peer.second)?;
            ready(b, origin, password)?;
            b.click(&b.css(&format!(
                ".zxs-disk-entry[data-slot='{slot}']:not([disabled])"
            ))?)?;
            choose(b, "pick-right", SKIP)?;
            b.keys(
                &b.css("[data-zxs-directory-snapshot-state=head] input[type=text]")?,
                "must remain unsubmitted",
            )?;
            b.script(r#"
            const native=window.fetch,state=window.__zxsCompetingOriginal={posts:0};
            window.fetch=function(...args){
              const r=args[0],u=new URL(r.url||r,location.href);
              if(u.origin===location.origin&&u.pathname==='/api/directory-snapshots/commits'&&(args[1]?.method||r.method)==='POST')state.posts++;
              return native.apply(this,args);
            };return true;
        "#)?;
            switch(b, &peer.first)?;
            exercise(&peer)
        },
        || {
            switch(b, &peer.second)?;
            b.command("DELETE", "/window", None)?;
            switch(b, &peer.first)
        },
    )
}

impl Peer {
    pub(super) fn contend(&self, b: &Browser) -> Result<()> {
        const ACCOUNT: &str = "zxs:directory-snapshot:smoke-admin:";
        until(
            b,
            "real nested publication reached server, authentic reply not yet delivered",
            "const s=window.__zxsNestedSelection;return s?.held===true&&s.original!==null&&s.receipt&&typeof s.release==='function'",
        )?;
        let captured = b.script(
            "const s=window.__zxsNestedSelection;return [s.key,s.original,s.replacement]",
        )?;
        let key = captured[0].as_str().ok_or("missing real original key")?;
        let saved = captured[1].as_str().ok_or("missing original bytes")?;
        let replacement = captured[2].as_str().ok_or("missing fixture replacement")?;
        with_cleanup(
            || {
                switch(b, &self.second)?;
                b.click(&b.css("[data-zxs-directory-snapshot-commit]:not([disabled])")?)?;
                until(
                    b,
                    "different real selection queues without publishing or replacing the held original",
                    &format!(
                        "return navigator.locks.query().then(q=>q.held.some(x=>x.name==={0}&&x.mode==='exclusive')&&q.pending.some(x=>x.name==={0})&&window.__zxsCompetingOriginal.posts===0&&localStorage.getItem({1})==={2})",
                        json!(ACCOUNT),
                        json!(key),
                        json!(saved)
                    ),
                )?;
                // The *application* is queued. A separate fixture write does not
                // cooperate with Web Locks; only compare/remove can protect its bytes.
                if b.script(&format!("if(localStorage.getItem({0})!=={1})return false;localStorage.setItem({0},{2});return true",json!(key),json!(saved),json!(replacement)))? != true {
                return Err("competing tab did not find the first tab's exact retained bytes".into());
            }
                switch(b, &self.first)?;
                b.script("window.__zxsNestedSelection.release();return true")?;
                switch(b, &self.second)?;
                until(
                    b,
                    "second queued operation stays unsent after acquiring the lock and rereading the retained original",
                    &format!(
                        "return navigator.locks.query().then(q=>!q.held.some(x=>x.name==={0})&&!q.pending.some(x=>x.name==={0})&&window.__zxsCompetingOriginal.posts===0&&localStorage.getItem({1})==={2}&&!!document.querySelector('[data-zxs-directory-snapshot-recovery] [role=alert]'))",
                        json!(ACCOUNT),
                        json!(key),
                        json!(replacement)
                    ),
                )
            },
            || switch(b, &self.first),
        )
    }
}
