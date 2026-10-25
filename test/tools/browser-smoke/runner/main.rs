//! Real Chrome/WebDriver smoke with the production server/WASM. Explicit
//! synthetic delivery fixtures test browser lifetimes, not backend correctness.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use json_codec::{Value, json};
use std::{
    env,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
#[cfg(test)]
#[path = "../unit/base64_compatibility_tests.rs"]
mod base64_compatibility_tests;

mod accessibility;
mod diagnostics;
mod file_pagination;
mod file_rows;
mod fixture;
use fixture::seed_files;
mod fonts;
mod lifecycle;
mod management;
mod metadata;
mod session_delivery;
use metadata::{assert_metadata_only, metadata_edit, open_fixture_directory};
mod file_viewports;
mod session_logout;
mod single_account;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ELEMENT: &str = "element-6066-11e4-a52e-4f735466cecf";

// Each process owns a new group, so failed browser startup cannot leave Chrome children behind.
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let group = format!("-{}", self.0.id());
        let _ = Command::new("kill")
            .args(["-TERM", "--", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        thread::sleep(Duration::from_millis(100));
        let _ = Command::new("kill")
            .args(["-KILL", "--", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(command: &mut Command, log: &Path) -> Result<Process> {
    let output = File::create(log)?;
    Ok(Process(
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output)
            .spawn()?,
    ))
}
fn port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

// ChromeDriver speaks HTTP/1.1 with Content-Length. Keep this tiny client local-only.
fn http(port: u16, method: &str, path: &str, body: Option<Value>) -> Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse()?,
        Duration::from_secs(2),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let body = body.map(|v| v.to_string()).unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    read_http(&mut stream)
}

// HTTP framing, not socket EOF, marks the end of a response. ChromeDriver keeps
// its socket open even when the request asks for Connection: close.
fn read_http(reader: &mut impl Read) -> Result<(u16, Vec<u8>)> {
    const MAX_HEADERS: usize = 64 * 1024;
    const MAX_BODY: usize = 64 * 1024 * 1024;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= MAX_HEADERS {
            return Err("HTTP headers exceed size limit".into());
        }
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        header.push(byte[0]);
    }
    let header = std::str::from_utf8(&header)?;
    let mut lines = header.split("\r\n");
    let mut status_line = lines
        .next()
        .ok_or("missing HTTP status")?
        .split_whitespace();
    if !matches!(status_line.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
        return Err("invalid HTTP version".into());
    }
    let status: u16 = status_line.next().ok_or("missing HTTP status")?.parse()?;
    let mut content_length = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("malformed HTTP header")?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("unsupported transfer-encoding in local HTTP response".into());
        }
        if name.eq_ignore_ascii_case("content-length") {
            let length: usize = value.trim().parse()?;
            if content_length.replace(length).is_some() {
                return Err("duplicate HTTP Content-Length".into());
            }
        }
    }
    let length = match content_length {
        Some(length) => length,
        None if status == 204 || status == 304 => 0,
        None => return Err("local HTTP response requires Content-Length".into()),
    };
    if length > MAX_BODY {
        return Err("HTTP response body exceeds size limit".into());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok((status, body))
}

fn wait_for<T>(label: &str, mut check: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match check() {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() >= deadline => {
                return Err(format!("timed out waiting for {label}: {error}").into());
            }
            Err(_) => thread::sleep(Duration::from_millis(150)),
        }
    }
}
struct Browser {
    port: u16,
    session: String,
}
impl Browser {
    fn command(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let (status, bytes) = http(
            self.port,
            method,
            &format!("/session/{}{path}", self.session),
            body,
        )?;
        let response: Value = json_codec::from_slice(&bytes)?;
        if status >= 400 || response["value"]["error"].is_string() {
            return Err(format!("WebDriver {method} {path}: {}", response["value"]).into());
        }
        Ok(response["value"].clone())
    }
    fn element(&self, using: &str, selector: &str) -> Result<String> {
        wait_for(selector, || {
            let v = self.command(
                "POST",
                "/element",
                Some(json!({"using":using,"value":selector})),
            )?;
            Ok(v[ELEMENT].as_str().ok_or("missing element id")?.to_owned())
        })
    }
    fn css(&self, selector: &str) -> Result<String> {
        self.element("css selector", selector)
    }
    fn button(&self, en: &str, zh: &str) -> Result<String> {
        self.element("xpath", &format!("//button[normalize-space(.)='{en}' or normalize-space(.)='{zh}' or @aria-label='{en}' or @aria-label='{zh}']"))
    }
    fn click(&self, element: &str) -> Result<()> {
        wait_for("enabled clickable element", || {
            self.command(
                "POST",
                &format!("/element/{element}/click"),
                Some(json!({})),
            )
        })?;
        Ok(())
    }
    fn keys(&self, element: &str, text: &str) -> Result<()> {
        self.command(
            "POST",
            &format!("/element/{element}/value"),
            Some(json!({"text":text})),
        )?;
        Ok(())
    }
    fn cdp(&self, cmd: &str, params: Value) -> Result<Value> {
        self.command(
            "POST",
            "/goog/cdp/execute",
            Some(json!({"cmd":cmd,"params":params})),
        )
    }
    fn script(&self, script: &str) -> Result<Value> {
        self.command(
            "POST",
            "/execute/sync",
            Some(json!({"script":script,"args":[]})),
        )
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.command("DELETE", "", None);
    }
}

