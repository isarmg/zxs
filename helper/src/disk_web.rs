//! Explicit local disk panel. Pairing alone never authorizes a checker or repair.
mod check;
mod first;
pub(crate) use first::serve_repair;
mod http;
mod view;

use crate::{
    pinned::storage_status::{Selection, WebSelection},
    volume_images::{CheckFormat, Request as CopyCheck, panel::Operand},
};
use helper_linux_evidence::diagnostic_process::{cancelled, watch_cancellation};
use http::{Reply, Request};
use rustix::{
    process::{getgid, getuid},
    rand::{GetRandomFlags, getrandom},
};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Write},
    net::TcpListener,
    os::fd::AsFd,
    thread,
    time::{Duration, Instant},
};

const PAIR: Duration = Duration::from_secs(120);
const SESSION: Duration = Duration::from_secs(600);
const HEX: &[u8; 16] = b"0123456789abcdef";
fn token() -> io::Result<String> {
    let mut b = [0; 32];
    if getrandom(&mut b[..], GetRandomFlags::NONBLOCK)? != b.len() || b == [0; 32] {
        return Err(io::Error::other("secure panel credentials unavailable"));
    }
    let mut s = String::with_capacity(64);
    for c in b {
        s.push(HEX[usize::from(c >> 4)] as char);
        s.push(HEX[usize::from(c & 15)] as char);
    }
    Ok(s)
}
fn digest(s: &[u8]) -> [u8; 32] {
    Sha256::digest(s).into()
}
fn matches(s: &[u8], d: &[u8; 32]) -> bool {
    s.len() == 64
        && s.iter()
            .all(|&c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && digest(s) == *d
}
struct Session {
    bearer: [u8; 32],
    csrf: [u8; 32],
}
struct Panel {
    host: String,
    origin: String,
    pairing: Option<[u8; 32]>,
    failed: u8,
    session: Option<Session>,
    expires: Instant,
    closed: bool,
    check: Option<check::Check>,
    handoff: Option<first::Offer>,
    storage: Option<WebSelection>,
}
impl Panel {
    fn new(host: String, code: &[u8], now: Instant) -> Self {
        Self {
            origin: format!("http://{host}"),
            host,
            pairing: Some(digest(code)),
            failed: 0,
            session: None,
            expires: now + PAIR,
            closed: false,
            check: None,
            handoff: None,
            storage: None,
        }
    }
    fn handle(&mut self, r: Request, now: Instant) -> Reply {
        if self.closed || now >= self.expires {
            return Reply::plain(403, "会话已结束，请重新在终端启动。");
        }
        if r.host != self.host {
            return Reply::plain(403, "需要本机准确的回环地址和端口。");
        }
        if r.method == "GET" && r.body.is_empty() {
            return self.asset(&r.path);
        }
        if r.method != "POST" {
            return Reply::plain(405, "不支持此请求。");
        }
        if r.origin.as_deref() != Some(self.origin.as_str()) {
            return Reply::plain(403, "来源不匹配。");
        }
        if r.path == "/api/pair" {
            return self.pair(&r, now);
        }
        let Some(ref session) = self.session else {
            return Reply::plain(401, "请先在本机终端配对。");
        };
        if !r
            .auth
            .as_deref()
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|v| matches(v.as_bytes(), &session.bearer))
            || !r
                .csrf
                .as_deref()
                .is_some_and(|v| matches(v.as_bytes(), &session.csrf))
        {
            return Reply::plain(403, "会话凭据无效。");
        }
        if let Some(offer) = &mut self.handoff {
            let (response, close) = offer.route(r, now);
            self.closed = close;
            return response;
        }
        if matches!(r.path.as_str(), "/api/copy/check" | "/api/copy/result") {
            if r.content_type.as_deref() != Some("application/octet-stream") || r.body.len() != 64 {
                return Reply::plain(400, "只接受本会话的固定长度预览票。");
            }
            return match self.check.as_mut() {
                Some(c) => c.action(r.path == "/api/copy/check", &r.body, now),
                None => Reply::plain(403, "终端未为本次面板授权任何镜像检查。"),
            };
        }
        if !r.body.is_empty() || r.content_type.is_some() {
            return Reply::plain(400, "不接受路径、设备号、选项或请求正文。");
        }
        match r.path.as_str() {
            "/api/copy/preview" => match self.check.as_mut() {
                Some(c) => c.preview(now),
                None => Reply::plain(403, "本次终端只开放观察，没有预选卷镜像。"),
            },
            "/api/disks" => view::disks(false),
            "/api/rates" => view::disks(true),
            "/api/tools" => view::tools(),
            "/api/hardware" => view::hardware(&crate::hardware::observe()),
            "/api/storage" => match &self.storage {
                Some(roots) => view::storage(&roots.observe()),
                None => Reply::plain(
                    403,
                    "本次终端没有预选挂载目录；请另从本机终端以 disk-web --storage-roots 启动。",
                ),
            },
            "/api/close" => {
                self.closed = true;
                Reply::plain(
                    200,
                    "不再接受操作；终端可能仍在等工作进程收尾。此回复不能作为升级许可。",
                )
            }
            _ => Reply::plain(404, "没有该操作。"),
        }
    }
    fn asset(&self, path: &str) -> Reply {
        match path {
            "/" if self.handoff.as_ref().is_some_and(first::Offer::is_repair) => Reply::asset(
                "text/html; charset=utf-8",
                include_str!("disk_web/repair.html"),
            ),
            "/" if self.handoff.is_some() => Reply::asset(
                "text/html; charset=utf-8",
                include_str!("disk_web/first.html"),
            ),
            "/panel.js" if self.handoff.is_some() => Reply::asset(
                "text/javascript; charset=utf-8",
                include_str!("disk_web/first.js"),
            ),
            "/" => Reply::asset(
                "text/html; charset=utf-8",
                include_str!("disk_web/index.html"),
            ),
            "/panel.js" => Reply::asset(
                "text/javascript; charset=utf-8",
                include_str!("disk_web/panel.js"),
            ),
            "/panel.css" => Reply::asset(
                "text/css; charset=utf-8",
                include_str!("disk_web/panel.css"),
            ),
            _ => Reply::plain(404, "未提供此页面。"),
        }
    }

    fn pair(&mut self, r: &Request, now: Instant) -> Reply {
        let Some(expect) = self.pairing.as_ref() else {
            return Reply::plain(403, "配对已结束。");
        };
        if r.content_type.as_deref() != Some("application/octet-stream")
            || r.auth.is_some()
            || r.csrf.is_some()
        {
            return Reply::plain(400, "不支持的配对请求。");
        }
        if !matches(&r.body, expect) {
            self.failed += 1;
            if self.failed >= 6 {
                self.closed = true;
                return Reply::plain(429, "配对尝试已用完，请重新启动。");
            }
            return Reply::plain(403, "配对失败。");
        }
        let (Ok(bearer), Ok(csrf)) = (token(), token()) else {
            self.closed = true;
            return Reply::plain(503, "无法建立会话。");
        };
        if bearer == csrf || matches(bearer.as_bytes(), expect) || matches(csrf.as_bytes(), expect)
        {
            self.closed = true;
            return Reply::plain(503, "无法建立独立凭据。");
        }
        self.pairing = None;
        self.expires = now + SESSION;
        self.session = Some(Session {
            bearer: digest(bearer.as_bytes()),
            csrf: digest(csrf.as_bytes()),
        });
        Reply::plain(200, format!("{bearer}\n{csrf}"))
    }
}

