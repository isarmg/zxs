use crate::Result;
use json_codec::{Value, json};
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
    time::{Duration, Instant},
};
const LIMIT: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn management(listen: SocketAddr) -> Result<Value> {
    let address = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen.port())
        }
        _ => listen,
    };
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT)?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    write!(
        stream,
        "GET /api/management/status HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )?;
    let mut bytes = Vec::new();
    let mut block = [0_u8; 2048];
    loop {
        let remaining = TIMEOUT
            .checked_sub(started.elapsed())
            .ok_or("management readiness probe timed out")?;
        if remaining.is_zero() {
            return Err("management readiness probe timed out".into());
        }
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut block)?;
        if count == 0 {
            break;
        }
        if count > LIMIT - bytes.len() {
            return Err("management readiness reply exceeded 64 KiB".into());
        }
        bytes.extend_from_slice(&block[..count]);
    }
    let end = bytes
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .ok_or("management readiness response has no HTTP header")?;
    let header = std::str::from_utf8(&bytes[..end])?;
    let mut lines = header.split("\r\n");
    if !matches!(lines.next(), Some("HTTP/1.1 200 OK" | "HTTP/1.0 200 OK")) {
        return Err("management readiness endpoint did not return HTTP 200".into());
    }
    let mut length = None;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or("invalid management HTTP header")?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("unexpected encoded management readiness response".into());
        }
        if name.eq_ignore_ascii_case("content-length")
            && length.replace(value.trim().parse::<usize>()?).is_some()
        {
            return Err("duplicate management content length".into());
        }
    }
    let body = &bytes[end + 4..];
    if length != Some(body.len()) {
        return Err("management readiness response was incomplete".into());
    }
    let value: Value = json_codec::from_slice(body)?;
    if value["management"] != "alive" {
        return Err("endpoint is not the ZXS management observer".into());
    }
    Ok(value)
}
pub(super) fn ready(status: &Value) -> bool {
    status["authentication"] == "ready"
        && status["storage"]["state"] == "ready"
        && status["auxiliary"]["web_assets"] == "ready"
}
pub(super) fn bridge_socket(service_uid: u32) -> Result<Value> {
    let path = Path::new("/zxs/run/smb.sock");
    let before = std::fs::symlink_metadata(path)?;
    if !before.file_type().is_socket()
        || before.uid() != service_uid
        || before.mode() & 0o7777 != 0o600
    {
        return Err(
            "ZXS bridge socket has not been safely published by the selected service account"
                .into(),
        );
    }
    let after = std::fs::symlink_metadata(path)?;
    if (before.dev(), before.ino(), before.uid(), before.mode())
        != (after.dev(), after.ino(), after.uid(), after.mode())
    {
        return Err("ZXS bridge socket changed during observation".into());
    }
    Ok(
        json!({"socket_published":true, "device":before.dev().to_string(),
        "inode":before.ino().to_string(), "bridge_protocol_session_verified":false,
        "network_password_login_verified":false,"file_write_test_performed":false}),
    )
}
