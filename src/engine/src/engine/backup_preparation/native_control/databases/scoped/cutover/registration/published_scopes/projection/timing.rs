//! Test-feature-only per-thread timings, never admission or control state.
//! disk_finish includes disk_hash/disk_memo/disk_encode and actual readback;
//! archive includes authenticated replay and PreparedDiskScopes::observe.
use std::{cell::RefCell, io::Write, time::Instant};

#[derive(Default)]
struct Totals {
    captures: u64,
    lines: u8,
    nanos: [u128; 13],
    calls: [u64; 13],
}
thread_local! {
    static TOTALS: RefCell<Totals> = RefCell::new(Totals::default());
}

pub(super) struct Span {
    stage: usize,
    start: Instant,
}
impl Span {
    pub(super) fn new(stage: usize) -> Self {
        Self { stage, start: Instant::now() }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_nanos();
        let _ = TOTALS.try_with(|totals| {
            if let Ok(mut totals) = totals.try_borrow_mut() {
                totals.nanos[self.stage] = totals.nanos[self.stage].saturating_add(elapsed);
                totals.calls[self.stage] = totals.calls[self.stage].saturating_add(1);
            }
        });
    }
}

pub(super) struct Capture;
impl Capture {
    pub(super) fn new() -> Self { Self }
}
impl Drop for Capture {
    fn drop(&mut self) {
        // Counts include failed/unwound captures; they do not certify success.
        let record = TOTALS.try_with(|totals| {
            let mut totals = totals.try_borrow_mut().ok()?;
            totals.captures = totals.captures.saturating_add(1);
            if totals.lines == 32 || (totals.captures != 1 && totals.captures % 32 != 0) {
                return None;
            }
            totals.lines += 1;
            Some((totals.captures, totals.nanos, totals.calls))
        }).ok().flatten();
        if let Some((captures, nanos, calls)) = record {
            let us = nanos.map(|value| value / 1000);
            // All values are cumulative per thread. Nested buckets overlap.
            // No TLS borrow or encoding-memo mutex is held during output.
            let _ = writeln!(std::io::stderr().lock(),
                "[backup-scopes-us inclusive] thread={:?} capture_exits={} registry={} state={} archive={} extend={} system_input={} system_memo={} system_encode={} system_readback={} disk_finish={} disk_hash={} disk_memo={} disk_encode={} manifest_binding={} system_requests={} system_encoder_calls={} system_hits={} disk_requests={} disk_encoder_calls={} disk_hits={}",
                std::thread::current().id(), captures, us[0], us[1], us[2], us[3], us[4], us[5], us[6],
                us[7], us[8], us[9], us[10], us[11], us[12], calls[5], calls[6],
                calls[5].saturating_sub(calls[6]), calls[10], calls[11],
                calls[10].saturating_sub(calls[11]));
        }
    }
}
