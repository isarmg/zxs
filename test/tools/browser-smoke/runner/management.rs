//! Real second-daemon acceptance: persisted control-plane credentials, corrupt
//! pool configuration, production Web/WASM, and real browser login/retry.
use super::{
    Browser, Command, OpenOptions, OpenOptionsExt, Path, Result, Write, fs, http, json, port,
    spawn, wait_for,
};

fn prepare_offline(source_pool: &Path, offline: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let pool = offline.join("pool");
    let management = offline.join("management");
    // This second daemon must obey the production control-directory contract;
    // create_dir_all would inherit a typical umask and make credentials unusable.
    for path in [&pool, &management] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    // Only the independently persisted, verified management authority may be
    // copied. Legacy pool/auth.json is deliberately not accepted as authority.
    let auth = fs::read(source_pool.join("management/management-auth.json"))?;
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(management.join("management-auth.json"))?
        .write_all(&auth)?;
    fs::write(pool.join("config.json"), b"{corrupt pool configuration")?;
    Ok(())
}

pub(super) fn run(
    browser: &Browser,
    daemon: &Path,
    web: &Path,
    source_pool: &Path,
    root: &Path,
    password: &str,
) -> Result<()> {
    let offline = root.join("offline-management");
    let pool = offline.join("pool");
    let management = offline.join("management");
    prepare_offline(source_pool, &offline)?;
    let server_port = port()?;
    let origin = format!("http://127.0.0.1:{server_port}");
    let _server = spawn(
        Command::new(daemon)
            .arg("--root")
            .arg(&pool)
            .arg("--management-root")
            .arg(&management)
            .args(["--listen", &format!("127.0.0.1:{server_port}"), "--web-dir"])
            .arg(web),
        &offline.join("server.log"),
    )?;
    wait_for("offline management listener", || {
        let (status, bytes) = http(server_port, "GET", "/api/management/status", None)?;
        let value: json_codec::Value = json_codec::from_slice(&bytes)?;
        if status == 200
            && value["authentication"] == "ready"
            && value["storage"]["state"] == "unavailable"
        {
            Ok(())
        } else {
            Err(format!(
                "offline control-plane state not ready: HTTP {status}, authentication={}, storage={}",
                value["authentication"], value["storage"]
            ).into())
        }
    })?;
    browser.command("DELETE", "/cookie", None)?;
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    browser.keys(
        &browser.css("input[autocomplete='username']")?,
        "smoke-admin",
    )?;
    browser.keys(
        &browser.css("input[autocomplete='current-password']")?,
        password,
    )?;
    browser.click(&browser.button("Sign in", "登录")?)?;
    browser.button("Sign out", "退出")?;
    wait_for("offline management health visible", || {
        if browser.script("return document.body.textContent.includes('Data storage is unavailable') || document.body.textContent.includes('数据存储不可用')")? == true { Ok(()) } else { Err("offline health is not visible".into()) }
    })?;
    browser.click(&browser.button("Status", "状态")?)?;
    browser.css(".zxs-monitor-page [data-metric='cpu']")?;
    browser.css(".zxs-monitor-page [data-metric='memory']")?;
    browser.click(&browser.button("Management health", "管理健康")?)?;
    browser.script("window.__managementRetryStatus = null; const original = window.fetch; window.fetch = async function(...args) { const response = await Reflect.apply(original, this, args); const input = args[0]; const path = new URL(typeof input === 'string' ? input : input.url, location.href).pathname; if (path === '/api/system/storage/retry') { window.__managementRetryStatus = response.status; window.fetch = original; } return response; }; return true")?;
    browser.click(&browser.button("Retry opening data storage", "重试打开数据存储")?)?;
    wait_for("actual authenticated storage retry response", || {
        if browser.script("return window.__managementRetryStatus")? == 200 {
            Ok(())
        } else {
            Err("storage retry response not received or rejected".into())
        }
    })?;
    wait_for("failed storage reopen leaves management available", || {
        let (status, bytes) = http(server_port, "GET", "/api/management/status", None)?;
        let value: json_codec::Value = json_codec::from_slice(&bytes)?;
        if status == 200
            && value["storage"]["state"] == "unavailable"
            && value["storage"]["recovering"] == false
        {
            Ok(())
        } else {
            Err("storage retry still running".into())
        }
    })?;
    browser.button("Sign out", "退出")?;
    if browser
        .script("return document.querySelector('input[autocomplete=new-password]') !== null")?
        == true
    {
        return Err("storage fault opened administrator reset/setup form".into());
    }
    if browser.script("return window.__zxsSmokeErrors")? != json!([]) {
        return Err("uncaught browser error during offline management".into());
    }
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    println!(
        "PASS real corrupt-pool daemon: existing administrator login, health, CPU/memory, retry and logout without initialization"
    );
    Ok(())
}

