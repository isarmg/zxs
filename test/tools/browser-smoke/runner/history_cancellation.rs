//! Synthetic history delivery through production WASM, seeded from real wire
//! responses. Gates deliberately ignore abort so late completions still run.
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

fn select(browser: &Browser, name: &str) -> Result<()> {
    browser.click(&browser.element(
        "xpath",
        &format!("//button[contains(@class,'zxs-file-name')][normalize-space(.)='{name}']"),
    )?)
}

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(
        r#"
        const original = window.fetch;
        const probe = window.__zxsHistoryCancellation = {
            pending: [], seed: null,
            restore: () => {
                window.fetch = original;
                for (const pending of probe.pending) pending.release();
            }
        };
        window.fetch = async function(...args) {
            const request = args[0];
            const url = new URL(typeof request === 'string' ? request : request.url, location.href);
            if (!/^\/api\/file-manager\/files\/[^/]+\/history$/.test(url.pathname)) {
                return original.apply(this, args);
            }
            let release;
            const gate = new Promise(resolve => release = resolve);
            const pending = {signal: request.signal, url: url.href, release,
                ready: false, delivered: false, note: 'history-owner-' + probe.pending.length};
            probe.pending.push(pending);
            if (!probe.seed) {
                const response = await original.apply(this, args);
                if (!response.ok) throw new Error('real history seed failed');
                probe.seed = await response.json();
            }
            const history = {...probe.seed, context: {...probe.seed.context, note: pending.note}};
            pending.ready = true;
            await gate;
            // Fully buffered bytes let decoding continue in the same microtask turn.
            pending.delivered = true;
            return new Response(JSON.stringify(history), {status: 200,
                headers: {'Content-Type': 'application/json'}});
        };
        return true;
    "#,
    )?;
    with_cleanup(
        || {
            browser.click(&browser.button("Refresh", "刷新")?)?;
            check(
                browser,
                "real listing refresh before history cancellation fixture",
                "return document.querySelectorAll('.zxs-file-table tbody tr').length===1&&!document.querySelector('.zxs-header-refresh').disabled",
            )?;
            select(browser, "browser-smoke.txt")?;
            check(
                browser,
                "history A is held before delivery",
                "const p=window.__zxsHistoryCancellation.pending;return p.length===1&&p[0].ready&&!p[0].signal.aborted",
            )?;
            browser.click(&browser.button("Files", "文件")?)?;
            browser.click(&browser.button("Refresh", "刷新")?)?;
            check(
                browser,
                "refresh replaces A with held B and aborts A",
                "const p=window.__zxsHistoryCancellation.pending;return p.length===2&&p[1].ready&&p[0].signal.aborted&&!p[1].signal.aborted&&p[0].url===p[1].url",
            )?;
            browser.click(&browser.button("Details", "详情")?)?;
            browser.script("window.__zxsHistoryCancellation.pending[0].release();return true")?;
            check(
                browser,
                "late A cannot release B or replace its pending metadata",
                "const p=window.__zxsHistoryCancellation.pending;return p[0].delivered&&!p[1].signal.aborted&&document.querySelector('.zxs-header-refresh').disabled&&!document.querySelector('.zxs-entry-content').textContent.includes(p[0].note)&&document.querySelectorAll('.zxs-version-node').length===0",
            )?;
            browser.click(&browser.button("Files", "文件")?)?;
            select(browser, "browser-smoke.txt")?;
            check(
                browser,
                "late A left B owned so reselecting the real file aborts B",
                "const p=window.__zxsHistoryCancellation.pending,a=new URL(p[1].url),b=p[2]&&new URL(p[2].url);return p.length===3&&p[2].ready&&p[1].signal.aborted&&!p[2].signal.aborted&&a.href===b.href&&document.querySelector('.zxs-entry-content h2').textContent==='browser-smoke.txt'",
            )?;
            browser.script("window.__zxsHistoryCancellation.pending[1].release();return true")?;
            check(
                browser,
                "late B preserves real-file C selection and loading owner",
                "const p=window.__zxsHistoryCancellation.pending;return p[1].delivered&&!p[2].signal.aborted&&document.querySelector('.zxs-header-refresh').disabled&&document.querySelector('.zxs-entry-content h2').textContent==='browser-smoke.txt'&&!document.querySelector('.zxs-entry-content').textContent.includes(p[1].note)",
            )?;
            browser.script("window.__zxsHistoryCancellation.pending[2].release();return true")?;
            check(
                browser,
                "only current C installs metadata and clears loading",
                "const p=window.__zxsHistoryCancellation.pending,text=document.querySelector('.zxs-entry-content').textContent;return p[2].delivered&&text.includes(p[2].note)&&!text.includes(p[0].note)&&!text.includes(p[1].note)&&document.querySelector('.zxs-entry-content h2').textContent==='browser-smoke.txt'&&!document.querySelector('.zxs-header-refresh').disabled&&document.querySelectorAll('.zxs-version-node').length>=2",
            )?;
            browser.click(&browser.button("Files", "文件")?)?;
            select(browser, "browser-smoke.txt")?;
            check(
                browser,
                "history D is held before leaving its directory",
                "const p=window.__zxsHistoryCancellation.pending;return p.length===4&&p[3].ready&&!p[3].signal.aborted",
            )?;
            browser.click(&browser.button("Files", "文件")?)?;
            browser.click(&browser.button("All disks", "所有硬盘")?)?;
            check(
                browser,
                "navigation clears selection and aborts D",
                "return window.__zxsHistoryCancellation.pending[3].signal.aborted&&document.querySelectorAll('.zxs-disk-entry').length===3",
            )?;
            browser.script("window.__zxsHistoryCancellation.pending[3].release();return true")?;
            check(
                browser,
                "late D cannot restore details after navigation",
                "return window.__zxsHistoryCancellation.pending[3].delivered&&document.querySelectorAll('.zxs-disk-entry').length===3&&!document.querySelector('.zxs-entry-content,.zxs-version-node')",
            )?;
            Ok(())
        },
        || {
            browser.script("if(window.__zxsHistoryCancellation){window.__zxsHistoryCancellation.restore();delete window.__zxsHistoryCancellation;}return true")?;
            Ok(())
        },
    )?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    open_fixture_directory(browser)?;
    println!(
        "PASS synthetic history AbortSignal replacement, late completion ownership, real-file selection and navigation cleanup"
    );
    Ok(())
}

