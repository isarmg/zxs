//! One explicit HTTP listener with bounded workers and foreground drain.
use crate::{
    container_store::Store,
    headless_http::{IO_TIMEOUT, Incoming, Outcome, ReuseBudget, serve_connection},
    headless_start::Bound,
    workspace_protocol::Request,
};
use std::{io, net::TcpListener, thread, time::Duration};

const WORKERS: usize = 32;
const POLL: Duration = Duration::from_millis(20);

pub(crate) fn serve_selected(
    bound: &Bound,
    listener: TcpListener,
    stopping: impl Fn() -> bool + Sync,
    dispatch: impl Fn(&Store, Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome + Sync,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let budget = &ReuseBudget::default();
    thread::scope(|scope| {
        let mut workers: Vec<thread::ScopedJoinHandle<'_, ()>> = Vec::new();
        let mut result = loop {
            let mut next = 0;
            let mut panicked = false;
            while next < workers.len() {
                if workers[next].is_finished() {
                    panicked |= workers.swap_remove(next).join().is_err();
                } else {
                    next += 1;
                }
            }
            if panicked {
                break Err(io::Error::other("headless worker stopped unexpectedly"));
            }
            if stopping() {
                break Ok(());
            }
            if workers.len() >= WORKERS {
                thread::sleep(POLL);
                continue;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if stopping() {
                        break Ok(());
                    }
                    if stream.set_nonblocking(false).is_err()
                        || stream.set_write_timeout(Some(IO_TIMEOUT)).is_err()
                    {
                        continue;
                    }
                    let dispatch = &dispatch;
                    let stopping = &stopping;
                    match thread::Builder::new().spawn_scoped(scope, move || {
                        let _ = serve_connection(
                            bound.account(),
                            &stream,
                            stopping,
                            budget,
                            |scope| bound.store().upload_lane(scope),
                            |request, body, live| dispatch(bound.store(), request, body, live),
                        );
                    }) {
                        Ok(worker) => workers.push(worker),
                        Err(error) => break Err(error),
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => break Err(error),
            }
        };
        // Stop the listener first. Already admitted requests may finish their
        // durable work and return their exact receipts on a graceful stop.
        drop(listener);
        if result.is_err() {
            bound.account().stop_admission();
        }
        for worker in workers {
            if worker.join().is_err() && result.is_ok() {
                result = Err(io::Error::other("headless worker stopped during drain"));
            }
        }
        bound.account().stop_admission();
        result
    })
}
