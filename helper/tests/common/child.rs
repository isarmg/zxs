//! Only for a disposable unit-test child, not an isolated production group.
use std::{
    io,
    process::{Child, Command, ExitStatus},
};

pub(crate) struct FixtureChild(Child);

impl FixtureChild {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        command.spawn().map(Self)
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }
}

impl Drop for FixtureChild {
    fn drop(&mut self) {
        // try_wait either reaps the original child, returns its cached exit,
        // or proves this same unreaped child is still the one to stop.
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
