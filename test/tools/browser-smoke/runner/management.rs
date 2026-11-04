//! Real second-daemon acceptance: persisted control-plane credentials, corrupt
//! pool configuration, production Web/WASM, and real browser login/retry.
use super::{
    Browser, Command, OpenOptions, OpenOptionsExt, Path, Result, Write, fs, http, json, port,
    wait_for, with_cleanup,
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
    web: Option<&Path>,
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
    let mut command = Command::new(daemon);
    command
        .arg("--root")
        .arg(&pool)
        .arg("--management-root")
        .arg(&management)
        .args(["--listen", &format!("127.0.0.1:{server_port}")]);
    super::server_web(&mut command, web, &offline);
    let server = super::server_launcher::launch(&mut command, &offline.join("server.log"))?;
    server.finish(|| {
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
        &browser.css("form[data-auth-form='login'] input[autocomplete='username']:not([readonly])")?,
        "smoke-admin",
    )?;
    browser.keys(
        &browser.css("form[data-auth-form='login'] input[autocomplete='current-password']:not([readonly])")?,
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
    browser.click(&browser.button("Administration", "系统管理")?)?;
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
    browser.css("form[data-auth-form='login'] input[autocomplete='current-password']:not([readonly])")?;
    println!(
        "PASS real corrupt-pool daemon: existing administrator login, health, CPU/memory, retry and logout without initialization"
    );
    Ok(())
    })
}

pub(super) fn setup(
    browser: &Browser,
    daemon: &Path,
    web: Option<&Path>,
    root: &Path,
    password: &str,
) -> Result<()> {
    let fixture = SetupFixture::new(root, "fresh-web-setup")?;
    let server = fixture.launch(daemon, web, "server.log")?;
    let storage = fixture.root.join("storage-parent/selected-storage");
    server.finish(|| {
        let previews = std::env::var_os("ZXS_LOGIN_PREVIEWS")
            .map(|dir| std::path::PathBuf::from(dir).join("first-account"));
        fixture.visit(browser)?;
        super::auth_layout::first_account(
            browser,
            &format!("http://127.0.0.1:{}", fixture.port),
            previews.as_deref(),
        )?;
        fixture.open(browser)?;
        browser.keys(&browser.css("[data-setup-user]")?, "fresh-admin")?;
        reauthorize_identity(browser, "fresh-admin")?;
        credentials(browser, "fresh-admin", password)?;
        browser.click(
            &browser.css("[data-first-account-form] button[type=submit]:not([disabled])")?,
        )?;
        fixture.wait_state("unconfigured")?;
        fixture.no_storage()?;
        cleared_secrets(browser, password)?;
        sign_in(browser, "fresh-admin", password)?;
        browser.click(&browser.button("Status", "状态")?)?;
        browser.css(".zxs-monitor-page [data-metric='cpu']")?;
        browser.css(".zxs-monitor-page [data-metric='memory']")?;
        // Provision only a disposable test-owned child after account creation.
        // No --setup-disk/preset or mount-root restriction is used by this daemon.
        let storage = fixture.storage_directory()?;
        with_cleanup(
            || {
                browser.script(STORAGE_PROBE)?;
                select_storage(browser, &storage)?;
                browser.click(&browser.css("[data-storage-activate]")?)?;
                fixture.wait_state("ready")?;
                storage_active(browser)?;
                assert_activation_posts(browser, &["/api/admin/storage/activate"])?;
                assert_single_storage(&fixture, &storage)?;
                Ok(())
            },
            || restore_storage_probe(browser),
        )?;
        browser.click(&browser.button("Files", "文件")?)?;
        browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
        sign_out(browser)?;
        Ok(())
    })?;
    // A disappearing selected child must remain missing on restart. Its parent
    // and mount still exist, so accepting the mount alone would be a real defect.
    fs::rename(&storage, storage.with_file_name("selected-storage-offline"))?;
    let restarted = fixture.launch(daemon, web, "missing-directory-server.log")?;
    restarted.finish(|| {
        fixture.wait_state("unavailable")?;
        fixture.visit(browser)?;
        sign_in(browser, "fresh-admin", password)?;
        wait_for("missing selected directory warning", || {
            if browser.script("return document.body.textContent.includes('Data storage is unavailable') || document.body.textContent.includes('数据存储不可用')")? == true {
                Ok(())
            } else {
                Err("missing-directory warning absent".into())
            }
        })?;
        browser.click(&browser.button("Status", "状态")?)?;
        browser.css(".zxs-monitor-page [data-metric='cpu']")?;
        browser.css(".zxs-monitor-page [data-metric='memory']")?;
        if storage.exists() {
            return Err("missing selected storage directory was silently recreated".into());
        }
        sign_out(browser)
    })?;
    println!(
        "PASS no-disk account setup, administrator monitoring, real nested-directory browse/inspection/activation, exact configured root, and missing-directory restart without recreation"
    );
    lost_initialize_response(browser, daemon, web, root, password)?;
    complete_activation(browser, daemon, web, root, password)
}

