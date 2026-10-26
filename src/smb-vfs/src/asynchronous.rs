//! Own buffers and descriptors in workers; only the host event loop touches
//! talloc, requests, file structures or client-provided output buffers.
use super::{error_errno, sdk};
use std::{
    ffi::c_void,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{Arc, Mutex, OnceLock, mpsc},
};

pub(super) const MAX_IO: usize = 8 * 1024 * 1024;

pub(super) struct Completion {
    pub result: isize,
    pub bytes: Vec<u8>,
}
impl Completion {
    pub fn done(result: isize) -> Self {
        Self {
            result,
            bytes: Vec::new(),
        }
    }
}
struct Outcome {
    error: i32,
    completion: Completion,
}
type Shared = Arc<Mutex<Option<Outcome>>>;
type Job = Box<dyn FnOnce() -> io::Result<Completion> + Send>;
struct Work {
    job: Job,
    shared: Shared,
    wake: OwnedFd,
}

fn pool() -> io::Result<&'static mpsc::SyncSender<Work>> {
    static POOL: OnceLock<Result<mpsc::SyncSender<Work>, Arc<io::Error>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Work>(8);
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..2 {
            let receiver = receiver.clone();
            let started = std::thread::Builder::new()
                .name(format!("smb-protocol-io-{index}"))
                .spawn(move || worker_loop(receiver));
            started.map_err(Arc::new)?;
        }
        Ok(sender)
    })
    .as_ref()
    .map_err(cached_start_error)
}

fn cached_start_error(error: &Arc<io::Error>) -> io::Error {
    match error.raw_os_error() {
        Some(errno) => io::Error::from_raw_os_error(errno),
        None => io::Error::new(error.kind(), error.clone()),
    }
}

fn worker_loop(receiver: Arc<Mutex<mpsc::Receiver<Work>>>) {
    loop {
        let work = match receiver
            .lock()
            .expect("SMB worker queue lock poisoned")
            .recv()
        {
            Ok(work) => work,
            Err(_) => return,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (work.job)().inspect_err(|error| eprintln!("zxs SMB async: {error}"))
        }))
        .unwrap_or_else(|_| Err(io::Error::from_raw_os_error(libc::EIO)));
        let outcome = match result {
            Ok(completion) => Outcome {
                error: 0,
                completion,
            },
            Err(error) => Outcome {
                error: error_errno(&error),
                completion: Completion::done(-1),
            },
        };
        *work.shared.lock().expect("SMB completion lock poisoned") = Some(outcome);
        let signal = 1_u64;
        loop {
            // SAFETY: wake owns a live eventfd duplicate; signal is a live
            // initialized u64 and write reads exactly its eight bytes.
            let result =
                unsafe { libc::write(work.wake.as_raw_fd(), (&signal as *const u64).cast(), 8) };
            if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            // One writer owns this eventfd duplicate and writes one value to a zero counter.
            assert_eq!(
                result,
                8,
                "invariant: owned eventfd signal failed: {}",
                io::Error::last_os_error()
            );
            break;
        }
    }
}