// Always restore fault-injection hooks, including on browser/assertion errors.
fn with_cleanup(
    action: impl FnOnce() -> Result<()>,
    cleanup: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let result = action();
    let cleanup_result = cleanup();
    match (result, cleanup_result) {
        (Err(error), Err(cleanup_error)) => {
            eprintln!("fixture cleanup also failed: {cleanup_error}");
            Err(error)
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

fn exercise(
    browser: &Browser,
    origin: &str,
    password: &str,
    screenshot: Option<&Path>,
) -> Result<()> {
    diagnostics::install(browser)?;
    let viewport_previews = env::var_os("ZXS_VIEWPORT_PREVIEWS").map(PathBuf::from);
    let url = if screenshot.is_some() || viewport_previews.is_some() {
        format!("{origin}/?lang=zh-CN")
    } else {
        origin.to_owned()
    };
    browser.command("POST", "/url", Some(json!({"url":url})))?;
    browser.keys(
        &browser.css("input[autocomplete='username']")?,
        "smoke-admin",
    )?;
    browser.keys(
        &browser.css("input[autocomplete='current-password']")?,
        password,
    )?;
    browser.click(&browser.button("Sign in", "登录")?)?;
    let disk = browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?;
    let disks = browser.command(
        "POST",
        "/elements",
        Some(json!({"using":"css selector","value":".zxs-disk-entry"})),
    )?;
    if disks.as_array().map(Vec::len) != Some(3) {
        return Err("expected exactly three development disk directories".into());
    }
    println!("PASS login and three real disk directories");
    browser.click(&disk)?;
    open_fixture_directory(browser)?;
    assert_metadata_only(browser)?;
    println!("PASS engine-seeded file browsing in the real server namespace");
    file_rows::run(browser)?;
    file_pagination::run(browser)?;
    if let Some(path) = screenshot {
        // Save the real regression view without adding presentation-only data.
        wait_for("loaded screenshot fonts", || {
            if browser.script("return document.fonts.status === 'loaded'")? == true {
                Ok(())
            } else {
                Err("fonts still loading".into())
            }
        })?;
        let encoded = browser.command("GET", "/screenshot", None)?;
        let png = STANDARD.decode(encoded.as_str().ok_or("missing screenshot data")?)?;
        if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("WebDriver screenshot is not a PNG".into());
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, png)?;
        println!("PASS real browser screenshot saved to {}", path.display());
    }
    file_viewports::run(browser, viewport_previews.as_deref())?;
    let current_history = session_delivery::begin_paused_history(browser)?;
    session_delivery::finish(current_history, true)?;
    assert_metadata_only(browser)?;
    metadata_edit(browser)?;
    browser.click(&browser.button("History", "历史记录")?)?;
    wait_for("seeded file in activity history", || {
        if browser.script("return Array.from(document.querySelectorAll('.zxs-history-path')).some(e=>e.textContent.includes('browser-smoke.txt'))")? == true { Ok(()) } else { Err("history missing seeded file".into()) }
    })?;
    browser.click(&browser.button("Status", "状态")?)?;
    browser.css(".zxs-monitor-page .zxs-metric-card")?;
    assert_metadata_only(browser)?;
    println!("PASS administrator status page retains real telemetry controls");
    accessibility::run(browser)?;
    single_account::run(browser, password)?;
    session_logout::run(browser, password)?;
    browser.click(&browser.button("Files", "文件")?)?;
    let stale_history = session_delivery::begin_paused_history(browser)?;
    browser.click(&browser.button("Sign out", "退出")?)?;
    browser.css("input[autocomplete='current-password']")?;
    session_delivery::finish(stale_history, false)?;
    println!("PASS late file metadata response cannot escape its original session");
    if browser.script("return window.__zxsSmokeErrors")? != json!([]) {
        return Err("uncaught error before logout/reload".into());
    }
    // Reload exercises server-side session revocation, rather than merely a UI reset.
    browser.command("POST", "/refresh", Some(json!({})))?;
    browser.css("input[autocomplete='current-password']")?;
    println!("PASS history, logout and anonymous reload");
    let errors = browser.script("return window.__zxsSmokeErrors")?;
    if errors != json!([]) {
        return Err(format!("uncaught browser errors: {errors}").into());
    }
    let logs = browser.command("POST", "/log", Some(json!({"type":"browser"})))?;
    for entry in logs.as_array().ok_or("invalid browser log")? {
        if entry["level"] == "SEVERE" && entry["source"] == "javascript" {
            return Err(format!("browser JavaScript error: {}", entry["message"]).into());
        }
    }
    let logs = browser.command("POST", "/log", Some(json!({"type":"performance"})))?;
    let mut responses = Vec::new();
    for entry in logs.as_array().ok_or("invalid performance log")? {
        let event: Value = json_codec::from_str(
            entry["message"]
                .as_str()
                .ok_or("missing network log message")?,
        )?;
        if event["message"]["method"] == "Network.requestWillBeSent" {
            let url = event["message"]["params"]["request"]["url"]
                .as_str()
                .unwrap_or_default();
            if url.starts_with(origin)
                && (url.contains("/api/admin/accounts")
                    || url
                        .split('?')
                        .next()
                        .is_some_and(|path| path.ends_with("/grants")))
            {
                return Err(format!("Web requested a retired account/ACL route: {url}").into());
            }
            if metadata::is_file_body_request(url, origin) {
                return Err(
                    format!("metadata-only Web requested a file-content route: {url}").into(),
                );
            }
        }
        if event["message"]["method"] == "Network.responseReceived" {
            let r = &event["message"]["params"]["response"];
            let url = r["url"].as_str().unwrap_or_default();
            let status = r["status"].as_f64().unwrap_or(0.0) as u64;
            if url.starts_with(origin) {
                if status >= 400
                    && !(url.ends_with("/api/auth/me") && status == 401)
                    && !(url.ends_with("/favicon.ico") && status == 404)
                {
                    return Err(format!("unexpected HTTP {status}: {url}").into());
                }
                responses.push((url.to_owned(), status));
            }
        }
    }
    for route in [
        "/api/auth/login",
        "/api/file-manager/disks",
        "/api/file-manager/history",
        "/api/auth/logout",
    ] {
        if !responses
            .iter()
            .any(|(url, status)| url.ends_with(route) && (200..300).contains(status))
        {
            return Err(format!("no successful real browser response for {route}").into());
        }
    }
    if !responses
        .iter()
        .any(|(url, status)| url.contains(".wasm") && *status == 200)
    {
        return Err("browser did not load the built WASM bundle".into());
    }
    println!("PASS uncaught-error and real HTTP/WASM response checks");
    lifecycle::run(browser, origin)?;
    fonts::run(browser, origin)?;
    Ok(())
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let mut cli_binary = PathBuf::from("target/native/debug/zxs");
    let mut web = PathBuf::from("build/web");
    let mut screenshot: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cli-bin" => cli_binary = args.next().ok_or("--cli-bin requires a path")?.into(),
            "--web-dir" => web = args.next().ok_or("--web-dir requires a path")?.into(),
            "--screenshot" => {
                screenshot = Some(args.next().ok_or("--screenshot requires a path")?.into());
            }
            _ => {
                return Err(format!(
                    "unknown argument {arg}; usage: browser-smoke [--cli-bin PATH] [--web-dir PATH] [--screenshot PNG]"
                )
                .into());
            }
        }
    }
    let cli_binary = cli_binary.canonicalize()?;
    let web = web.canonicalize()?;
    if !web.join("index.html").is_file() {
        return Err("build the real build/web before running this test".into());
    }
    let temp = tempfile::Builder::new()
        .prefix("browser-smoke-")
        .tempdir()?;
    let root = temp.path();
    let pool = root.join("pool");
    let mut entropy = [0u8; 24];
    File::open("/dev/urandom")?.read_exact(&mut entropy)?;
    let password: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    fixture::initialize_management(&temp, &password)?;
    seed_files(&temp)?;
    let server_port = port()?;
    let driver_port = port()?;
    let origin = format!("http://127.0.0.1:{server_port}");
    let server_log = root.join("server.log");
    let driver_log = root.join("driver.log");
    let _server = spawn(
        Command::new(&cli_binary)
            .arg("--root")
            .arg(&pool)
            .args(["--listen", &format!("127.0.0.1:{server_port}"), "--web-dir"])
            .arg(&web),
        &server_log,
    )?;
    let result = (|| -> Result<()> {
        wait_for("persisted fixture authority and storage", || {
            let (status, bytes) = http(server_port, "GET", "/api/management/status", None)?;
            let value: Value = json_codec::from_slice(&bytes)?;
            if status == 200
                && value["authentication"] == "ready"
                && value["storage"]["state"] == "ready"
                && value["storage"]["recovering"] == false
            {
                Ok(())
            } else {
                Err(format!("fixture management not ready: HTTP {status}: {value}").into())
            }
        })?;
        let driver = env::var_os("CHROMEDRIVER").unwrap_or_else(|| "chromedriver".into());
        let _driver = spawn(
            Command::new(driver).args([
                format!("--port={driver_port}"),
                "--allowed-ips=127.0.0.1".into(),
            ]),
            &driver_log,
        )?;
        wait_for("loopback ChromeDriver", || {
            let (status, _) = http(driver_port, "GET", "/status", None)?;
            if status == 200 {
                Ok(())
            } else {
                Err("driver not ready".into())
            }
        })?;
        let window_size = if screenshot.is_some() {
            "--window-size=1440,1000"
        } else {
            "--window-size=1280,900"
        };
        let mut chrome = json!({"args":["--headless=new","--disable-dev-shm-usage",window_size]});
        if let Some(binary) = env::var_os("CHROME_BIN") {
            chrome["binary"] = binary.to_string_lossy().into_owned().into();
        }
        let (status, body) = http(
            driver_port,
            "POST",
            "/session",
            Some(
                json!({"capabilities":{"alwaysMatch":{"browserName":"chrome","pageLoadStrategy":"eager","goog:chromeOptions":chrome,"goog:loggingPrefs":{"browser":"ALL","performance":"ALL"}}}}),
            ),
        )?;
        let response: Value = json_codec::from_slice(&body)?;
        if status >= 400 {
            return Err(format!("Chrome startup failed (never skipped): {response}").into());
        }
        let browser = Browser {
            port: driver_port,
            session: response["value"]["sessionId"]
                .as_str()
                .ok_or("missing WebDriver session")?
                .to_owned(),
        };
        let result = exercise(&browser, &origin, &password, screenshot.as_deref())
            .and_then(|()| management::run(&browser, &cli_binary, &web, &pool, root, &password))
            .and_then(|()| management::setup(&browser, &cli_binary, &web, root, &password));
        if result.is_err() {
            let diagnostics_dir = PathBuf::from("artifacts/browser-smoke/diagnostics");
            diagnostics::failure(&browser, &diagnostics_dir, &password);
        }
        result
    })();
    if result.is_err() {
        for (name, path) in [("ZXS", server_log), ("ChromeDriver", driver_log)] {
            if let Ok(log) = fs::read_to_string(path) {
                eprintln!("--- {name} log ---\n{log}");
            }
        }
    }
    result
}
fn main() {
    if let Err(error) = run() {
        eprintln!("browser smoke FAILED: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
#[path = "../unit/main_tests.rs"]
mod tests;