// Verify the already-held descriptor, not a mutable pathname or a generic
// isatty. Writing to a PTY master would feed input to its slave.
fn supported_terminal(mode: u32, dev: u64, magic: u64) -> bool {
    let (major, minor) = (rustix::fs::major(dev), rustix::fs::minor(dev));
    mode & 0o170_000 == 0o020_000
        && ((major, minor) == (5, 0)
            || (major == 4 && (1..=63).contains(&minor))
            || ((136..=143).contains(&major) && magic == 0x1cd1)) // Linux devpts slave
}
fn foreground(tty_sid: i32, our_sid: i32, tty_group: i32, our_group: i32) -> bool {
    tty_sid > 0 && tty_group > 0 && tty_sid == our_sid && tty_group == our_group
}
pub(crate) fn require_terminal(fd: impl AsFd) -> io::Result<()> {
    use rustix::{
        fs::{fstat, fstatfs},
        process::{getpgrp, getsid},
        termios::{tcgetpgrp, tcgetsid},
    };
    let deny = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "disk-web requires stderr on this operator's supported foreground controlling terminal",
        )
    };
    let stat = fstat(&fd)?;
    if !supported_terminal(
        stat.st_mode,
        stat.st_rdev,
        fstatfs(&fd)?.f_type.cast_unsigned(),
    ) {
        return Err(deny());
    }
    if !foreground(
        tcgetsid(&fd)?.as_raw_pid(),
        getsid(None)?.as_raw_pid(),
        tcgetpgrp(&fd)?.as_raw_pid(),
        getpgrp().as_raw_pid(),
    ) {
        return Err(deny());
    }
    Ok(())
}

