//! Production WASM and a real disposable daemon. API data is never fabricated.
//! Only selected original response deliveries are delayed or deliberately lost.
use super::*;
mod fixture;
mod session;
use fixture::Fixture;
use session::Session;

const SAVE: &str = "//section[contains(@class,'zxs-placement-review')]//button[normalize-space(.)='Save primary placement' or normalize-space(.)='保存主盘规则']";
const CLOSE: &str = "//section[contains(@class,'zxs-placement-review')]//button[normalize-space(.)='Close' or normalize-space(.)='关闭']";
fn check(browser: &Browser, label: &str, script: &str) -> Result<()> {
    wait_for(label, || {
        if browser.script(script)? == true {
            Ok(())
        } else {
            Err(format!("{label} not satisfied").into())
        }
    })
}
fn login(browser: &Browser, password: &str) -> Result<()> {
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
fn open_name(browser: &Browser, name: &str) -> Result<()> {
    browser.click(&browser.element(
        "xpath",
        &format!("//button[contains(@class,'zxs-file-name')][normalize-space(.)='{name}']"),
    )?)
}
fn repository(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Files", "文件")?)?;
    if browser.script("return [...document.querySelectorAll('button')].some(b=>['All disks','所有硬盘'].includes(b.textContent.trim()))")?==true {
        browser.click(&browser.button("All disks","所有硬盘")?)?;
    }
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    open_name(browser, fixture::REPOSITORY)?;
    browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='future']",
    )?;
    Ok(())
}
fn open_editor(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Primary placement", "主盘规则")?)?;
    editor_ready(browser)
}
fn editor_ready(browser: &Browser) -> Result<()> {
    check(
        browser,
        "real placement configuration loaded",
        r#"
        const panel=document.querySelector('.zxs-placement-review');
        const save=panel&&Array.from(panel.querySelectorAll('button')).find(b=>['Save primary placement','保存主盘规则'].includes(b.textContent.trim()));
        return !!save&&!save.disabled&&!!panel.querySelector('[role=combobox]')&&!panel.querySelector('input')
          &&(panel.textContent.includes('不搬移已有版本')||panel.textContent.includes('Existing versions stay where they are'))
          &&(panel.textContent.includes('不会自动改写备份设置')||panel.textContent.includes('does not change backup settings automatically'));
    "#,
    )
}
fn close_editor(browser: &Browser) -> Result<()> {
    browser.click(&browser.element("xpath", CLOSE)?)?;
    check(
        browser,
        "placement editor closed",
        "return !document.querySelector('.zxs-placement-review')",
    )
}
fn select_primary(browser: &Browser, root: &str) -> Result<()> {
    browser.click(&browser.css(".zxs-placement-review [role=combobox]")?)?;
    // The user selects the displayed physical directory, not an internal rule ID.
    browser.click(&browser.element(
        "xpath",
        &format!(
            "//*[@role='option' and @aria-disabled='false'][contains(normalize-space(.),'{root}')]"
        ),
    )?)?;
    check(
        browser,
        "actual primary directory selected",
        &format!(
            "return document.querySelector('.zxs-placement-review [role=combobox]').textContent.includes({})",
            json!(root)
        ),
    )
}
fn body(snapshot: &Value, primary: &str) -> Result<Value> {
    Ok(
        json!({"path":snapshot["scope"]["path"],"primary_slot_id":primary,
        "expected_scope_id":snapshot["scope"]["id"],"expected_rule_id":snapshot["rule_id"],
        "expected_generation":snapshot["generation"],"operation_id":storage_engine::OperationId::generate()?}),
    )
}
fn current_read(browser: &Browser) -> Result<Value> {
    browser.script("return window.__zxsPlacement.reads.at(-1).response")
}
fn expect_context(browser: &Browser, fixture: &Fixture, path: &str, scope: &str) -> Result<Value> {
    let snapshot = current_read(browser)?;
    if snapshot["repository_id"] != fixture.repository.repository_id
        || snapshot["scope"]["path"] != path
        || snapshot["scope"]["id"] != scope
    {
        return Err(format!("placement UI read the wrong object: {snapshot}").into());
    }
    if browser.script("return document.querySelector('.zxs-placement-review').querySelectorAll('input,textarea').length===0")?!=true {
        return Err("placement object editor asked for free-text IDs or generations".into());
    }
    Ok(snapshot)
}
fn request_matches(browser: &Browser, snapshot: &Value, index: usize) -> Result<()> {
    let request = browser.script(&format!("return window.__zxsPlacement.writes[{index}]"))?;
    let expected_keys = json!([
        "expected_generation",
        "expected_rule_id",
        "expected_scope_id",
        "operation_id",
        "path",
        "primary_slot_id"
    ]);
    let mut keys = request["body"]
        .as_object()
        .ok_or("missing observed placement request")?
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    if request["path"]
        != format!(
            "/api/repositories/{}/placement-configuration",
            snapshot["repository_id"]
                .as_str()
                .ok_or("missing snapshot repository")?
        )
        || json!(keys) != expected_keys
        || request["body"]["path"] != snapshot["scope"]["path"]
        || request["body"]["expected_scope_id"] != snapshot["scope"]["id"]
        || request["body"]["expected_rule_id"] != snapshot["rule_id"]
        || request["body"]["expected_generation"] != snapshot["generation"]
        || request["body"]["primary_slot_id"] != "P02"
    {
        return Err(
            format!("browser did not carry its real object/rule baseline: {request}").into(),
        );
    }
    request["body"]["operation_id"]
        .as_str()
        .ok_or("missing operation identity")?
        .parse::<storage_engine::OperationId>()?;
    Ok(())
}
fn close_and_session_delivery(browser: &Browser, fixture: &Fixture, password: &str) -> Result<()> {
    repository(browser)?;
    browser.script("window.__zxsPlacement.holdRead=true;return true")?;
    browser.click(&browser.button("Primary placement", "主盘规则")?)?;
    check(
        browser,
        "original placement read is delayed",
        "return !!window.__zxsPlacement.pendingRead",
    )?;
    close_editor(browser)?;
    browser.script("window.__zxsPlacement.pendingRead.release();return true")?;
    browser.command("POST","/execute/async",Some(json!({"script":"const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)));","args":[]})))?;
    check(
        browser,
        "closed editor ignores original delayed response",
        "return !document.querySelector('.zxs-placement-review')",
    )?;
    // Deliver another original response only after its login has been replaced.
    browser.script("window.__zxsPlacement.holdRead=true;return true")?;
    browser.click(&browser.button("Primary placement", "主盘规则")?)?;
    check(
        browser,
        "old-session placement read delayed",
        "return !!window.__zxsPlacement.pendingRead",
    )?;
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    login(browser, password)?;
    repository(browser)?;
    open_name(browser, "future")?;
    open_editor(browser)?;
    let before = expect_context(browser, fixture, "future", &fixture.future_directory)?;
    if before["primary_slot_id"] != "P02" || before["generation"] != "2" {
        return Err("replacement login did not load the saved future rule".into());
    }
    browser.script("window.__zxsPlacement.pendingRead.release();return true")?;
    browser.command("POST","/execute/async",Some(json!({"script":"const done=arguments[arguments.length-1];requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)));","args":[]})))?;
    check(
        browser,
        "replacement login retains its own current editor",
        r#"
        const panel=document.querySelector('.zxs-placement-review');
        return !!panel&&panel.textContent.includes('future')
          &&panel.querySelector('[role=combobox]')?.textContent.includes('P02')
          &&!panel.querySelector('.zxs-inline-error').textContent;
    "#,
    )?;
    check(
        browser,
        "replacement login retains its actual primary directory",
        &format!(
            "return document.querySelector('.zxs-placement-review [role=combobox]').textContent.includes({})",
            json!(fixture.target_root)
        ),
    )?;
    if current_read(browser)? != before {
        return Err("unexpected replacement-session configuration read after old delivery".into());
    }
    close_editor(browser)
}
fn read_only_views(browser: &Browser, fixture: &Fixture, session: &Session) -> Result<()> {
    session.success(
        "POST",
        "/api/file-manager/disks/P01/directory-branches",
        Some(json!({
        "path":format!("{}/future",fixture::REPOSITORY),"name":"placement-preview",
        "operation_id":storage_engine::OperationId::generate()?})),
    )?;
    repository(browser)?;
    open_name(browser, "future")?;
    let browse=browser.element("xpath","//div[contains(@class,'zxs-select')][./label[normalize-space(.)='Browse folder branch' or normalize-space(.)='浏览目录分支']]/button[@role='combobox']")?;
    let panel = browser.command(
        "GET",
        &format!("/element/{browse}/attribute/aria-controls"),
        None,
    )?;
    let panel = panel.as_str().ok_or("folder combobox lacks a listbox")?;
    browser.click(&browse)?;
    browser.click(&browser.element("xpath", &select_option_xpath(panel, "placement-preview"))?)?;
    browser.button("Return to default view", "返回默认视图")?;
    check(
        browser,
        "historical directory preview forbids placement writes",
        r#"
        const buttons=Array.from(document.querySelectorAll('button')).filter(b=>['Primary placement','主盘规则'].includes(b.textContent.trim()));
        return buttons.length===1&&buttons[0].disabled&&!document.querySelector('.zxs-placement-review');
    "#,
    )?;
    browser.click(&browser.button("Return to default view", "返回默认视图")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='B01']:not([disabled])")?)?;
    check(
        browser,
        "real backup disk forbids placement writes",
        r#"
        const buttons=Array.from(document.querySelectorAll('button')).filter(b=>['Primary placement','主盘规则'].includes(b.textContent.trim()));
        return buttons.length===1&&buttons[0].disabled&&!document.querySelector('.zxs-placement-review');
    "#,
    )?;
    let (status, _) = http(
        session_port(browser)?,
        "PUT",
        &fixture.endpoint(),
        Some(json!({})),
    )?;
    if status != 401 {
        return Err(format!("anonymous placement mutation returned {status}, not 401").into());
    }
    check(
        browser,
        "read-only and anonymous probes produced no UI writes",
        "return window.__zxsPlacement.writes.length===2",
    )
}
fn session_port(browser: &Browser) -> Result<u16> {
    browser.script("if(location.hostname!=='127.0.0.1'||location.protocol!=='http:')throw new Error('not the disposable fixture');return location.port")?
        .as_str().ok_or("missing loopback port")?.parse().map_err(Into::into)
}
fn exercise(browser: &Browser, fixture: &Fixture, password: &str) -> Result<Value> {
    browser.script(include_str!("placement_configuration/probe.js"))?;
    let session = Session::login(session_port(browser)?, password)?;
    let mut evidence = None;
    with_cleanup(
        || {
            repository(browser)?;
            open_editor(browser)?;
            let root = expect_context(browser, fixture, "", &fixture.repository.repository_id)?;
            if root["rule_id"] != Value::Null || root["generation"] != "0" {
                return Err("root read unexpectedly created a rule".into());
            }
            close_editor(browser)?;
            browser.click(&browser.element("xpath","//tr[.//button[contains(@class,'zxs-file-name')][normalize-space(.)='future']]//button[normalize-space(.)='Details' or normalize-space(.)='详情']")?)?;
            browser.click(&browser.element("xpath","//article[@aria-label='Folder details' or @aria-label='文件夹详情']//button[normalize-space(.)='Primary placement' or normalize-space(.)='主盘规则']")?)?;
            editor_ready(browser)?;
            let baseline = expect_context(browser, fixture, "future", &fixture.future_directory)?;
            select_primary(browser, &fixture.target_root)?;
            // A genuine second actor advances the exact rule after the UI read.
            session.success("PUT", &fixture.endpoint(), Some(body(&baseline, "P01")?))?;
            browser.click(&browser.element("xpath", SAVE)?)?;
            check(
                browser,
                "real concurrent rule conflict preserves editor",
                r#"
            const p=window.__zxsPlacement,panel=document.querySelector('.zxs-placement-review');
            return p.writes.length===1&&p.writes[0].status===409&&p.writes[0].response.error.code==='CONFIG_CONFLICT'
                &&!!panel&&panel.querySelector('.zxs-inline-error').textContent.includes('CONFIG_CONFLICT');
        "#,
            )?;
            request_matches(browser, &baseline, 0)?;
            let unchanged =
                session.success("GET", &format!("{}?path=future", fixture.endpoint()), None)?;
            if unchanged["primary_slot_id"] != "P01" || unchanged["generation"] != "1" {
                return Err("stale browser write changed placement".into());
            }
            browser.click(&browser.button("Reload current rule", "重新读取规则")?)?;
            check(
                browser,
                "conflicted editor reloads the actual current generation",
                "return window.__zxsPlacement.reads.at(-1).response?.generation==='1'&&!document.querySelector('.zxs-placement-review .zxs-inline-error').textContent",
            )?;
            let current = expect_context(browser, fixture, "future", &fixture.future_directory)?;
            select_primary(browser, &fixture.target_root)?;
            browser.script("window.__zxsPlacement.loseWrite=true;window.__zxsPlacement.holdOutcome=true;return true")?;
            browser.click(&browser.element("xpath", SAVE)?)?;
            check(
                browser,
                "committed but lost reply keeps original-operation fence",
                r#"
            const p=window.__zxsPlacement,panel=document.querySelector('.zxs-placement-review');
            if(p.writes.length!==2||p.writes[1].status!==200||!p.pendingOutcome||!panel)return false;
            return Array.from(panel.querySelectorAll('button')).filter(b=>['Save primary placement','保存主盘规则','Close','关闭'].includes(b.textContent.trim())).every(b=>b.disabled);
        "#,
            )?;
            request_matches(browser, &current, 1)?;
            for selector in [SAVE, CLOSE, SAVE] {
                let element = browser.element("xpath", selector)?;
                browser.command(
                    "POST",
                    &format!("/element/{element}/click"),
                    Some(json!({})),
                )?;
            }
            check(
                browser,
                "repeated clicks cannot repeat unknown placement write",
                "return window.__zxsPlacement.writes.length===2&&!!window.__zxsPlacement.pendingOutcome&&!!document.querySelector('.zxs-placement-review')",
            )?;
            browser.script("window.__zxsPlacement.pendingOutcome.release();return true")?;
            check(
                browser,
                "original real outcome completes without a repeated write",
                "const p=window.__zxsPlacement;return p.writes.length===2&&!document.querySelector('.zxs-placement-review')&&p.outcomes.some(o=>o.status===200&&o.response.operation_id===p.writes[1].body.operation_id)",
            )?;
            repository(browser)?;
            open_name(browser, "protected")?;
            open_editor(browser)?;
            let protected =
                expect_context(browser, fixture, "protected", &fixture.protected_directory)?;
            if !protected["targets"]
                .as_array()
                .ok_or("missing targets")?
                .iter()
                .any(|target| {
                    target["slot_id"] == "P02"
                        && target["unavailable_reason"] == "missing_backup_prefixes"
                })
            {
                return Err(
                    "real missing-prefix target was not rejected by read projection".into(),
                );
            }
            browser.click(&browser.css(".zxs-placement-review [role=combobox]")?)?;
            let blocked=browser.element("xpath",&format!("//*[@role='option' and @aria-disabled='true'][contains(normalize-space(.),'{}')]",fixture.target_root))?;
            browser.command(
                "POST",
                &format!("/element/{blocked}/click"),
                Some(json!({})),
            )?;
            check(
                browser,
                "missing-prefix option cannot change selected primary",
                "return document.querySelector('.zxs-placement-review [role=combobox]').textContent.includes('P01')&&window.__zxsPlacement.writes.length===2",
            )?;
            browser.keys(
                &browser.css(".zxs-placement-review [role=combobox]")?,
                "\u{e00c}",
            )?;
            let (status, error) =
                session.request("PUT", &fixture.endpoint(), Some(body(&protected, "P02")?))?;
            if status != 400 || error["error"]["code"] != "BACKUP_PREFIX_REQUIRED" {
                return Err(format!(
                    "missing-prefix mutation was not safely rejected: HTTP {status}: {error}"
                )
                .into());
            }
            let mut wrong_scope = body(&protected, "P01")?;
            wrong_scope["expected_scope_id"] = fixture.future_directory.clone().into();
            let (scope_status, scope_error) =
                session.request("PUT", &fixture.endpoint(), Some(wrong_scope))?;
            if scope_status != 409 || scope_error["error"]["code"] != "SCOPE_CHANGED" {
                return Err(format!("changed directory identity was not rejected: HTTP {scope_status}: {scope_error}").into());
            }
            if session.success(
                "GET",
                &format!("{}?path=protected", fixture.endpoint()),
                None,
            )? != protected
            {
                return Err(
                    "rejected target or identity changed protected-directory configuration".into(),
                );
            }
            close_editor(browser)?;
            read_only_views(browser, fixture, &session)?;
            close_and_session_delivery(browser, fixture, password)?;
            evidence=Some(browser.script("const p=window.__zxsPlacement;return {reads:p.reads,writes:p.writes,outcomes:p.outcomes}")?);
            if browser.script("return window.__zxsSmokeErrors")? != json!([]) {
                return Err("uncaught JavaScript error in real placement browser flow".into());
            }
            Ok(())
        },
        || {
            browser.script(
                "window.__zxsPlacement?.restore();delete window.__zxsPlacement;return true",
            )?;
            Ok(())
        },
    )?;
    evidence.ok_or_else(|| "missing placement browser evidence".into())
}