pub(super) fn setup(
    browser: &Browser,
    daemon: &Path,
    web: &Path,
    root: &Path,
    password: &str,
) -> Result<()> {
    use std::io::Read;
    use std::os::unix::fs::DirBuilderExt;
    let fresh = root.join("fresh-web-setup");
    fs::create_dir_all(&fresh)?;
    let pool = fresh.join("pool");
    let management = fresh.join("management");
    fs::DirBuilder::new().mode(0o700).create(&management)?;
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let key: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(management.join("bootstrap.key"))?
        .write_all(key.as_bytes())?;
    let server_port = port()?;
    let origin = format!("http://127.0.0.1:{server_port}");
    let mut command = Command::new(daemon);
    command
        .arg("--root")
        .arg(&pool)
        .arg("--management-root")
        .arg(&management)
        .args(["--listen", &format!("127.0.0.1:{server_port}"), "--web-dir"])
        .arg(web)
        .arg("--allow-development-setup");
    let id = "fixture-primary";
    let disk = fresh.join(id);
    fs::DirBuilder::new().mode(0o700).create(&disk)?;
    command
        .arg("--setup-disk")
        .arg(format!("{id}={}", disk.display()));
    let server = spawn(&mut command, &fresh.join("server.log"))?;
    wait_for("fresh setup listener", || {
        if http(server_port, "GET", "/", None)?.0 == 200 {
            Ok(())
        } else {
            Err("setup listener not ready".into())
        }
    })?;
    browser.command("DELETE", "/cookie", None)?;
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    browser.css("input[autocomplete='current-password']")?;
    browser.click(&browser.css("details > summary")?)?;
    browser.keys(&browser.css("[data-setup-key]")?, &key)?;
    browser.click(&browser.button("Authorize and check disks", "验证授权并检查磁盘")?)?;
    browser.css("[data-setup-disk]")?;
    browser.keys(&browser.css("[data-setup-user]")?, "fresh-admin")?;
    reauthorize_identity(browser, "fresh-admin")?;
    let disks = browser.command(
        "POST",
        "/elements",
        Some(json!({"using":"css selector","value":"[data-setup-disk]"})),
    )?;
    if disks.as_array().map(Vec::len) != Some(1) {
        return Err("single-disk setup must expose exactly one authorized storage disk".into());
    }
    for disk in disks.as_array().ok_or("missing setup disk selectors")? {
        browser.click(
            disk[super::ELEMENT]
                .as_str()
                .ok_or("missing setup disk element")?,
        )?;
    }
    if browser.script("return document.body.textContent.includes('no disk redundancy') || document.body.textContent.includes('没有磁盘冗余')")? != true {
        return Err("single-disk setup must explicitly warn about no redundancy".into());
    }
    browser.click(&browser.css("[data-setup-development]")?)?;
    browser.keys(&browser.css("[data-setup-password]")?, password)?;
    browser.click(&browser.css("[data-setup-confirm]")?)?;
    browser.click(&browser.button("Initialize new installation", "初始化新安装")?)?;
    wait_for("setup accepted and hot storage reopen", || {
        let (status, bytes) = http(server_port, "GET", "/api/management/status", None)?;
        let value: json_codec::Value = json_codec::from_slice(&bytes)?;
        if status == 200
            && value["authentication"] == "ready"
            && value["storage"]["state"] == "ready"
        {
            Ok(())
        } else {
            Err("setup is not yet ready".into())
        }
    })?;
    wait_for("setup secrets cleared after submission", || {
        if browser.script("return document.querySelector('[data-setup-key]').value === '' && (!document.querySelector('[data-setup-password]') || document.querySelector('[data-setup-password]').value === '')")? == true { Ok(()) } else { Err("setup input still retains a secret".into()) }
    })?;
    let persisted = browser.script("return location.href + JSON.stringify(Object.entries(localStorage)) + JSON.stringify(Object.entries(sessionStorage))")?;
    let persisted = persisted.as_str().unwrap_or_default();
    if persisted.contains(&key) || persisted.contains(password) {
        return Err("setup secret persisted in browser storage or URL".into());
    }
    browser.keys(
        &browser.css(".authentication-card input[autocomplete='username']")?,
        "fresh-admin",
    )?;
    browser.keys(
        &browser.css("input[autocomplete='current-password']")?,
        password,
    )?;
    browser.click(&browser.button("Sign in", "登录")?)?;
    browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    let config: storage_engine::Config =
        json_codec::from_slice(&fs::read(pool.join("config.json"))?)?;
    if config.disks.len() != 1
        || config.disks[0].level != 1
        || config.disks[0].upstream_slot_id.is_some()
    {
        return Err("single-disk wizard invented backup levels".into());
    }
    drop(server);
    let disk = fresh.join("fixture-primary");
    fs::rename(&disk, fresh.join("fixture-primary-offline"))?;
    let _restarted = spawn(&mut command, &fresh.join("missing-disk-server.log"))?;
    wait_for("single missing disk leaves management online", || {
        let (status, bytes) = http(server_port, "GET", "/api/management/status", None)?;
        let value: json_codec::Value = json_codec::from_slice(&bytes)?;
        if status == 200
            && value["authentication"] == "ready"
            && value["storage"]["state"] == "unavailable"
            && value["storage"]["recovering"] == false
        {
            Ok(())
        } else {
            Err("single-disk management restart not ready".into())
        }
    })?;
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    browser.keys(
        &browser.css(".authentication-card input[autocomplete='username']")?,
        "fresh-admin",
    )?;
    browser.keys(
        &browser.css("input[autocomplete='current-password']")?,
        password,
    )?;
    browser.click(&browser.button("Sign in", "登录")?)?;
    browser.button("Sign out", "退出")?;
    wait_for("single-disk failure warning visible after login", || {
        if browser.script("return document.body.textContent.includes('Data storage is unavailable') || document.body.textContent.includes('数据存储不可用')")? == true { Ok(()) } else { Err("single-disk failure warning absent".into()) }
    })?;
    browser.click(&browser.button("Status", "状态")?)?;
    browser.css(".zxs-monitor-page [data-metric='cpu']")?;
    browser.css(".zxs-monitor-page [data-metric='memory']")?;
    if disk.exists() {
        return Err("missing storage disk was silently recreated".into());
    }
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    println!(
        "PASS protected single-disk Web setup: no backup levels, explicit no-redundancy warning, key + CSRF, hot ready reopen, administrator login, no persisted secrets; missing only disk preserves browser login and monitoring without recreating storage"
    );
    preset_setup(browser, daemon, web, root, password)?;
    complete_setup(browser, daemon, web, root, password)
}

