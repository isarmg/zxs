//! Opt-in real foreign-review browser fixture. Never executes a foreign import.
//! Delivery faults delay/drop original responses, not server data or receipts.
use super::*;
mod fixture;
mod scenario;
mod session;
use fixture::Fixture;

fn check(browser: &Browser, label: &str, script: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}
fn audit(browser: &Browser, phase: &str, source: &Value) -> Result<Value> {
    let histories = source["histories"]
        .as_array()
        .ok_or("real source omitted histories")?;
    let groups = histories
        .iter()
        .map(|history| {
            Ok(json!({
                "id":history["group_id"], "history":history["history_tree_id"],
                "origin":history["origin"]["disk_id"],
                "missing":history["temporarily_unavailable_versions"].as_array()
                    .ok_or("real source omitted missing-body inventory")?.len(),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let script = format!(
        "const validate=(()=>{{{}}})(); const snapshot=(function(){{{}}}).apply(null,arguments);return {{snapshot,errors:validate(snapshot)}};",
        include_str!("foreign_review/accessibility_model.js"),
        include_str!("foreign_review/accessibility_snapshot.js")
    );
    let report = browser.command(
        "POST",
        "/execute/sync",
        Some(json!({
            "script":script, "args":[{"phase":phase,"expectedGroups":groups}],
        })),
    )?;
    if report["errors"] != json!([]) {
        return Err(format!("foreign review accessibility ({phase}): {report}").into());
    }
    Ok(report)
}
fn visit(browser: &Browser, server_port: u16, password: &str) -> Result<()> {
    wait_for("real foreign-review daemon ready", || {
        let (status, bytes) = http(server_port, "GET", "/api/management/status", None)?;
        let value: Value = json_codec::from_slice(&bytes)?;
        if status == 200
            && value["authentication"] == "ready"
            && value["storage"]["state"] == "ready"
            && value["storage"]["recovering"] == false
        {
            Ok(())
        } else {
            Err(format!("foreign-review fixture not ready: HTTP {status}: {value}").into())
        }
    })?;
    browser.command("DELETE", "/cookie", None)?;
    browser.command(
        "POST",
        "/url",
        Some(json!({"url":format!("http://127.0.0.1:{server_port}")})),
    )?;
    browser.keys(
        &browser.css("input[autocomplete='username']:not([readonly])")?,
        fixture::PRINCIPAL,
    )?;
    browser.keys(
        &browser.css("input[autocomplete='current-password']")?,
        password,
    )?;
    browser.click(&browser.button("Sign in", "登录")?)?;
    browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
    Ok(())
}
pub(super) fn run(
    browser: &Browser,
    daemon: &Path,
    web: Option<&Path>,
    source_pool: &Path,
    root: &Path,
    password: &str,
) -> Result<()> {
    let fixture = Fixture::new(root, source_pool)?;
    let artifacts = PathBuf::from("artifacts/browser-smoke/foreign-review");
    fs::create_dir_all(&artifacts)?;
    diagnostics::install(browser)?;
    let server_port = port()?;
    let log = fixture.root().join("foreign-review-server.log");
    let mut command = Command::new(daemon);
    command
        .arg("--root")
        .arg(&fixture.pool)
        .arg("--storage-discovery-root")
        .arg(&fixture.discovery)
        .args(["--listen", &format!("127.0.0.1:{server_port}")]);
    server_web(&mut command, web, fixture.root());
    let server = server_launcher::launch(&mut command, &log)?;
    let result = server.finish(|| {
        visit(browser, server_port, password)?;
        let actor = session::Session::login(server_port, password)?;
        let before = fixture::snapshot(&fixture.target)?;
        browser.script(include_str!("foreign_review/delivery.js"))?;
        with_cleanup(|| {
            let evidence = scenario::exercise(browser, &fixture, &actor, &before)?;
            fixture.source_unchanged()?;
            fs::write(artifacts.join("evidence.json"), json_codec::to_string_pretty(&evidence)?)?;
            let encoded = browser.command("GET", "/screenshot", None)?;
            let png = STANDARD.decode(encoded.as_str().ok_or("missing foreign-review screenshot")?)?;
            if !png.starts_with(b"\x89PNG\r\n\x1a\n") { return Err("foreign-review screenshot is not PNG".into()); }
            fs::write(artifacts.join("review-only.png"), png)?;
            if browser.script("return window.__zxsSmokeErrors")? != json!([]) {
                return Err("uncaught browser exception in real foreign-review scenario".into());
            }
            Ok(())
        }, || {
            if browser.script("window.__zxsForeignReview.restore(); return window.__zxsForeignReview.pendingIds().length===0")? != true {
                return Err("foreign review cleanup left an original response pending".into());
            }
            Ok(())
        })?;
        browser.click(&browser.button("Sign out", "退出")?)?;
        browser.css("input[autocomplete='current-password']")?;
        Ok(())
    });
    if result.is_err() {
        diagnostics::failure(browser, &artifacts.join("diagnostics"), password);
        if let Ok(log) = fs::read_to_string(&log) {
            eprintln!("--- foreign-review daemon ---\n{log}");
        }
    }
    result?;
    fixture.source_unchanged()?;
    println!(
        "PASS real foreign review: per-history provenance, unavailable bodies, plan-only consent, unknown-save original-ID recovery and stale revalidation; import execution remains unavailable"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../unit/foreign_review_probe_tests.rs"]
mod tests;