#[repr(C)]
struct State {
    extra: *mut Extra,
    result: isize,
    error: i32,
}
struct Extra {
    request: *mut sdk::tevent_req,
    read_fd: i32,
    shared: Shared,
    destination: *mut c_void,
    capacity: usize,
}
unsafe extern "C" fn destroy(pointer: *mut c_void) -> i32 {
    let state = pointer.cast::<State>();
    // SAFETY: talloc invokes this destructor with the live State allocated by send; it runs only on
    // the owning event loop.
    let extra = unsafe { (*state).extra };
    if !extra.is_null() {
        // SAFETY: Extra came from Box::into_raw in send. The destructor clears the slot before
        // reclaiming it exactly once; workers own only a separate Arc and descriptor.
        unsafe {
            (*state).extra = std::ptr::null_mut();
            drop(Box::from_raw(extra));
        }
    }
    0
}
unsafe extern "C" fn completed(
    _event: *mut sdk::tevent_context,
    fd: *mut sdk::tevent_fd,
    _flags: u16,
    pointer: *mut c_void,
) {
    let state = pointer.cast::<State>();
    // SAFETY: The watcher is a child of State and cannot survive request cancellation. send
    // installed the Extra box before registering this callback.
    let extra = unsafe { &*(*state).extra };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut signal = 0_u64;
        loop {
            // SAFETY: The live watcher owns read_fd; signal is writable eight-byte storage.
            let read = unsafe { libc::read(extra.read_fd, (&mut signal as *mut u64).cast(), 8) };
            if read < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            assert_eq!(
                read, 8,
                "invariant: readable eventfd has one complete signal"
            );
            break;
        }
        extra
            .shared
            .lock()
            .expect("SMB completion lock poisoned")
            .take()
            .expect("invariant: worker publishes outcome before signaling")
    }))
    .unwrap_or_else(|_| Outcome {
        error: libc::EIO,
        completion: Completion::done(-1),
    });
    let request = extra.request;
    let mut error = outcome.error;
    if !outcome.completion.bytes.is_empty() {
        if extra.destination.is_null() || outcome.completion.bytes.len() > extra.capacity {
            error = libc::EINVAL;
        } else {
            // SAFETY: send requires destination to remain exclusively writable until this callback;
            // bounds/null checks passed and the owned worker Vec cannot alias the host buffer.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    outcome.completion.bytes.as_ptr(),
                    extra.destination.cast::<u8>(),
                    outcome.completion.bytes.len(),
                );
            }
        }
    }
    // SAFETY: State and request remain live on this event loop. Free the watcher before marking
    // completion, since completion may synchronously free the request; access no state afterward.
    unsafe {
        (*state).result = if error == 0 {
            outcome.completion.result
        } else {
            -1
        };
        (*state).error = error;
        // Unregister before closing the watched descriptor, as tevent requires.
        sdk::_talloc_free(fd.cast(), c"zxs Rust VFS completion".as_ptr());
        sdk::_tevent_req_done(request, c"zxs Rust VFS completion".as_ptr());
    }
}

