//! Real sole-account UI and HTTP contracts against the disposable smoke daemon.
//! HTTP rejection probes use a separate bearer session, never browser API mocks.
use super::*;

struct Session {
    port: u16,
    token: String,
}
impl Session {
    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<(u16, Vec<u8>)> {
        let mut stream = TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", self.port).parse()?,
            Duration::from_secs(2),
        )?;
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let body = body.map(|value| value.to_string()).unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            self.token,
            body.len()
        )?;
        read_http(&mut stream)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.request("POST", "/api/auth/logout", Some(json!({})));
    }
}

pub(super) fn run(browser: &Browser, password: &str) -> Result<()> {
    browser.click(&browser.button("Administration", "系统管理")?)?;
    if browser.script(
        r#"
        const section = document.querySelector('section[aria-label="Daily administration"], section[aria-label="日常管理"]');
        if (!section || section.querySelector('input[type=password]')) return false;
        const text = section.textContent;
        return !['刷新账户', 'Refresh accounts', '账户 Accounts', '访问权限 ACL',
            'admin=', 'disabled=', 'repositories=', '仓库授权'].some(label => text.includes(label));
    "#,
    )? != true
    {
        return Err("retired account or ACL management remains in the real page".into());
    }
    browser.click(&browser.button("Account settings", "账号设置")?)?;
    browser.css(".zxs-account-form input[name='current_password']")?;
    if browser.script("const input=document.querySelector('.zxs-account-form input[name=username]'); return input?.readOnly && input.value==='smoke-admin'")? != true {
        return Err("account settings did not preserve the sole read-only identity".into());
    }
    let port = browser
        .script("if(location.hostname!=='127.0.0.1'||location.protocol!=='http:')throw new Error('not the disposable local daemon');return location.port")?
        .as_str()
        .ok_or("missing local daemon port")?
        .parse()?;
    let (status, bytes) = http(
        port,
        "POST",
        "/api/auth/login",
        Some(json!({"username":"smoke-admin","password":password})),
    )?;
    let login: Value = json_codec::from_slice(&bytes)?;
    if status != 200
        || !exact_fields(&login, &["token", "csrf_token", "principal", "expires_at"])
        || login["principal"] != "smoke-admin"
        || !login["expires_at"]
            .as_str()
            .is_some_and(|value| value.parse::<u64>().is_ok_and(|value| value > 0))
    {
        return Err("sole-account login returned an incomplete or retired response shape".into());
    }
    let token = login["token"]
        .as_str()
        .filter(|token| {
            token.len() == 43
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or("invalid bearer credential shape")?
        .to_owned();
    let session = Session { port, token };
    let (status, bytes) = session.request("GET", "/api/auth/me", None)?;
    let identity: Value = json_codec::from_slice(&bytes)?;
    if status != 200
        || !exact_fields(&identity, &["principal", "csrf_token"])
        || identity["principal"] != "smoke-admin"
        || !identity["csrf_token"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    {
        return Err("sole-account session restoration returned a retired role contract".into());
    }
    for method in ["GET", "POST"] {
        let body = (method == "POST")
            .then(|| json!({"username":"retired-extra-account","password":password}));
        if session.request(method, "/api/admin/accounts", body)?.0 != 404 {
            return Err(format!("retired {method} account-management route still exists").into());
        }
    }
    let (status, bytes) = session.request("GET", "/api/repositories", None)?;
    let repositories: Value = json_codec::from_slice(&bytes)?;
    let repositories = repositories["repositories"]
        .as_array()
        .ok_or("missing repository listing")?;
    let repository = repositories.first().ok_or("smoke repository missing")?;
    let id = repository["repository_id"]
        .as_str()
        .ok_or("missing repository identity")?;
    if status != 200
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("invalid smoke repository response".into());
    }
    if repository.get("grants").is_some() || repository.get("account_scope_repository_id").is_some()
    {
        return Err("repository still exposes a multi-account permission model".into());
    }
    if session.request("PUT", &format!("/api/repositories/{id}/grants"), Some(json!({"grants":{"read":[],"write":[],"manage":[]},"operation_id":storage_engine::new_id("op")})))?.0 != 404 {
        return Err("retired repository ACL route still exists".into());
    }
    let (status, _) = http(
        port,
        "POST",
        "/api/auth/login",
        Some(json!({"username":"not-the-sole-account","password":password})),
    )?;
    if status != 401 {
        return Err("a different account name was not rejected".into());
    }
    println!(
        "PASS sole-account UI/session shapes and removed account/ACL routes; unknown account rejected"
    );
    Ok(())
}

fn exact_fields(value: &Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == expected.len() && expected.iter().all(|field| object.contains_key(*field))
    })
}
