//! Real second-daemon acceptance: persisted control-plane credentials, corrupt
//! pool configuration, production Web/WASM, and real browser login/retry.
use super::{
    AdminPage, Browser, Command, OpenOptions, OpenOptionsExt, Path, Result, Write, fs, http, json,
    port, wait_for, with_cleanup,
};

mod primary_browse;

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
    // Copy the existing independent credential authority and its original
    // SYSTEM marker. Normal startup must not create the marker for this test.
    for (source, destination) in [
        (
            source_pool.join("management/management-auth.json"),
            management.join("management-auth.json"),
        ),
        (
            source_pool.join("management-authority.json"),
            pool.join("management-authority.json"),
        ),
    ] {
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(destination)?
            .write_all(&fs::read(source)?)?;
    }
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(pool.join("config.json"))?
        .write_all(b"{corrupt pool configuration")?;
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
    AdminPage::Diagnostics.open(browser)?;
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

// First installation is exercised by the delivered helper in the isolated
// Debian acceptance. This browser fixture only opens already prepared storage.
pub(super) fn prepared_primary(
    browser: &Browser,
    daemon: &Path,
    web: Option<&Path>,
    root: &Path,
    password: &str,
) -> Result<()> {
    let fixture = PreparedFixture::new(root, "prepared-primary")?;
    let storage = prepare_primary(&fixture, password)?;
    let server = fixture.launch(daemon, web, "server.log")?;
    server.finish(|| {
        fixture.wait_state("ready")?;
        fixture.visit(browser)?;
        // Login starts the automatic disk read; observe before it can complete.
        primary_browse::observe(browser)?;
        with_cleanup(
            || {
                sign_in(browser, "prepared-admin", password)?;
                AdminPage::Storage.open(browser)?;
                primary_browse::run(browser, &storage)?;
                assert_single_storage(&fixture, &storage)
            },
            || primary_browse::restore(browser),
        )?;
        sign_out(browser)
    })?;
    // A disappearing selected child must remain missing on restart. Its parent
    // and mount still exist, so accepting the mount alone would be a real defect.
    fs::rename(&storage, storage.with_file_name("selected-storage-offline"))?;
    let restarted = fixture.launch(daemon, web, "missing-directory-server.log")?;
    restarted.finish(|| {
        fixture.wait_state("unavailable")?;
        fixture.visit(browser)?;
        sign_in(browser, "prepared-admin", password)?;
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
        "PASS offline-prepared primary login/browse and missing-directory restart without recreation"
    );
    Ok(())
}

// Exercise the delivered daemon rather than the in-process startup helper.
// A development flag must not turn an empty system into online installation.
pub(super) fn reject_empty(daemon: &Path, web: Option<&Path>, root: &Path) -> Result<()> {
    for label in ["missing-system", "empty-system", "empty-development-system"] {
        let fixture = PreparedFixture::new(root, label)?;
        let missing = label == "missing-system";
        if missing {
            fs::remove_dir(&fixture.pool)?;
            fs::remove_dir(&fixture.management)?;
        }
        let log = fixture.root.join("server.log");
        let mut command = fixture.command(daemon, web);
        if label == "empty-development-system" {
            command.arg("--allow-development-setup");
        }
        let mut server = super::server_launcher::launch(&mut command, &log)?;
        let exited = server.exit_code();
        server.finish(|| {
            if exited? != 1 {
                return Err("unprepared daemon did not exit with the startup rejection".into());
            }
            Ok(())
        })?;
        let diagnostic = fs::read_to_string(&log)?;
        let expected = if missing {
            "normal ZXS requires an existing safe private directory"
        } else {
            "normal ZXS cannot start without an existing storage configuration"
        };
        if !diagnostic.contains(expected) {
            return Err(format!("wrong cold startup rejection for {label}: {diagnostic}").into());
        }
        match std::net::TcpStream::connect(("127.0.0.1", fixture.port)) {
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
            result => return Err(format!("unprepared listener was not closed: {result:?}").into()),
        }
        for path in [&fixture.pool, &fixture.management] {
            if missing {
                match fs::symlink_metadata(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    result => {
                        return Err(format!(
                            "startup created missing directory {path:?}: {result:?}"
                        )
                        .into());
                    }
                }
            } else if fs::read_dir(path)?.next().is_some() {
                return Err(format!("startup wrote to empty directory {path:?}").into());
            }
        }
        match fs::symlink_metadata(fixture.root.join("storage-parent")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            result => return Err(format!("startup created discovery root: {result:?}").into()),
        }
    }
    println!(
        "PASS missing/empty/development startup refuses before listener or initialization writes"
    );
    Ok(())
}

struct PreparedFixture {
    root: std::path::PathBuf,
    pool: std::path::PathBuf,
    management: std::path::PathBuf,
    port: u16,
}
impl PreparedFixture {
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
        let mut command = self.command(daemon, web);
        let server = super::server_launcher::launch(&mut command, &self.root.join(log))?;
        let ready = wait_for("protected management fixture listener", || {
            if http(self.port, "GET", "/", None)?.0 == 200 {
                Ok(())
            } else {
                Err("fixture listener not ready".into())
            }
        });
        if let Err(error) = ready {
            let failure = server
                .finish(|| Err(error))
                .expect_err("invariant: readiness failed");
            let diagnostic = fs::read_to_string(self.root.join(log))?;
            return Err(format!("{failure}; prepared fixture log:\n{diagnostic}").into());
        }
        Ok(server)
    }
    fn command(&self, daemon: &Path, web: Option<&Path>) -> Command {
        let mut command = Command::new(daemon);
        command
            .arg("--root")
            .arg(&self.pool)
            .arg("--management-root")
            .arg(&self.management)
            .arg("--storage-discovery-root")
            .arg(self.root.join("storage-parent"))
            .args(["--listen", &format!("127.0.0.1:{}", self.port)]);
        super::server_web(&mut command, web, &self.root);
        command
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

fn assert_single_storage(fixture: &PreparedFixture, storage: &Path) -> Result<()> {
    let config: storage_engine::Config =
        json_codec::from_slice(&fs::read(fixture.pool.join("config.json"))?)?;
    if config.disks.len() != 1
        || config.disks[0].slot_id != "D01"
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

// Prepare disposable browser data through the existing account/activation
// implementation before launching the daemon. This is fixture construction,
// not evidence that the helper UI or a real machine installation has passed.
fn prepare_primary(fixture: &PreparedFixture, password: &str) -> Result<std::path::PathBuf> {
    use std::sync::{Arc, Mutex};
    use storage_server::{Backend, setup_recovery::StorageActivationRequest};
    let storage = fixture.storage_directory()?;
    let discovery = fixture.root.join("storage-parent");
    let (backend, auth) = storage_server::management::open_service_with_options(
        &fixture.pool,
        &fixture.management,
        storage_server::management::ManagementOptions {
            discovery_root: discovery
                .to_str()
                .ok_or("non-UTF8 discovery root")?
                .parse()?,
            allow_development_setup: true,
            ..Default::default()
        },
    )?;
    let auth = Arc::new(Mutex::new(auth));
    backend.start_management(auth.clone());
    with_cleanup(
        || {
            wait_for("offline fixture account observation", || {
                if !backend.management_status().recovering() {
                    Ok(())
                } else {
                    Err("fixture discovery pending".into())
                }
            })?;
            backend.setup_operation(
                "initialize",
                json!({"username":"prepared-admin","password":password}),
                None,
                None,
            )?;
            let session = auth.lock().unwrap().login("prepared-admin", password)?;
            let identity = auth.lock().unwrap().authenticate(&session.token)?;
            let path = storage.to_str().ok_or("non-UTF8 storage root")?;
            let listing = storage_server::storage_directories::browse(path)?;
            let selection = storage_server::storage_directories::inspect_within(
                discovery.to_str().ok_or("non-UTF8 discovery root")?,
                &json_codec::from_value(json!({"mount_path":listing.mount_path,"root_path":path}))?,
                &[fixture.pool.clone(), fixture.management.clone()],
            )?;
            backend.setup_operation(
                "activate",
                json_codec::to_value(StorageActivationRequest {
                    selections: vec![selection],
                    mode: storage_engine::Mode::Development,
                })?,
                None,
                Some(&identity),
            )?;
            wait_for("offline fixture activation", || {
                let status = backend.management_status();
                if status.state() == storage_server::StorageState::Ready && !status.recovering() {
                    Ok(())
                } else {
                    Err(format!("fixture activation not ready: {status:?}").into())
                }
            })
        },
        || Ok(backend.drain_management()?),
    )?;
    drop(backend);
    drop(auth);
    storage_server::startup_admission::PreparedService::open(&fixture.pool, &fixture.management)?;
    assert_single_storage(fixture, &storage)?;
    Ok(storage)
}

#[cfg(test)]
#[path = "../unit/offline_management_tests.rs"]
mod offline_management_tests;