struct SetupFixture {
    root: std::path::PathBuf,
    pool: std::path::PathBuf,
    management: std::path::PathBuf,
    port: u16,
}
impl SetupFixture {
    fn new(root: &Path, label: &str) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let root = root.join(label);
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("pool"))?;
        let management = root.join("management");
        fs::DirBuilder::new().mode(0o700).create(&management)?;
        Ok(Self {
            pool: root.join("pool"),
            root,
            management,
            port: port()?,
        })
    }
    fn launch(
        &self,
        daemon: &Path,
        web: Option<&Path>,
        log: &str,
    ) -> Result<super::server_launcher::ServerProcess> {
        let mut command = Command::new(daemon);
        command
            .arg("--root")
            .arg(&self.pool)
            .arg("--management-root")
            .arg(&self.management)
            .args(["--listen", &format!("127.0.0.1:{}", self.port)])
            .arg("--allow-development-setup");
        super::server_web(&mut command, web, &self.root);
        let server = super::server_launcher::launch(&mut command, &self.root.join(log))?;
        let ready = wait_for("protected management fixture listener", || {
            if http(self.port, "GET", "/", None)?.0 == 200 {
                Ok(())
            } else {
                Err("fixture listener not ready".into())
            }
        });
        if let Err(error) = ready {
            return server.finish(|| Err(error)).map(|()| unreachable!());
        }
        Ok(server)
    }
    fn visit(&self, browser: &Browser) -> Result<()> {
        browser.command("DELETE", "/cookie", None)?;
        browser.command(
            "POST",
            "/url",
            Some(json!({"url":format!("http://127.0.0.1:{}",self.port)})),
        )?;
        browser
            .css(".authentication-card > form input[autocomplete='username']:not([disabled]):not([readonly])")?;
        Ok(())
    }
    fn open(&self, browser: &Browser) -> Result<()> {
        self.visit(browser)?;
        self.authorize(browser)?;
        browser.css("[data-first-account-form] [data-setup-user]:not([disabled])")?;
        browser.css("[data-first-account-form] [data-setup-password]:not([disabled])")?;
        Ok(())
    }
    fn authorize(&self, browser: &Browser) -> Result<()> {
        browser.click(&browser.css("[data-setup-check]:not([disabled])")?)?;
        Ok(())
    }
    fn wait_state(&self, expected: &str) -> Result<()> {
        wait_for("real management and storage lifecycle state", || {
            let (status, bytes) = http(self.port, "GET", "/api/management/status", None)?;
            let value: json_codec::Value = json_codec::from_slice(&bytes)?;
            if status == 200
                && value["authentication"] == "ready"
                && value["storage"]["state"] == expected
                && value["storage"]["recovering"] == false
            {
                Ok(())
            } else {
                Err(format!("expected {expected}, actual management status: {value}").into())
            }
        })
    }
    fn no_storage(&self) -> Result<()> {
        if !self.management.join("management-auth.json").is_file()
            || !self.pool.join("management-authority.json").is_file()
            || self.pool.join("config.json").exists()
            || self.pool.join("workspaces").exists()
            || self.root.join("storage-parent").exists()
        {
            return Err(
                "account-only setup created storage or omitted durable management authority".into(),
            );
        }
        Ok(())
    }
    fn storage_directory(&self) -> Result<std::path::PathBuf> {
        use std::os::unix::fs::DirBuilderExt;
        let parent = self.root.join("storage-parent");
        fs::DirBuilder::new().mode(0o700).create(&parent)?;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(parent.join("outside-selection.txt"))?
            .write_all(b"outside selected storage\n")?;
        let storage = parent.join("selected-storage");
        fs::DirBuilder::new().mode(0o700).create(&storage)?;
        Ok(storage)
    }
}

const REAUTHORIZE_PROBE: &str = r#"
if(window.__zxsSetupRecheck)throw new Error('Setup recheck probe already installed');
const original=window.fetch;
const state=window.__zxsSetupRecheck={reads:[],form:document.querySelector('[data-first-account-form]'),
  identity:document.querySelector('[data-setup-user]'),password:document.querySelector('[data-setup-password]'),
  restore:()=>{window.fetch=original;delete window.__zxsSetupRecheck;}};