struct SetupFixture {
    root: std::path::PathBuf,
    pool: std::path::PathBuf,
    management: std::path::PathBuf,
    disks: std::collections::BTreeMap<String, std::path::PathBuf>,
    key: String,
    port: u16,
}
impl SetupFixture {
    fn new(root: &Path, label: &str, count: usize) -> Result<Self> {
        use std::{io::Read, os::unix::fs::DirBuilderExt};
        let root = root.join(label);
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        // Direct service fixtures, unlike daemon startup, must provision the
        // empty data root before SetupRecovery validates its directory.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("pool"))?;
        let management = root.join("management");
        fs::DirBuilder::new().mode(0o700).create(&management)?;
        let mut entropy = [0u8; 32];
        fs::File::open("/dev/urandom")?.read_exact(&mut entropy)?;
        let key: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(management.join("bootstrap.key"))?
            .write_all(key.as_bytes())?;
        let mut disks = std::collections::BTreeMap::new();
        for index in 0..count {
            let id = format!("candidate-{index}");
            let path = root.join(&id);
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            disks.insert(id, path);
        }
        Ok(Self {
            pool: root.join("pool"),
            root,
            management,
            disks,
            key,
            port: port()?,
        })
    }
    fn launch(&self, daemon: &Path, web: &Path, preset: Option<&Path>) -> Result<super::Process> {
        let mut command = Command::new(daemon);
        command
            .arg("--root")
            .arg(&self.pool)
            .arg("--management-root")
            .arg(&self.management)
            .args(["--listen", &format!("127.0.0.1:{}", self.port), "--web-dir"])
            .arg(web)
            .arg("--allow-development-setup");
        for (id, path) in &self.disks {
            command
                .arg("--setup-disk")
                .arg(format!("{id}={}", path.display()));
        }
        if let Some(preset) = preset {
            command
                .arg("--setup-config")
                .arg(format!("four-disk-preset={}", preset.display()));
        }
        let server = spawn(&mut command, &self.root.join("server.log"))?;
        wait_for("protected setup fixture listener", || {
            if http(self.port, "GET", "/", None)?.0 == 200 {
                Ok(())
            } else {
                Err("fixture listener not ready".into())
            }
        })?;
        Ok(server)
    }
    fn open(&self, browser: &Browser) -> Result<()> {
        browser.command("DELETE", "/cookie", None)?;
        browser.command(
            "POST",
            "/url",
            Some(json!({"url":format!("http://127.0.0.1:{}",self.port)})),
        )?;
        browser.css("input[autocomplete='current-password']")?;
        browser.click(&browser.css("details > summary")?)?;
        self.authorize(browser)
    }
    fn authorize(&self, browser: &Browser) -> Result<()> {
        browser.keys(&browser.css("[data-setup-key]")?, &self.key)?;
        browser.click(&browser.button("Authorize and check disks", "验证授权并检查磁盘")?)?;
        Ok(())
    }
    fn ready(&self) -> Result<()> {
        wait_for("real storage initialization ready", || {
            let (status, bytes) = http(self.port, "GET", "/api/management/status", None)?;
            let value: json_codec::Value = json_codec::from_slice(&bytes)?;
            if status == 200
                && value["authentication"] == "ready"
                && value["storage"]["state"] == "ready"
            {
                Ok(())
            } else {
                Err("initialization still pending".into())
            }
        })
    }
}
// Reauthorization unmounts and remounts the real setup form. Its visible
// identity must remain exactly the model value that will create the account.
fn reauthorize_identity(browser: &Browser, username: &str) -> Result<()> {
    for _ in 0..2 {
        browser.click(&browser.css("[data-setup-confirm]")?)?;
        browser.script("window.__setupIdentityBeforeAuthorization = document.querySelector('[data-setup-user]'); return true")?;
        browser.click(&browser.button("Authorize and check disks", "验证授权并检查磁盘")?)?;
        browser.css("[data-setup-user]")?;
        let expected = json_codec::to_string(username)?;
        wait_for("reauthorized setup identity matches submission", || {
            if browser.script(&format!("return document.querySelector('[data-setup-user]') !== window.__setupIdentityBeforeAuthorization && document.querySelector('[data-setup-user]')?.value === {expected} && !document.querySelector('[data-setup-confirm]').checked"))? == true {
                Ok(())
            } else {
                Err("reauthorization changed the displayed administrator identity".into())
            }
        })?;
        browser.script("delete window.__setupIdentityBeforeAuthorization; return true")?;
    }
    Ok(())
}
fn credentials_and_confirm(browser: &Browser, username: &str, password: &str) -> Result<()> {
    let user = browser.css("[data-setup-user]")?;
    browser.command("POST", &format!("/element/{user}/clear"), Some(json!({})))?;
    browser.keys(&user, username)?;
    browser.keys(&browser.css("[data-setup-password]")?, password)?;
    browser.click(&browser.css("[data-setup-confirm]")?)?;
    Ok(())
}
fn login_setup(browser: &Browser, username: &str, password: &str, count: usize) -> Result<()> {
    browser.keys(
        &browser.css(".authentication-card input[autocomplete='username']")?,
        username,
    )?;
    browser.keys(
        &browser.css("input[autocomplete='current-password']")?,
        password,
    )?;
    browser.click(&browser.button("Sign in", "登录")?)?;
    wait_for("preserved initialized disk count", || {
        let result = browser
            .script("return document.querySelectorAll('.zxs-disk-entry:not([disabled])').length")?;
        if result.as_u64() == Some(count as u64) {
            Ok(())
        } else {
            Err("initialized disk count differs".into())
        }
    })?;
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    Ok(())
}
fn select_preset(browser: &Browser, expected_limit: usize) -> Result<()> {
    browser.click(&browser.button("Use local configuration preset", "使用本地预置方案")?)?;
    // Scrolling the trigger into view can close its popover. Reacquire and
    // reopen the control on each attempt instead of retrying a hidden option.
    // Keep real WebDriver pointer clicks: do not select through script state.
    wait_for("visible four-disk preset selection", || {
        if browser.script("return document.querySelector('details [role=combobox] .zxs-select-value')?.textContent === 'four-disk-preset'")? == true {
            return Ok(());
        }
        if browser.script("return document.querySelector('details [role=combobox]')?.getAttribute('aria-expanded') === 'true'")? != true {
            let trigger = browser.css("details [role='combobox']")?;
            browser.command("POST", &format!("/element/{trigger}/click"), Some(json!({})))?;
        }
        let option = browser.element(
            "xpath",
            "//*[@role='option' and normalize-space(.)='four-disk-preset']",
        )?;
        browser.command("POST", &format!("/element/{option}/click"), Some(json!({})))?;
        Err("waiting for preset selection to render".into())
    })?;
    browser.click(&browser.button("Review preset summary", "查看预置方案摘要")?)?;
    wait_for("real sanitized preset preview", || {
        let value=browser.script("const e=document.querySelector('[data-setup-preset-summary]'); return e ? JSON.parse(e.textContent) : null")?;
        if value["max_workspaces"].as_u64() == Some(expected_limit as u64)
            && value["disks"].as_array().is_some_and(|disks| {
                disks.len() == 4
                    && disks
                        .iter()
                        .all(|disk| disk.get("root").is_none() && disk["candidate_id"].is_string())
            })
        {
            Ok(())
        } else {
            Err("preset preview absent or lost parameters / exposed roots".into())
        }
    })
}
fn write_preset(fixture: &SetupFixture) -> Result<(storage_engine::Config, std::path::PathBuf)> {
    let mut config = storage_engine::Config::development([
        fixture.disks["candidate-0"].clone(),
        fixture.disks["candidate-1"].clone(),
        fixture.disks["candidate-2"].clone(),
    ]);
    let mut fourth = config.disks[2].clone();
    fourth.slot_id = "D01".into();
    fourth.level = 4;
    fourth.upstream_slot_id = Some("C01".into());
    fourth.disk_id = storage_engine::new_id("disk");
    fourth.root = fixture.disks["candidate-3"].clone();
    fourth.metadata_member = false;
    config.disks.push(fourth);
    config.max_workspaces = 17;
    let preset = fixture.root.join("private-config.json");
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&preset)?
        .write_all(&json_codec::to_vec(&config)?)?;
    Ok((config, preset))
}
fn preset_setup(
    browser: &Browser,
    daemon: &Path,
    web: &Path,
    root: &Path,
    password: &str,
) -> Result<()> {
    let fixture = SetupFixture::new(root, "preset-web-setup", 4)?;
    let (mut config, preset) = write_preset(&fixture)?;
    let _server = fixture.launch(daemon, web, Some(&preset))?;
    fixture.open(browser)?;
    select_preset(browser, 17)?;
    // Modify a local preset only after the real preview response. The browser
    // still holds the old digest and must not silently apply these new values.
    config.max_workspaces = 19;
    fs::write(&preset, json_codec::to_vec(&config)?)?;
    browser.script("window.__presetRejected = null; const original=window.fetch; window.fetch=async function(...args) {const response=await Reflect.apply(original,this,args); const input=args[0]; if(new URL(typeof input==='string'?input:input.url,location.href).pathname==='/api/setup/initialize-preset'){window.__presetRejected=response.status; window.fetch=original;} return response;}; return true")?;
    credentials_and_confirm(browser, "preset-admin", password)?;
    browser.click(&browser.button("Initialize new installation", "初始化新安装")?)?;
    wait_for("changed preset rejected by real HTTP", || {
        if browser.script("return window.__presetRejected")? == 409 {
            Ok(())
        } else {
            Err("expected preset digest conflict".into())
        }
    })?;
    wait_for("changed preset actionable error", || {
        if browser.script("return document.body.textContent.includes('SETUP_PRESET_CHANGED')")?
            == true
        {
            Ok(())
        } else {
            Err("digest rejection missing from UI".into())
        }
    })?;
    if fixture.pool.join("config.json").exists()
        || fixture.management.join("management-auth.json").exists()
    {
        return Err("rejected preset initialized storage or authentication".into());
    }
    fixture.authorize(browser)?;
    select_preset(browser, 19)?;
    credentials_and_confirm(browser, "preset-admin", password)?;
    browser.click(&browser.button("Initialize new installation", "初始化新安装")?)?;
    fixture.ready()?;
    let actual: json_codec::Value =
        json_codec::from_slice(&fs::read(fixture.pool.join("config.json"))?)?;
    if actual != json_codec::to_value(&config)? {
        return Err("preset initialization rewrote reviewed configuration".into());
    }
    login_setup(browser, "preset-admin", password, 4)?;
    println!(
        "PASS real four-disk preset preview, changed-digest HTTP409 rejection, explicit re-review, exact configuration preservation and administrator login"
    );
    Ok(())
}
fn complete_setup(
    browser: &Browser,
    daemon: &Path,
    web: &Path,
    root: &Path,
    password: &str,
) -> Result<()> {
    let fixture = SetupFixture::new(root, "interrupted-web-setup", 3)?;
    prepare_interruption(&fixture, password)?;
    let original_config = fs::read(fixture.pool.join("config.json"))?;
    let _server = fixture.launch(daemon, web, None)?;
    fixture.open(browser)?;
    browser.click(&browser.button("Complete interrupted initialization", "完成中断的初始化")?)?;
    browser.keys(&browser.css("[data-setup-user]")?, "completion-admin")?;
    reauthorize_identity(browser, "completion-admin")?;
    browser.click(&browser.button("Complete interrupted initialization", "完成中断的初始化")?)?;
    browser.keys(&browser.css("[data-setup-password]")?, password)?;
    browser.click(&browser.css("[data-setup-confirm]")?)?;
    browser.click(&browser.button("Confirm initialization completion", "确认补全初始化")?)?;
    fixture.ready()?;
    if fs::read(fixture.pool.join("config.json"))? != original_config {
        return Err("bounded completion rewrote the initialized pool".into());
    }
    login_setup(browser, "completion-admin", password, 3)?;
    println!(
        "PASS real durable pre-authority interruption fixture, explicit protected Web completion, unchanged pool and administrator login; no product debug endpoint"
    );
    Ok(())
}

