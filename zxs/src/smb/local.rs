//! Common Linux entropy and UTC FILETIME for locally generated protocol fields.
//! An I/O error never falls back to a GUID, counter, clock or weaker random source.
use std::time::Duration;

pub(super) fn random<const N:usize>(dst: &mut [u8;N]) -> bool {
    // Existing ZXS Linux getrandom(0) wrapper; handles EINTR and short fills.
    // No pathname, host crypto library, architecture-specific syscall number
    // or weaker fallback. Every caller supplies its own fresh salt or challenge buffer.
    unsafe extern "C" { fn zxs_random(out: *mut u8, length: usize) -> std::os::raw::c_int; }
    // SAFETY: dst is live/writable for the exact synchronous fill.
    (unsafe { zxs_random(dst.as_mut_ptr(),dst.len()) }) == 0
}

pub(super) fn filetime_delta(delta: std::result::Result<Duration,Duration>) -> Option<u64> {
    const UNIX_TICKS: u128 = 116_444_736_000_000_000;
    let ticks = match delta {
        Ok(after) => UNIX_TICKS.checked_add(after.as_nanos()/100)?,
        // Floor on the absolute timeline; an instant 1 ns before the Unix
        // epoch is one FILETIME tick earlier, not the epoch itself.
        Err(before) => UNIX_TICKS.checked_sub(before.as_nanos().div_ceil(100))?,
    };
    ticks.try_into().ok()
}