// Reuse the real status page and Select: no test-only product entry point.
pub(super) fn monitor_range(browser: &Browser) -> Result<()> {
    browser.script(
        r#"
        const original = window.fetch;
        const probe = window.__zxsMonitorCancellation = {
            pending: [], seed: null, started: false,
            restore: () => {
                window.fetch = original;
                for (const pending of probe.pending) pending.release();
            }
        };
        window.fetch = async function(...args) {
            const request = args[0];
            const url = new URL(typeof request === 'string' ? request : request.url, location.href);
            if (url.pathname !== '/api/system/history' ||
                (!probe.started && url.searchParams.get('range') !== '30min')) {
                return original.apply(this, args);
            }
            probe.started = true;
            let release;
            const gate = new Promise(resolve => release = resolve);
            const pending = {signal: request.signal, release, ready: false, delivered: false};
            probe.pending.push(pending);
            if (!probe.seed) {
                const response = await original.apply(this, args);
                if (!response.ok) throw new Error('real monitor history seed failed');
                probe.seed = await response.json();
            }
            pending.ready = true;
            await gate;
            pending.delivered = true;
            return new Response(JSON.stringify(probe.seed), {
                status: 200, headers: {'Content-Type': 'application/json'}
            });
        };
        return true;
    "#,
    )?;
    with_cleanup(
        || {
            for index in 0..21 {
                let range = if index % 2 == 0 { "30min" } else { "1min" };
                browser.click(&browser.css(".zxs-monitor-range .zxs-select-trigger")?)?;
                browser.click(&browser.element(
                    "xpath",
                    &format!("//div[@role='option'][normalize-space(.)='{range}']"),
                )?)?;
                check(
                    browser,
                    "only current monitor history transport survives range replacement",
                    &format!(
                        "const p=window.__zxsMonitorCancellation.pending;return p.length==={}&&p.every((r,i)=>r.ready&&r.signal.aborted===(i<p.length-1))",
                        index + 1
                    ),
                )?;
            }
            browser.script(
                "for(const p of window.__zxsMonitorCancellation.pending.slice(0,-1))p.release();return true",
            )?;
            check(
                browser,
                "late cancelled monitor reads do not settle current range",
                "const p=window.__zxsMonitorCancellation.pending;return p.slice(0,-1).every(r=>r.delivered)&&!p.at(-1).signal.aborted&&Array.from(document.querySelectorAll('.zxs-chart-empty')).some(e=>/Loading history|正在读取历史/.test(e.textContent))",
            )?;
            browser
                .script("window.__zxsMonitorCancellation.pending.at(-1).release();return true")?;
            check(
                browser,
                "current monitor history settles after delayed delivery",
                "return window.__zxsMonitorCancellation.pending.at(-1).delivered&&!Array.from(document.querySelectorAll('.zxs-chart-empty')).some(e=>/Loading history|正在读取历史/.test(e.textContent))",
            )?;
            check(
                browser,
                "loaded CPU history has a real SVG before heartbeat identity check",
                "const svg=document.querySelector('[data-metric=cpu] .zxs-metric-chart svg');if(!svg)return false;const p=window.__zxsMonitorCancellation;p.svg=svg;p.completed=p.pending.length;return true",
            )?;
            check(
                browser,
                "next heartbeat starts while keeping the loaded chart node",
                "const p=window.__zxsMonitorCancellation;return p.pending.length>p.completed&&p.pending.at(-1).ready",
            )?;
            browser.command("POST", "/execute/async", Some(json!({"script":
                "const done=arguments[0];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)));",
                "args":[]})))?;
            browser.script(
                "const p=window.__zxsMonitorCancellation;if(document.querySelector('[data-metric=cpu] .zxs-metric-chart svg')!==p.svg)throw new Error('loading heartbeat replaced unchanged history SVG');p.pending.at(-1).release();return true",
            )?;
            check(
                browser,
                "equal heartbeat response is delivered",
                "return window.__zxsMonitorCancellation.pending.at(-1).delivered",
            )?;
            browser.command("POST", "/execute/async", Some(json!({"script":
                "const done=arguments[0];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)));",
                "args":[]})))?;
            browser.script(
                "if(document.querySelector('[data-metric=cpu] .zxs-metric-chart svg')!==window.__zxsMonitorCancellation.svg)throw new Error('equal heartbeat replaced unchanged history SVG');return true",
            )?;
            Ok(())
        },
        || {
            browser.script("if(window.__zxsMonitorCancellation){window.__zxsMonitorCancellation.restore();delete window.__zxsMonitorCancellation;}return true")?;
            Ok(())
        },
    )?;
    println!(
        "PASS monitor history range replacement cancels obsolete reads; equal heartbeats preserve loaded SVG identity"
    );
    Ok(())
}
