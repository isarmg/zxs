//! Credential-specific host admission for the legacy worker flow.
use super::bad;
pub(super) use crate::local_worker::Worker;
use crate::maintenance::thread_file;
use std::io::{self, Read};

fn current_status() -> io::Result<String> {
    let mut s = String::new();
    thread_file(c"thread-self/status")?
        .take(65537)
        .read_to_string(&mut s)?;
    if s.len() > 65536 || !s.ends_with('\n') {
        return Err(bad("invalid current credential process status"));
    }
    Ok(s)
}
pub(super) fn prerequisite() -> io::Result<()> {
    let s = current_status()?;
    let mut e = s.lines().filter_map(|l| l.strip_prefix("CapEff:"));
    let cap = e
        .next()
        .and_then(|v| u64::from_str_radix(v.trim_ascii(), 16).ok());
    // DAC override/read plus root process-control rights; inadequate signal
    // authority cannot start a worker it would be unable to reap on failure.
    if e.next().is_some() || !cap.is_some_and(|v| v & (1 << 5) != 0 && v & 6 != 0) {
        return Err(bad(
            "the site cannot verify or collect this original credential process",
        ));
    }
    Ok(())
}