window.fetch=async function(...args){
  const input=args[0],url=new URL(typeof input === 'string' ? input : input.url,location.href);
  const method=(args[1]?.method || input.method || 'GET').toUpperCase();
  const response=await Reflect.apply(original,this,args);
  if(url.origin === location.origin && url.pathname === '/api/setup/status' && method === 'GET'){
    const body=await response.clone().json();state.reads.push({status:response.status,state:body.state});
  }
  return response;
};return true;
"#;
fn reauthorize_identity(browser: &Browser, username: &str) -> Result<()> {
    browser.script(REAUTHORIZE_PROBE)?;
    with_cleanup(
        || {
            let expected = json_codec::to_string(username)?;
            for count in 1..=2 {
                browser.click(&browser.css("[data-setup-check]:not([disabled])")?)?;
                wait_for(
                    "real setup recheck preserves the displayed identity",
                    || {
                        if browser.script(&format!("const s=window.__zxsSetupRecheck,e=document.querySelector('[data-setup-user]'),check=document.querySelector('[data-setup-check]');return s.reads.length === {count} && s.reads.every(r=>r.status===200&&r.state==='empty') && document.querySelector('[data-first-account-form]') === s.form && document.querySelector('[data-setup-password]') === s.password && e === s.identity && e?.value === {expected} && !e.disabled && check?.disabled === false && !document.querySelector('[data-setup-confirm]')"))? == true { Ok(()) }
                else { Err("real setup recheck replaced or changed the displayed administrator identity".into()) }
                    },
                )?;
            }
            Ok(())
        },
        || {
            browser.script("window.__zxsSetupRecheck.restore();return true")?;
            Ok(())
        },
    )
}
fn credentials(browser: &Browser, username: &str, password: &str) -> Result<()> {
    let user = browser.css("[data-setup-user]")?;
    browser.command("POST", &format!("/element/{user}/clear"), Some(json!({})))?;
    browser.keys(&user, username)?;
    browser.keys(&browser.css("[data-setup-password]")?, password)?;
    browser.css("[data-first-account-form] button[type=submit]:not([disabled])")?;
    Ok(())
}
fn cleared_secrets(browser: &Browser, password: &str) -> Result<()> {
    wait_for("submitted setup secrets cleared", || {
        if browser.script("const key=document.querySelector('[data-setup-key]'); const password=document.querySelector('[data-setup-password]'); return !key && (!password || password.value === '')")? == true { Ok(()) }
        else { Err("setup input retains a submitted secret".into()) }
    })?;
    let persisted = browser.script("return location.href + JSON.stringify(Object.entries(localStorage)) + JSON.stringify(Object.entries(sessionStorage))")?;
    let persisted = persisted
        .as_str()
        .ok_or("browser persistence probe returned non-string")?;
    if persisted.contains(password) {
        return Err("setup secret persisted in browser storage or URL".into());
    }
    Ok(())
}
fn sign_in(browser: &Browser, username: &str, password: &str) -> Result<()> {
    browser.keys(
        &browser
            .css("form[data-auth-form='login'] input[autocomplete='username']:not([disabled]):not([readonly])")?,
        username,
    )?;
    browser.keys(
        &browser.css(
            "form[data-auth-form='login'] input[autocomplete='current-password']:not([disabled]):not([readonly])",
        )?,
        password,
    )?;
    browser
        .click(&browser.css("form[data-auth-form='login'] button[type=submit]:not([disabled])")?)?;
    browser.button("Sign out", "退出")?;
    Ok(())
}
fn sign_out(browser: &Browser) -> Result<()> {
    if browser.script("return window.__zxsSmokeErrors")? != json!([]) {
        return Err("uncaught browser error during management lifecycle".into());
    }
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    Ok(())
}