/// # Safety
/// mem_ctx is null or a live talloc context and event is a live tevent context.
/// Call on the owning event loop. If job returns bytes, destination must remain
/// exclusively writable for capacity bytes until completion, or until the host
/// cancels by freeing the request on this event loop before releasing the buffer.
/// Workers must not capture host pointers; they may return only owned data.
pub(super) unsafe fn send(
    mem_ctx: *mut sdk::TALLOC_CTX,
    event: *mut sdk::tevent_context,
    destination: *mut c_void,
    capacity: usize,
    job: impl FnOnce() -> io::Result<Completion> + Send + 'static,
) -> *mut sdk::tevent_req {
    let mut state: *mut State = std::ptr::null_mut();
    // SAFETY: The caller provides a valid talloc parent (or null); the output slot is writable and
    // the SDK allocates size_of::<State>() aligned state storage.
    let request = unsafe {
        sdk::__tevent_req_create(
            mem_ctx,
            (&mut state as *mut *mut State).cast(),
            std::mem::size_of::<State>(),
            c"zxs_async_state".as_ptr(),
            c"zxs_async_send".as_ptr(),
            c"zxs Rust VFS".as_ptr(),
        )
    };
    if request.is_null() {
        super::errno(libc::ENOMEM);
        return request;
    }
    let shared = Arc::new(Mutex::new(None));
    // SAFETY: Successful __tevent_req_create initialized the state allocation. Populate it before
    // registering its destructor; Extra ownership transfers to that destructor.
    unsafe {
        (*state).result = -1;
        (*state).error = libc::EINPROGRESS;
        (*state).extra = Box::into_raw(Box::new(Extra {
            request,
            read_fd: -1,
            shared: shared.clone(),
            destination,
            capacity,
        }));
        sdk::_talloc_set_destructor(state.cast(), Some(destroy));
    }
    let mut watcher: *mut sdk::tevent_fd = std::ptr::null_mut();
    let prepared = (|| -> io::Result<()> {
        // SAFETY: eventfd has no pointer arguments; the returned descriptor is checked before
        // ownership is adopted.
        let descriptor = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: eventfd succeeded and returned a fresh owned descriptor not yet managed by
        // another owner.
        let read = unsafe { OwnedFd::from_raw_fd(descriptor) };
        // SAFETY: read owns descriptor; fcntl duplicates it without consuming it and its return
        // value is checked.
        let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fcntl succeeded and the duplicate is a fresh descriptor adopted exactly once
        // here.
        let wake = unsafe { OwnedFd::from_raw_fd(duplicate) };
        // SAFETY: event and State are live host objects. Parenting the watcher to State cancels it
        // before that state can be reclaimed; private_data points to that State.
        watcher = unsafe {
            sdk::_tevent_add_fd(
                event,
                state.cast(),
                descriptor,
                sdk::TEVENT_FD_READ as u16,
                Some(completed),
                state.cast(),
                c"zxs_async_completed".as_ptr(),
                c"zxs Rust VFS".as_ptr(),
            )
        };
        if watcher.is_null() {
            return Err(io::Error::from_raw_os_error(libc::ENOMEM));
        }
        // SAFETY: State/Extra and watcher are live on the owning event loop. Auto-close transfers
        // descriptor cleanup to the watcher before read is forgotten.
        unsafe {
            (*(*state).extra).read_fd = descriptor;
            sdk::tevent_fd_set_auto_close(watcher);
        }
        std::mem::forget(read); // The tevent watcher now owns this descriptor.
        pool()?
            .try_send(Work {
                job: Box::new(job),
                shared,
                wake,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => io::Error::from_raw_os_error(libc::EAGAIN),
                mpsc::TrySendError::Disconnected(_) => io::Error::from_raw_os_error(libc::EIO),
            })?;
        Ok(())
    })();
    if let Err(error) = prepared {
        let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            eprintln!("zxs SMB async request setup failed: {error}");
            error_errno(&error)
        }))
        .unwrap_or(libc::EIO);
        // SAFETY: Only this event loop owns watcher/request. Free any registered watcher, set the
        // initialized state error, then post completion without further state access.
        unsafe {
            if !watcher.is_null() {
                sdk::_talloc_free(watcher.cast(), c"zxs Rust VFS queue failure".as_ptr());
            }
            (*state).error = error;
            sdk::_tevent_req_done(request, c"zxs Rust VFS queue failure".as_ptr());
            return sdk::tevent_req_post(request, event);
        }
    }
    request
}

/// # Safety
/// request must be a completed request returned by send, not previously received
/// or freed. aio must be live, exclusive writable SDK storage. Use the owning
/// event loop only; no references into request state may survive this call.
pub(super) unsafe fn recv(request: *mut sdk::tevent_req, aio: *mut sdk::vfs_aio_state) -> isize {
    if request.is_null() || aio.is_null() {
        super::errno(libc::EINVAL);
        return -1;
    }
    // SAFETY: The caller supplies the matching live request returned by send; its private data was
    // allocated as State.
    let state = unsafe { sdk::_tevent_req_data(request).cast::<State>() };
    // SAFETY: The matching completed request still owns its initialized State; copy scalar results
    // before tevent_req_received can free state.
    let (result, error) = unsafe { ((*state).result, (*state).error) };
    // SAFETY: aio is caller-provided writable storage. Request is live and completed; no state
    // references remain when received releases its data.
    unsafe {
        (*aio).error = error;
        (*aio).duration = 0;
        sdk::tevent_req_received(request);
    }
    if error != 0 {
        super::errno(error);
    }
    result
}

#[cfg(test)]
#[path = "../../../test/smb-vfs/unit/asynchronous/tests.rs"]
mod tests;
