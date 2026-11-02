use crate::output::adapter::StripBytes;
use crate::output::stream::AsLockedWrite;
use crate::output::stream::IsTerminal;

/// Only pass printable data to the inner `Write`
#[derive(Debug)]
pub struct StripStream<S>
where
    S: std::io::Write,
{
    raw: S,
    state: StripBytes,
}

impl<S> StripStream<S>
where
    S: std::io::Write,
{
    /// Only pass printable data to the inner `Write`
    #[inline]
    pub fn new(raw: S) -> Self {
        Self {
            raw,
            state: Default::default(),
        }
    }

    /// Get the wrapped [`std::io::Write`]
    #[inline]
    pub fn into_inner(self) -> S {
        self.raw
    }

    /// Get the wrapped [`std::io::Write`]
    #[inline]
    pub fn as_inner(&self) -> &S {
        &self.raw
    }
}

impl<S> StripStream<S>
where
    S: std::io::Write,
    S: IsTerminal,
{
    /// Returns `true` if the descriptor/handle refers to a terminal/tty.
    #[inline]
    pub fn is_terminal(&self) -> bool {
        self.raw.is_terminal()
    }
}

impl StripStream<std::io::Stdout> {
    /// Get exclusive access to the `StripStream`
    ///
    /// Why?
    /// - Faster performance when writing in a loop
    /// - Avoid other threads interleaving output with the current thread
    #[inline]
    pub fn lock(self) -> StripStream<std::io::StdoutLock<'static>> {
        StripStream {
            raw: self.raw.lock(),
            state: self.state,
        }
    }
}

impl StripStream<std::io::Stderr> {
    /// Get exclusive access to the `StripStream`
    ///
    /// Why?
    /// - Faster performance when writing in a loop
    /// - Avoid other threads interleaving output with the current thread
    #[inline]
    pub fn lock(self) -> StripStream<std::io::StderrLock<'static>> {
        StripStream {
            raw: self.raw.lock(),
            state: self.state,
        }
    }
}

impl<S> std::io::Write for StripStream<S>
where
    S: std::io::Write,
    S: AsLockedWrite,
{
    // Must forward all calls to ensure locking happens appropriately
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        write(&mut self.raw.as_locked_write(), &mut self.state, buf)
    }
    #[inline]
    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        let buf = bufs
            .iter()
            .find(|b| !b.is_empty())
            .map(|b| &**b)
            .unwrap_or(&[][..]);
        self.write(buf)
    }
    // is_write_vectored: nightly only
    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        self.raw.as_locked_write().flush()
    }
    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        write_all(&mut self.raw.as_locked_write(), &mut self.state, buf)
    }
    // write_all_vectored: nightly only
    #[inline]
    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
        write_fmt(&mut self.raw.as_locked_write(), &mut self.state, args)
    }
}

fn write(
    raw: &mut dyn std::io::Write,
    state: &mut StripBytes,
    buf: &[u8],
) -> std::io::Result<usize> {
    let initial_state = state.clone();

    for printable in state.strip_next(buf) {
        let possible = printable.len();
        let written = raw.write(printable)?;
        if possible != written {
            let divergence = &printable[written..];
            let offset = offset_to(buf, divergence);
            let consumed = &buf[..offset];
            *state = initial_state;
            state.strip_next(consumed).last();
            return Ok(offset);
        }
    }
    Ok(buf.len())
}

fn write_all(
    raw: &mut dyn std::io::Write,
    state: &mut StripBytes,
    buf: &[u8],
) -> std::io::Result<()> {
    for printable in state.strip_next(buf) {
        raw.write_all(printable)?;
    }
    Ok(())
}

fn write_fmt(
    raw: &mut dyn std::io::Write,
    state: &mut StripBytes,
    args: std::fmt::Arguments<'_>,
) -> std::io::Result<()> {
    let write_all = |buf: &[u8]| write_all(raw, state, buf);
    crate::output::fmt::Adapter::new(write_all).write_fmt(args)
}

#[inline]
fn offset_to(total: &[u8], subslice: &[u8]) -> usize {
    let total = total.as_ptr();
    let subslice = subslice.as_ptr();

    debug_assert!(
        total <= subslice,
        "`Offset::offset_to` only accepts slices of `self`"
    );
    subslice as usize - total as usize
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/terminal-style/anstream/unit/src/strip/test_172.rs"
));
