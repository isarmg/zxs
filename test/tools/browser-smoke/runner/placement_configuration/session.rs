//! A separate real HTTP actor for concurrency and rejection probes.
use super::*;

pub(super) struct Session {
    port: u16,
    token: String,
}
impl Session {
    pub(super) fn login(port: u16, password: &str) -> Result<Self> {
        let (status, bytes) = http(
            port,
            "POST",
            "/api/auth/login",
            Some(json!({"username":fixture::PRINCIPAL,"password":password})),
        )?;
        if status != 200 {
            return Err(format!("placement fixture login returned HTTP {status}").into());
        }
        let login: Value = json_codec::from_slice(&bytes)?;
        let token = login["token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("placement login omitted token")?
            .to_owned();
        Ok(Self { port, token })
    }
    pub(super) fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
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
        let (status, bytes) = read_http(&mut stream)?;
        Ok((status, json_codec::from_slice(&bytes)?))
    }
    pub(super) fn success(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let (status, value) = self.request(method, path, body)?;
        if status != 200 {
            return Err(
                format!("placement fixture {method} {path}: HTTP {status}: {value}").into(),
            );
        }
        Ok(value)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.request("POST", "/api/auth/logout", Some(json!({})));
    }
}