// Observe real reads and mutations. The optional fault discards only a genuine
// successful activation response, then holds real status responses until the
// unknown-state assertions finish. No successful response/body is fabricated.
const STORAGE_PROBE: &str = r#"
if (window.__zxsStorageProbe) throw new Error('Storage probe already installed');
const original = window.fetch;
const state = window.__zxsStorageProbe = {
  listings:[], inspections:[], posts:[], statusReads:[], activation:null,
  loseActivation:false, dropped:false, holdStatus:false, waiters:[],
  submitStimuli:0, stimuliSettled:true, frame:null,
  release:() => { state.holdStatus=false; for (const resolve of state.waiters.splice(0)) resolve(); },
  restore:() => { state.release(); if(state.frame !== null)cancelAnimationFrame(state.frame); window.fetch=original; delete window.__zxsStorageProbe; }
};
window.fetch = async function(...args) {
  const input=args[0], url=new URL(typeof input === 'string' ? input : input.url, location.href);
  const method=(args[1]?.method || input.method || 'GET').toUpperCase();
  const own=url.origin === location.origin, path=url.pathname;
  if (own && method === 'POST' && ['/api/admin/storage/activate','/api/admin/storage/complete-activation'].includes(path)) state.posts.push(path);
  const response=await Reflect.apply(original,this,args);
  if (!own) return response;
  if (path === '/api/admin/storage/directories' && method === 'GET')
    state.listings.push({status:response.status, body:await response.clone().json()});
  if (path === '/api/admin/storage/directories/inspect' && method === 'POST')
    state.inspections.push({status:response.status, body:await response.clone().json()});
  if (path === '/api/admin/storage/activate' && method === 'POST' && state.loseActivation && !state.dropped) {
    const body=await response.clone().json();
    state.activation={status:response.status, body};
    if (response.ok && body.activated === true) {
      state.dropped=true; state.holdStatus=true;
      throw new TypeError('Browser smoke: activation response lost after successful activation');
    }
  }
  if (path === '/api/admin/storage/status' && method === 'GET') {
    state.statusReads.push({status:response.status, body:await response.clone().json()});
    if (state.holdStatus) await new Promise(resolve => state.waiters.push(resolve));
  }
  return response;
};
return true;
"#;
fn restore_storage_probe(browser: &Browser) -> Result<()> {
    browser.script("if(window.__zxsStorageProbe)window.__zxsStorageProbe.restore();return true")?;
    Ok(())
}
fn select_storage(browser: &Browser, storage: &Path) -> Result<()> {
    let parent = storage
        .parent()
        .ok_or("fixture storage has no parent")?
        .to_str()
        .ok_or("non-UTF8 fixture parent")?;
    let path = storage.to_str().ok_or("non-UTF8 fixture storage")?;
    browser.click(&browser.button("Administration", "系统管理")?)?;
    browser.css(".zxs-storage-activation [data-storage-field='browse']:not([disabled])")?;
    let discovery = browser.css("[data-storage-discovery-root]:not([disabled])")?;
    browser.command(
        "POST",
        &format!("/element/{discovery}/clear"),
        Some(json!({})),
    )?;
    browser.keys(&discovery, parent)?;
    browser.click(&browser.css("[data-storage-save-discovery-root]:not([disabled])")?)?;
    let expected_root = json_codec::to_string(parent)?;
    wait_for("custom discovery root persisted", || {
        if browser.script(&format!("return document.querySelector('[data-storage-field=browse]')?.value === {expected_root} && !document.querySelector('[data-storage-field=browse]')?.disabled"))? == true {Ok(())}
        else {Err("custom discovery root not yet loaded".into())}
    })?;
    let browse = browser.css("[data-storage-field='browse']")?;
    browser.command("POST", &format!("/element/{browse}/clear"), Some(json!({})))?;
    browser.keys(&browse, parent)?;
    browser.click(&browser.button("Browse", "浏览")?)?;
    let expected_parent = json_codec::to_string(parent)?;
    wait_for("real parent directory listing", || {
        if browser.script(&format!("const r=window.__zxsStorageProbe.listings.at(-1);return r?.status === 200 && r.body.path === {expected_parent} && r.body.entries.some(e=>e.name === 'selected-storage' && e.can_browse)"))? == true { Ok(()) }
        else { Err("server did not list the real nested storage directory".into()) }
    })?;
    browser.click(&browser.button("selected-storage", "selected-storage")?)?;
    let expected = json_codec::to_string(path)?;
    wait_for("real child directory listing", || {
        if browser.script(&format!("const r=window.__zxsStorageProbe.listings.at(-1);return r?.status === 200 && r.body.path === {expected} && r.body.entries.length === 0 && r.body.truncated === false"))? == true { Ok(()) }
        else { Err("child directory listing not yet available".into()) }
    })?;
    browser.click(&browser.button("Use this directory", "使用此目录")?)?;
    wait_for("mount and selected child remain distinct", || {
        if browser.script(&format!("const r=window.__zxsStorageProbe.listings.at(-1).body;return document.querySelector('[data-storage-field=mount]').value === r.mount_path && document.querySelector('[data-storage-field=root]').value === {expected} && r.mount_path !== {expected}"))? == true { Ok(()) }
        else { Err("directory selection lost the actual mount/child distinction".into()) }
    })?;
    browser.click(&browser.css("[data-storage-inspect]")?)?;
    browser.css("[data-storage-add-selection]:not([disabled])")?;
    let inspected=browser.script("const s=window.__zxsStorageProbe;return {inspection:s.inspections.at(-1), listing:s.listings.at(-1).body}")?;
    if inspected["inspection"]["status"] != 200
        || inspected["inspection"]["body"]["root_path"] != path
        || inspected["inspection"]["body"]["mount_path"] != inspected["listing"]["mount_path"]
        || !inspected["inspection"]["body"]["selection_id"]
            .as_str()
            .is_some_and(|s| s.starts_with("directory_") && s.len() == 74)
    {
        return Err(
            format!("real directory inspection did not bind selected child: {inspected}").into(),
        );
    }
    browser.click(&browser.css("[data-storage-add-selection]")?)?;
    if browser.script("const t=document.querySelector('.zxs-storage-activation').textContent;return t.includes('One directory holds one copy') || t.includes('单目录只有一份副本')")? != true {
        return Err("single-directory redundancy warning is missing".into());
    }
    browser.click(&browser.css("[data-storage-development]")?)?;
    browser.click(&browser.css("[data-storage-confirm]")?)?;
    browser.css("[data-storage-activate]:not([disabled])")?;
    Ok(())
}
fn storage_active(browser: &Browser) -> Result<()> {
    wait_for("actual activation status settles the Web view", || {
        if browser.script("const p=window.__zxsStorageProbe;const t=document.querySelector('.zxs-storage-activation')?.textContent || '';return p.statusReads.some(r=>r.status === 200 && r.body.state === 'active') && (t.includes('The storage pool is active.') || t.includes('存储池已激活。')) && !document.querySelector('[data-storage-activate]') && !document.querySelector('[data-language-toggle]')?.disabled")? == true { Ok(()) }
        else { Err("activation has not settled from real server status".into()) }
    })
}
fn assert_activation_posts(browser: &Browser, expected: &[&str]) -> Result<()> {
    let posts = browser.script("return window.__zxsStorageProbe.posts")?;
    if posts != json_codec::to_value(expected)? {
        return Err(format!("unexpected storage mutation sends: {posts}").into());
    }
    Ok(())
}
fn assert_single_storage(fixture: &SetupFixture, storage: &Path) -> Result<()> {
    let config: storage_engine::Config =
        json_codec::from_slice(&fs::read(fixture.pool.join("config.json"))?)?;
    if config.disks.len() != 1
        || config.disks[0].level != 1
        || config.disks[0].upstream_slot_id.is_some()
        || config.disks[0].root != storage
    {
        return Err("activation changed selected root or invented backup levels".into());
    }
    if fs::read(
        storage
            .parent()
            .ok_or("fixture storage has no parent")?
            .join("outside-selection.txt"),
    )? != b"outside selected storage\n"
    {
        return Err("activation changed data outside selected storage".into());
    }
    Ok(())
}