pub(crate) fn request_headless_review(
    unit: Option<String>,
    check: impl Fn() -> io::Result<()>,
) -> io::Result<bool> {
    check()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let now = Instant::now();
    let mut raw = zeroize::Zeroizing::new([0; 32]);
    if getrandom(&mut raw[..], GetRandomFlags::NONBLOCK)? != raw.len() || *raw == [0; 32] {
        return Err(io::Error::other("setup pairing code unavailable"));
    }
    {
        let mut terminal = io::stderr().lock();
        writeln!(
            terminal,
            "首次设置向导：http://{}/\n一次性配对码（2 分钟内使用）：",
            listener.local_addr()?
        )?;
        for byte in raw.iter() {
            write!(terminal, "{byte:02x}")?;
        }
        writeln!(terminal)?;
        terminal.flush()?;
    }
    first::serve_headless(listener, raw, now + Duration::from_secs(600), unit, check)
}

enum Launch {
    Ordinary(Option<CopyCheck>),
    Storage(Selection),
}
pub(crate) fn serve(image: Option<CopyCheck>) -> io::Result<()> {
    serve_mode(Launch::Ordinary(image))
}
pub(crate) fn serve_storage(selection: Selection) -> io::Result<()> {
    serve_mode(Launch::Storage(selection))
}

fn serve_mode(launch: Launch) -> io::Result<()> {
    // This command owns a fresh single-threaded process. No privileged Web
    // worker, inherited admin groups, ambient capabilities or fallback listener.
    rustix::thread::set_no_new_privs(true)?;
    crate::maintenance::diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
    let stderr = io::stderr();
    let mut tty = stderr.lock();
    require_terminal(stderr.as_fd())?;
    let (image, storage) = match launch {
        Launch::Ordinary(image) => (image, None),
        Launch::Storage(selection) => (None, Some(WebSelection::pin(&selection))),
    };
    let leased = image
        .as_ref()
        .is_some_and(|job| matches!(job.format(), CheckFormat::Leased(_)));
    let selected = if let Some(job) = image {
        if leased {
            writeln!(
                tty,
                "仅固定终端选择的卷镜像原件。浏览器确认后才取只读租约、核对 SHA-256 并运行检查器。"
            )?;
        } else {
            writeln!(
                tty,
                "正在读取终端指定的镜像并核对完整 SHA-256。仅保存封存副本；浏览器确认前不会运行检查器。"
            )?;
        }
        tty.flush()?;
        Some(check::Check::new(Operand::capture(job)?))
    } else {
        None
    };
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let host = listener.local_addr()?.to_string();
    let code = token()?;
    let mut panel = Panel::new(host, code.as_bytes(), Instant::now());
    panel.check = selected;
    panel.storage = storage;
    require_terminal(stderr.as_fd())?;
    writeln!(
        tty,
        "磁盘工具（本机只读）：{}\n一次性配对码（2 分钟有效，请勿分享）：{}",
        panel.origin, code
    )?;
    tty.flush()?;
    drop(code);
    drop(tty);
    // No worker exists while the terminal writes its prompts. Once requests
    // can run, let a foreground cancel close HTTP and reap this worker.
    if leased {
        watch_cancellation()?;
    }
    // Each connection has its own total read and write budgets.
    let transport = (|| -> io::Result<()> {
        let mut served = 0usize;
        while !panel.closed && !cancelled() && Instant::now() < panel.expires && served < 512 {
            if let Some(c) = &mut panel.check {
                c.poll();
            }
            match listener.accept() {
                Ok((mut socket, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    served += 1;
                    socket.set_nonblocking(false)?;
                    let request = http::read(&mut socket);
                    if cancelled() {
                        break;
                    }
                    let reply = match request {
                        Ok(r) => panel.handle(r, Instant::now()),
                        Err(_) => Reply::plain(400, "请求无效或未在时限内完成。"),
                    };
                    let _ = reply.send(&mut socket); // a disconnected browser does not extend the session
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(40));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })();
    drop(listener); // Closing HTTP must not wait on the original worker's I/O or reap.
    drop(panel.storage.take());
    let disposition = panel.check.take().map_or(Ok(()), check::Check::finish);
    disposition.and(transport)
}

#[cfg(test)]
#[path = "../tests/disk_web/session.rs"]
mod tests;