// The only injected failure is a test-owned service callback, before any
// authentication authority is published. No daemon debug route is involved.
fn prepare_interruption(fixture: &SetupFixture, password: &str) -> Result<()> {
    use storage_server::setup_recovery::{SetupRecovery, SetupRequest};
    let service = SetupRecovery::new(
        fixture.pool.clone(),
        fixture.management.clone(),
        fixture.disks.clone(),
    )?
    .allow_development_mode();
    let permit = service.authorize_bootstrap(&fixture.key)?;
    let result = service.initialize(
        &permit,
        SetupRequest {
            disk_ids: vec![
                "candidate-0".into(),
                "candidate-1".into(),
                "candidate-2".into(),
            ],
            mode: storage_engine::Mode::Development,
            username: "completion-admin".into(),
            password: password.into(),
        },
        |_engine, _, _| {
            Err(storage_server::ApiError::new(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_engine::ErrorCode::InjectedFailure,
                "fixture failure before authority publication",
            ))
        },
    );
    if !matches!(result,Err(error) if error.code==storage_engine::ErrorCode::InjectedFailure)
        || !service.status().completion_available
    {
        return Err("interrupted setup fixture lacks genuine bound completion proof".into());
    }
    drop(service);
    Ok(())
}

#[test]
fn completion_fixture_has_service_proof_without_published_authority() {
    let root = tempfile::tempdir().unwrap();
    let fixture = SetupFixture::new(root.path(), "completion-fixture-unit", 3).unwrap();
    prepare_interruption(&fixture, "fixture-only-long-password").unwrap();
    assert!(fixture.pool.join("config.json").exists());
    assert!(!fixture.pool.join("management-authority.json").exists());
    assert!(!fixture.management.join("management-auth.json").exists());
}