const LOST_INITIALIZE_PROBE: &str = r#"
const originalFetch=window.fetch;
const form=document.querySelector('[data-first-account-form]');
if (!form?.isConnected || window.__zxsLostInitialize) throw new Error('Lost initialization probe requires one live setup form');
const state=window.__zxsLostInitialize={
  form,setupPosts:[],initialization:null,statusReads:[],logins:[],dropped:false,
  submitStimuli:0,stimuliSettled:true,frame:null,
  restore:()=>{if(state.frame !== null)cancelAnimationFrame(state.frame);window.fetch=originalFetch;delete window.__zxsLostInitialize;}
};
window.fetch=async function(...args){
  const request=args[0],url=new URL(typeof request === 'string' ? request : request.url,location.href);
  const method=(args[1]?.method || request.method || 'GET').toUpperCase();
  if(url.origin === location.origin && url.pathname.startsWith('/api/setup/') && method === 'POST')state.setupPosts.push(url.pathname);
  const response=await Reflect.apply(originalFetch,this,args);
  if(url.origin !== location.origin)return response;
  if(url.pathname === '/api/setup/initialize' && method === 'POST' && !state.dropped){
    const body=await response.clone().json();
    state.initialization={status:response.status,initialized:body.initialized === true,storagePending:body.storage_pending === true};
    if(response.ok && body.initialized === true && body.storage_pending === true){
      state.dropped=true;
      throw new TypeError('Browser smoke: initialize response lost after successful initialization');
    }
  } else if(url.pathname === '/api/setup/status' && method === 'GET'){
    const body=await response.clone().json();state.statusReads.push({status:response.status,state:body.state});
  } else if(url.pathname === '/api/auth/login' && method === 'POST'){
    const body=await response.clone().json();state.logins.push({status:response.status,principal:body.principal});
  }
  return response;
};
return true;
"#;
const LOST_INITIALIZE_STATE: &str = r#"
const state=window.__zxsLostInitialize;
if(!state)throw new Error('Lost initialization probe is missing');
const key=document.querySelector('[data-setup-key]'),form=document.querySelector('[data-first-account-form]');
const login=document.querySelector('form[data-auth-form="login"]'),password=form?.querySelector('[data-setup-password]'),text=form?.textContent || '';
const visible=node=>node.checkVisibility({opacityProperty:true,visibilityProperty:true});
const forms=Array.from(document.querySelectorAll('.authentication-card > form'));
const loginReady=forms.length===1 && forms[0]===login && !!login?.isConnected && visible(login)
  && ['[name=username]','[name=password]','button[type=submit]'].every(selector=>{
    const node=login.querySelector(selector);return !!node?.isConnected && !node.disabled && !node.readOnly && visible(node);
  });
const complete=!form && !state.form.isConnected && loginReady;
const submits=Array.from(form?.querySelectorAll('button[type=submit]') || []);
return {sameForm:form === state.form && state.form.isConnected,
  unknown:!!form?.querySelector('[data-setup-unknown]') && Array.from(form?.querySelectorAll('[role=alert]') || []).some(e=>visible(e)&&/Outcome unconfirmed\. Check setup status\.|结果未确认，请检查设置状态。/.test(e.textContent)),
  complete,
  transportError:text.includes('Browser smoke: initialize response lost after successful initialization'),
  keyAbsent:!key,passwordCleared:!password || password.value === '',
  settled:form ? form.querySelector('[data-setup-check]')?.disabled === false : complete,
  noInitialization:form ? submits.length===1 && submits[0].disabled && visible(submits[0]) && !form.querySelector('[data-setup-disk], [data-setup-completion]') : complete,
  reloadBlocked:document.querySelector('[data-language-toggle]')?.disabled === true,
  dropped:state.dropped,initialization:state.initialization,setupPosts:state.setupPosts,statusReads:state.statusReads,logins:state.logins,
  submitStimuli:state.submitStimuli,stimuliSettled:state.stimuliSettled};