fn start(
    fixture: &Fixture,
    daemon: &Path,
    web: Option<&Path>,
    server_port: u16,
    log: &Path,
) -> Result<server_launcher::ServerProcess> {
    let mut command = Command::new(daemon);
    command
        .arg("--root")
        .arg(&fixture.pool)
        .arg("--storage-discovery-root")
        .arg(fixture.root())
        .args(["--listen", &format!("127.0.0.1:{server_port}")]);
    server_web(&mut command, web, fixture.root());
    server_launcher::launch(&mut command, log)
}
fn visit(browser: &Browser, server_port: u16, password: &str) -> Result<()> {
    wait_for("real placement daemon ready", || {
        let (status, bytes) = http(server_port, "GET", "/api/management/status", None)?;
        let value: Value = json_codec::from_slice(&bytes)?;
        if status == 200
            && value["authentication"] == "ready"
            && value["storage"]["state"] == "ready"
            && value["storage"]["recovering"] == false
        {
            Ok(())
        } else {
            Err(format!("placement fixture not ready: HTTP {status}: {value}").into())
        }
    })?;
    browser.command("DELETE", "/cookie", None)?;
    browser.command(
        "POST",
        "/url",
        Some(json!({"url":format!("http://127.0.0.1:{server_port}")})),
    )?;
    login(browser, password)
}
fn screenshot(browser: &Browser, path: &Path) -> Result<()> {
    let encoded = browser.command("GET", "/screenshot", None)?;
    let png = STANDARD.decode(encoded.as_str().ok_or("missing placement screenshot")?)?;
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("placement screenshot is not a PNG".into());
    }
    fs::write(path, png)?;
    Ok(())
}
fn network_evidence(browser: &Browser, origins: &[String]) -> Result<Value> {
    let console = browser.command("POST", "/log", Some(json!({"type":"browser"})))?;
    for entry in console.as_array().ok_or("invalid placement console log")? {
        if entry["level"] == "SEVERE" && entry["source"] == "javascript" {
            return Err(format!("placement JavaScript error: {}", entry["message"]).into());
        }
    }
    let logs = browser.command("POST", "/log", Some(json!({"type":"performance"})))?;
    let mut responses = Vec::new();
    for entry in logs.as_array().ok_or("invalid placement network log")? {
        let event: Value = json_codec::from_str(
            entry["message"]
                .as_str()
                .ok_or("missing placement network event")?,
        )?;
        let event = &event["message"];
        if event["method"] == "Network.requestWillBeSent" {
            let url = event["params"]["request"]["url"]
                .as_str()
                .unwrap_or_default();
            if origins
                .iter()
                .any(|origin| metadata::is_file_body_request(url, origin))
            {
                return Err(format!("placement Web requested file content: {url}").into());
            }
        }
        if event["method"] != "Network.responseReceived" {
            continue;
        }
        let response = &event["params"]["response"];
        let url = response["url"].as_str().unwrap_or_default();
        let status = response["status"].as_f64().unwrap_or_default() as u64;
        if !origins.iter().any(|origin| url.starts_with(origin)) {
            continue;
        }
        if status >= 400
            && !(status == 401 && url.ends_with("/api/auth/me"))
            && !(status == 404 && url.ends_with("/favicon.ico"))
            && !(status == 409 && url.ends_with("/placement-configuration"))
        {
            return Err(format!("unexpected placement browser HTTP {status}: {url}").into());
        }
        // Never retain cookies, bearer credentials, request headers or login bodies.
        responses.push(json!({"url":url,"status":status}));
    }
    for suffix in [
        "/api/auth/login",
        "/api/file-manager/disks",
        "/api/auth/logout",
    ] {
        if !responses.iter().any(|r| {
            r["url"].as_str().is_some_and(|url| url.ends_with(suffix)) && r["status"] == 200
        }) {
            return Err(format!("missing real placement browser response for {suffix}").into());
        }
    }
    if !responses
        .iter()
        .any(|r| r["url"].as_str().is_some_and(|url| url.contains(".wasm")) && r["status"] == 200)
    {
        return Err("placement browser did not load production WASM".into());
    }
    Ok(json!(responses))
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
    let artifacts = PathBuf::from("artifacts/browser-smoke/placement");
    fs::create_dir_all(&artifacts)?;
    diagnostics::install(browser)?;
    // Exclude other smoke fixtures' expected network failures from this proof.
    browser.command("POST", "/log", Some(json!({"type":"browser"})))?;
    browser.command("POST", "/log", Some(json!({"type":"performance"})))?;
    let first_port = port()?;
    let first_log = fixture.root().join("placement-server.log");
    let server = start(&fixture, daemon, web, first_port, &first_log)?;
    let mut evidence = None;
    let first = server.finish(|| {
        visit(browser, first_port, password)?;
        evidence = Some(exercise(browser, &fixture, password)?);
        repository(browser)?;
        open_name(browser, "future")?;
        open_editor(browser)?;
        screenshot(browser, &artifacts.join("future-placement.png"))?;
        close_editor(browser)?;
        browser.click(&browser.button("Sign out", "退出")?)?;
        browser.css("input[autocomplete='current-password']")?;
        Ok(())
    });
    if let Err(error) = first {
        diagnostics::failure(browser, &artifacts.join("diagnostics"), password);
        if let Ok(log) = fs::read_to_string(&first_log) {
            eprintln!("--- placement daemon ---\n{log}");
        }
        return Err(error);
    }
    // No browser content API exists. Stop the test daemon, use the production
    // engine to write exactly one NEW version, then reopen the real daemon.
    // The independent fixture checks all old version metadata, placements,
    // verified bytes and backup policies both before and after this write.
    let new_version = fixture.publish_future_version()?;
    let mut expected = fixture.originals();
    expected.push(new_version.clone());
    expected.sort();
    let second_port = port()?;
    let second_log = fixture.root().join("placement-restarted.log");
    let restarted = start(&fixture, daemon, web, second_port, &second_log)?;
    let result=restarted.finish(|| {
        visit(browser,second_port,password)?;
        repository(browser)?;open_name(browser,"future")?;open_name(browser,"note.txt")?;
        check(browser,"real preserved history after selected-primary restart",&format!(r#"
            const bodies=Array.from(document.querySelectorAll('.zxs-version-node[data-body-version-id]'))
              .map(node=>node.getAttribute('data-body-version-id')).filter(Boolean).sort();
            return JSON.stringify(bodies)===JSON.stringify({});
        "#,json!(expected)))?;
        assert_metadata_only(browser)?;
        screenshot(browser,&artifacts.join("preserved-history.png"))?;
        if browser.script("return window.__zxsSmokeErrors")?!=json!([]) {return Err("uncaught error after placement fixture restart".into());}
        let history=browser.script("return [...document.querySelectorAll('.zxs-version-node[data-body-version-id]')].map(node=>({node:node.getAttribute('data-node-id'),version:node.getAttribute('data-body-version-id'),text:node.innerText}))")?;
        browser.click(&browser.button("Sign out","退出")?)?;
        browser.css("input[autocomplete='current-password']")?;
        let network=network_evidence(browser,&[format!("http://127.0.0.1:{first_port}"),format!("http://127.0.0.1:{second_port}")])?;
        fs::write(artifacts.join("evidence.json"),json_codec::to_string_pretty(&json!({
            "source":"production WASM and disposable real daemon; original responses only",
            "browser":browser.script("return navigator.userAgent")?,
            "placement":evidence.take().ok_or("missing real placement evidence")?,
            "original_versions":fixture.originals(),"new_version":new_version,
            "new_primary":"P02","historical_metadata_placements_and_verified_bytes_unchanged":true,
            "backup_policies_unchanged":true,"history":history,"network":network
        }))?)?;
        Ok(())
    });
    if result.is_err() {
        diagnostics::failure(browser, &artifacts.join("diagnostics"), password);
        if let Ok(log) = fs::read_to_string(&second_log) {
            eprintln!("--- restarted placement daemon ---\n{log}");
        }
    }
    result?;
    println!(
        "PASS real primary-placement object/context, conflict, original-operation recovery, read-only and missing-prefix rejection, session fences, immutable history and future-only routing"
    );
    Ok(())
}
