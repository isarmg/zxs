//! Compare the existing status panel with its real authenticated HTTP response.
use super::{Browser, Result, wait_for, with_cleanup};

pub(super) fn run(browser: &Browser) -> Result<()> {
    browser.script(r#"
        window.__scrubFetch = window.fetch;
        window.__scrubStatus = null;
        window.fetch = async function(...args) {
            const response = await Reflect.apply(window.__scrubFetch, this, args);
            const input = args[0];
            const path = new URL(typeof input === 'string' ? input : input.url, location.href).pathname;
            if (path === '/api/admin/scrub') {
                window.__scrubStatus = {status: response.status, body: await response.clone().json()};
            }
            return response;
        };
        return true;
    "#)?;
    with_cleanup(
        || {
            browser.click(&browser.button("Status", "状态")?)?;
            wait_for("real full-integrity monitoring projection", || {
                if browser.script(r#"
                const observed = window.__scrubStatus;
                const panel = document.querySelector('[data-integrity-scrub]');
                if (!observed || observed.status !== 200 || !panel) return false;
                const status = observed.body;
                const label = panel.querySelector('[data-integrity-freshness]');
                if (label?.dataset.integrityFreshness !== status.freshness) return false;
                if (status.last_complete_success_at && !panel.textContent.includes(status.last_complete_success_at)) return false;
                if (status.phase.state === 'running' && (!panel.textContent.includes(status.phase.bytes_read) || !panel.textContent.includes(status.phase.expected_payload_bytes))) return false;
                return /not full-pool integrity checks|不等于全池完整性检查/.test(panel.textContent);
            "#)? == true { Ok(()) } else { Err("integrity panel has not consumed the real service status".into()) }
            })?;
            println!(
                "PASS real authenticated full-integrity monitoring, separate from storage readiness"
            );
            Ok(())
        },
        || {
            browser.script("window.fetch = window.__scrubFetch; delete window.__scrubFetch; delete window.__scrubStatus; return true")?;
            Ok(())
        },
    )
}