"#;
fn lost_initialize_state(browser: &Browser) -> Result<json_codec::Value> {
    browser.script(LOST_INITIALIZE_STATE)
}
fn unknown_initialize_fence(browser: &Browser) -> Result<json_codec::Value> {
    let state = lost_initialize_state(browser)?;
    if state["sameForm"] != true
        || state["unknown"] != true
        || state["settled"] != true
        || state["noInitialization"] != true
        || state["reloadBlocked"] != true
        || state["dropped"] != true
        || state["initialization"] != json!({"status":200,"initialized":true,"storagePending":true})
        || state["setupPosts"] != json!(["/api/setup/initialize"])
    {
        return Err(format!("lost initialization escaped its unknown fence: {state}").into());
    }
    Ok(state)
}
fn repeat_unknown_initialize(browser: &Browser, expected_stimuli: u64) -> Result<()> {
    browser.script(r#"
const state=window.__zxsLostInitialize,form=document.querySelector('[data-first-account-form]'),language=document.querySelector('[data-language-toggle]');
if(!state || form !== state.form || !form.isConnected || !language?.disabled)throw new Error('Unknown setup form lost its fence');
for(let i=0;i<3;i++){form.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));state.submitStimuli++;}
language.dispatchEvent(new MouseEvent('click',{bubbles:true}));state.stimuliSettled=false;
state.frame=requestAnimationFrame(()=>{state.frame=null;state.stimuliSettled=true;});return true;
"#)?;
    wait_for("repeated unknown account submissions stay fenced", || {
        let state = unknown_initialize_fence(browser)?;
        if state["stimuliSettled"] == true
            && state["submitStimuli"].as_u64() == Some(expected_stimuli)
        {
            Ok(())
        } else {
            Err(format!("unknown account stimulus pending: {state}").into())
        }
    })
}
fn lost_initialize_response(
    browser: &Browser,
    daemon: &Path,
    web: Option<&Path>,
    root: &Path,
    password: &str,
) -> Result<()> {
    let fixture = SetupFixture::new(root, "lost-initialize-response")?;
    let server = fixture.launch(daemon, web, "server.log")?;
    server.finish(|| {
        fixture.open(browser)?;
        credentials(browser,"lost-response-admin",password)?;
        with_cleanup(|| {
            browser.script(LOST_INITIALIZE_PROBE)?;
            browser.click(&browser.css("[data-first-account-form] button[type=submit]:not([disabled])")?)?;
            wait_for("successful account response loss becomes unknown", || {
                let state=unknown_initialize_fence(browser)?;
                if state["transportError"]==true && state["keyAbsent"]==true && state["passwordCleared"]==true {Ok(())}
                else {Err(format!("account response loss not yet visible: {state}").into())}
            })?;
            let previews = std::env::var_os("ZXS_LOGIN_PREVIEWS")
                .map(|dir| std::path::PathBuf::from(dir).join("first-account-unknown"));
            super::auth_layout::unconfirmed(browser, previews.as_deref())?;
            fixture.wait_state("unconfigured")?;
            fixture.no_storage()?;
            let authority=fs::read(fixture.pool.join("management-authority.json"))?;
            let account=fs::read(fixture.management.join("management-auth.json"))?;
            repeat_unknown_initialize(browser,3)?;
            browser.click(&browser.css("[data-setup-clear]:not([disabled])")?)?;
            let cleared=unknown_initialize_fence(browser)?;
            if cleared["keyAbsent"]!=true || cleared["passwordCleared"]!=true {
                return Err("clear setup inputs retained credentials".into());
            }
            repeat_unknown_initialize(browser,6)?;
            fixture.authorize(browser)?;
            wait_for("independent management-ready evidence settles account creation", || {
                let state=lost_initialize_state(browser)?;
                if state["statusReads"]==json!([{"status":200,"state":"management_ready"}])
                    && state["complete"]==true && state["unknown"]==false && state["reloadBlocked"]==false
                    && state["noInitialization"]==true && state["keyAbsent"]==true && state["passwordCleared"]==true
                    && state["setupPosts"]==json!(["/api/setup/initialize"]) {Ok(())}
                else {Err(format!("account authority not independently confirmed: {state}").into())}
            })?;
            cleared_secrets(browser,password)?;
            if fs::read(fixture.management.join("management-auth.json"))?!=account
                || fs::read(fixture.pool.join("management-authority.json"))?!=authority {
                return Err("account result recheck replaced installed authority".into());
            }
            sign_in(browser,"lost-response-admin",password)?;
            let recovered=browser.script("return {posts:window.__zxsLostInitialize.setupPosts,logins:window.__zxsLostInitialize.logins}")?;
            if recovered["posts"]!=json!(["/api/setup/initialize"])
                || recovered["logins"]!=json!([{"status":200,"principal":"lost-response-admin"}]) {
                return Err(format!("original account recovery sent unexpected requests: {recovered}").into());
            }
            Ok(())
        },|| {
            browser.script("if(window.__zxsLostInitialize)window.__zxsLostInitialize.restore();return true")?;
            Ok(())
        })?;
        // The original account now activates its first nested storage directory.
        let storage=fixture.storage_directory()?;
        with_cleanup(|| {
            browser.script(STORAGE_PROBE)?;
            select_storage(browser,&storage)?;
            browser.script("window.__zxsStorageProbe.loseActivation=true;return true")?;
            browser.click(&browser.css("[data-storage-activate]")?)?;
            wait_for("successful activation response loss stays unknown",|| {
                let state=unknown_activation_state(browser)?;
                if state["unknown"]==true && state["disabled"]==true && state["reloadBlocked"]==true
                    && state["dropped"]==true && state["actualSuccess"]==true && state["heldStatus"]==true {Ok(())}
                else {Err(format!("activation unknown fence not ready: {state}").into())}
            })?;
            fixture.wait_state("ready")?;
            let config=fs::read(fixture.pool.join("config.json"))?;
            let authority=fs::read(fixture.pool.join("management-authority.json"))?;
            browser.script(r#"
const state=window.__zxsStorageProbe,button=document.querySelector('[data-storage-activate]'),language=document.querySelector('[data-language-toggle]');
if(!button?.disabled || !language?.disabled)throw new Error('Unknown activation lost submission or reload fence');
for(let i=0;i<3;i++){button.dispatchEvent(new MouseEvent('click',{bubbles:true}));state.submitStimuli++;}
language.dispatchEvent(new MouseEvent('click',{bubbles:true}));state.stimuliSettled=false;
state.frame=requestAnimationFrame(()=>{state.frame=null;state.stimuliSettled=true;});return true;
"#)?;
            wait_for("unknown activation rejects repeated submit stimuli",|| {
                let state=unknown_activation_state(browser)?;
                assert_activation_posts(browser,&["/api/admin/storage/activate"])?;
                if state["unknown"]==true && state["disabled"]==true && state["reloadBlocked"]==true
                    && state["submitStimuli"]==3 && state["stimuliSettled"]==true {Ok(())}
                else {Err(format!("activation replay fence not settled: {state}").into())}
            })?;
            // Navigation must not erase the submitted operation or its fence.
            browser.click(&browser.button("Status","状态")?)?;
            browser.css(".zxs-monitor-page [data-metric='cpu']")?;
            browser.click(&browser.button("Administration","系统管理")?)?;
            wait_for("activation unknown state survives page navigation",|| {
                let state=unknown_activation_state(browser)?;
                if state["unknown"]==true && state["disabled"]==true && state["reloadBlocked"]==true {Ok(())}
                else {Err(format!("navigation lost activation fence: {state}").into())}
            })?;
            browser.script("window.__zxsStorageProbe.release();return true")?;
            storage_active(browser)?;
            assert_activation_posts(browser,&["/api/admin/storage/activate"])?;
            if fs::read(fixture.pool.join("config.json"))?!=config
                || fs::read(fixture.pool.join("management-authority.json"))?!=authority {
                return Err("activation result recovery changed pool configuration or authority".into());
            }
            assert_single_storage(&fixture,&storage)?;
            Ok(())
        },||restore_storage_probe(browser))?;
        sign_out(browser)?;
        println!("PASS real account and activation response loss: one POST per mutation, repeated submission/reload fences, account-authority recheck, activation fence across navigation, and real active-status recovery");
        Ok(())
    })
}
fn unknown_activation_state(browser: &Browser) -> Result<json_codec::Value> {
    browser.script(r#"
const s=window.__zxsStorageProbe,text=document.querySelector('.zxs-storage-activation')?.textContent || '';
if(!s)throw new Error('Storage probe missing');
return {unknown:/Activation outcome is unknown; resubmission is blocked|激活结果未知，禁止重新提交/.test(text),
  disabled:document.querySelector('[data-storage-activate]')?.disabled === true,
  reloadBlocked:document.querySelector('[data-language-toggle]')?.disabled === true,
  dropped:s.dropped,actualSuccess:s.activation?.status === 200 && s.activation.body.activated === true,
  heldStatus:s.waiters.length > 0,submitStimuli:s.submitStimuli,stimuliSettled:s.stimuliSettled};
"#)
}

fn complete_activation(
    browser: &Browser,
    daemon: &Path,
    web: Option<&Path>,
    root: &Path,
    password: &str,
) -> Result<()> {
    let fixture = SetupFixture::new(root, "interrupted-storage-activation")?;
    let storage = prepare_interruption(&fixture, password)?;
    let config = fs::read(fixture.pool.join("config.json"))?;
    let authority = fs::read(fixture.pool.join("management-authority.json"))?;
    let server = fixture.launch(daemon, web, "server.log")?;
    server.finish(|| {
        fixture.wait_state("recovery_required")?;
        fixture.visit(browser)?;
        sign_in(browser,"completion-admin",password)?;
        with_cleanup(|| {
            browser.script(STORAGE_PROBE)?;
            browser.click(&browser.button("Administration","系统管理")?)?;
            browser.css("[data-storage-complete]")?;
            if browser.script("return document.querySelector('[data-storage-activate], [data-setup-user]') !== null")? == true {
                return Err("interrupted activation offered new storage/account initialization".into());
            }
            browser.click(&browser.css("[data-storage-complete]")?)?;
            fixture.wait_state("ready")?;
            storage_active(browser)?;
            assert_activation_posts(browser,&["/api/admin/storage/complete-activation"])?;
            if fs::read(fixture.pool.join("config.json"))?!=config
                || fs::read(fixture.pool.join("management-authority.json"))?!=authority {
                return Err("bounded activation completion changed existing configuration or authority".into());
            }
            assert_single_storage(&fixture,&storage)?;
            Ok(())
        },||restore_storage_probe(browser))?;
        sign_out(browser)?;
        sign_in(browser,"completion-admin",password)?;
        browser.click(&browser.button("Files","文件")?)?;
        browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
        sign_out(browser)?;
        println!("PASS real durable interrupted activation: existing-account login, server-verified original completion, unchanged configuration/authority and usable storage; no debug endpoint");
        Ok(())
    })
}

// This callback fails only after real storage genesis and its proof are durable,
// before credential-replica installation. The daemon has no debug/failure route.
fn prepare_interruption(fixture: &SetupFixture, password: &str) -> Result<std::path::PathBuf> {
    use std::sync::{Arc, Mutex};
    use storage_server::{
        Backend,
        setup_recovery::{SetupRecovery, StorageActivationRequest},
    };
    let (backend, auth) = storage_server::management::open_service_with_disks(
        &fixture.pool,
        &fixture.management,
        std::collections::BTreeMap::new(),
        true,
    )?;
    let auth = Arc::new(Mutex::new(auth));
    backend.start_management(auth.clone());
    let mut prepared = None;
    with_cleanup(
        || {
            wait_for("account fixture storage discovery", || {
                if !backend.management_status().recovering() {
                    Ok(())
                } else {
                    Err("fixture discovery pending".into())
                }
            })?;
            backend.setup_operation(
                "initialize",
                json!({"username":"completion-admin","password":password}),
                None,
                None,
            )?;
            let session = auth.lock().unwrap().login("completion-admin", password)?;
            let identity = auth.lock().unwrap().authenticate(&session.token)?;
            prepared = Some((session, identity));
            Ok(())
        },
        || Ok(backend.drain_management()?),
    )?;
    // Cleanup joins the observer even if account creation failed.
    let (session, identity) = prepared.ok_or("completed fixture omitted authenticated identity")?;
    drop(backend);
    fixture.no_storage()?;
    let storage = fixture.storage_directory()?;
    let path = storage.to_str().ok_or("non-UTF8 fixture directory")?;
    let discovery = storage
        .parent()
        .ok_or("fixture storage needs parent")?
        .to_str()
        .ok_or("non-UTF8 discovery root")?;
    let listing = storage_server::storage_directories::browse_within(discovery, path)?;
    let selection = storage_server::storage_directories::inspect_within(
        discovery,
        &json_codec::from_value(json!({"mount_path":listing.mount_path,"root_path":path}))?,
        &[fixture.pool.clone(), fixture.management.clone()],
    )?;
    let service = SetupRecovery::new(
        fixture.pool.clone(),
        fixture.management.clone(),
        std::collections::BTreeMap::new(),
    )?
    .allow_development_mode();
    service.set_discovery_root(
        &identity,
        json_codec::from_value(
            json!({"expected_discovery_root":"/zxs/data/disks","discovery_root":discovery}),
        )?,
        |_| Ok(()),
    )?;
    let result = service.activate(
        &identity,
        StorageActivationRequest {
            selections: vec![selection],
            mode: storage_engine::Mode::Development,
        },
        |_| {
            auth.lock()
                .unwrap()
                .authenticate(&session.token)
                .map(|_| ())
        },
        |_engine| {
            Err(storage_server::ApiError::new(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_engine::ErrorCode::InjectedFailure,
                "fixture interruption before credential replica installation",
            ))
        },
    );
    if !matches!(result,Err(error) if error.code==storage_engine::ErrorCode::InjectedFailure)
        || !service.storage_status().completion_available
    {
        return Err("interrupted storage fixture lacks genuine bound activation proof".into());
    }
    Ok(storage)
}

#[cfg(test)]
#[path = "../unit/offline_management_tests.rs"]
mod offline_management_tests;