#[test]
fn browser_preset_fixture_has_valid_topology_and_sanitized_preview() {
    use storage_server::setup_recovery::{PresetPreviewRequest, SetupRecovery};
    let root = tempfile::tempdir().unwrap();
    let fixture = SetupFixture::new(root.path(), "preset-fixture-unit", 4).unwrap();
    let (config, preset) = write_preset(&fixture).unwrap();
    let service = SetupRecovery::new(
        fixture.pool.clone(),
        fixture.management.clone(),
        fixture.disks.clone(),
    )
    .unwrap()
    .allow_development_mode()
    .with_presets(std::collections::BTreeMap::from([(
        "four-disk-preset".into(),
        preset,
    )]));
    let permit = service.authorize_bootstrap(&fixture.key).unwrap();
    let preview = service
        .preview_preset(
            &permit,
            PresetPreviewRequest {
                preset_id: "four-disk-preset".into(),
            },
        )
        .unwrap();
    let summary = &preview["configuration"];
    assert_eq!(summary["max_workspaces"], 17);
    assert_eq!(summary["disks"].as_array().unwrap().len(), 4);
    assert_eq!(summary["disks"][3]["candidate_id"], "candidate-3");
    assert_eq!(config.disks[3].upstream_slot_id.as_deref(), Some("C01"));
    assert!(!summary.to_string().contains(fixture.root.to_str().unwrap()));
    assert!(
        summary["disks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|disk| disk.get("root").is_none())
    );
    assert!(!fixture.pool.join("config.json").exists());
}

#[cfg(test)]
#[path = "../unit/offline_management_tests.rs"]
mod offline_management_tests;
