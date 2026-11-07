//! WinHTTP owns in-flight buffers until HANDLE_CLOSING. Keep them together with
//! the callback context; cancellation only closes the request on its worker.
use super::{network_error, network_error_code, remaining_ms, set_timeouts, HttpHandle};
use crate::translation::check_cancel;
use std::{
    collections::VecDeque,
    ffi::c_void,
    mem::size_of,
    ptr::{null_mut, read_unaligned},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_IO_PENDING},
    Networking::WinHttp::*,
};

const CANCEL_INTERVAL: Duration = Duration::from_millis(20);
const BUFFER_SIZE: usize = 8192;
// Closing a request can return before its final callback. Bound retained buffers
// even if a platform delays cancellation or a native handle close fails.
const MAX_LIVE_REQUESTS: usize = 16;

#[derive(Default)]
pub(super) struct Slots(AtomicUsize);

#[cfg(test)]
impl Slots {
    pub(super) fn live(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

struct Permit(Arc<Slots>);

impl Permit {
    fn acquire(slots: Arc<Slots>) -> Result<Self, String> {
        let mut count = slots.0.load(Ordering::Acquire);
        loop {
            if count >= MAX_LIVE_REQUESTS {
                return Err("先前的翻译连接仍在关闭，请稍后重试。".into());
            }
            match slots.0.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => count = actual,
            }
        }
        Ok(Self(slots))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Debug)]
enum Completion {
    Sent,
    Headers,
    Available(u32),
    Read(u32),
    Failed(u32),
}

#[derive(Default)]
struct State {
    completions: VecDeque<Completion>,
    closed: bool,
}

struct Context {
    registration_owned: AtomicBool,
    _permit: Permit,
    state: Mutex<State>,
    changed: Condvar,
    // The context is referenced by the request owner and by every running
    // callback, so these buffers survive even the final closing callback.
    headers: Vec<u16>,
    body: Vec<u8>,
    read_buffer: Mutex<Box<[u8; BUFFER_SIZE]>>,
}

pub(super) struct Request {
    handle: Option<HttpHandle>,
    context: Arc<Context>,
}

impl Request {
    pub(super) fn new(
        handle: HttpHandle,
        headers: Vec<u16>,
        body: Vec<u8>,
        slots: Arc<Slots>,
    ) -> Result<Self, String> {
        let permit = Permit::acquire(slots)?;
        let context = Arc::new(Context {
            registration_owned: AtomicBool::new(true),
            _permit: permit,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            headers,
            body,
            read_buffer: Mutex::new(Box::new([0; BUFFER_SIZE])),
        });
        // A dedicated callback registration reference outlives Request itself.
        // It is released exactly once by HANDLE_CLOSING, not by worker Drop.
        let pointer = Arc::into_raw(Arc::clone(&context));
        let context_value = pointer as usize;
        // Register context before enabling notifications. No asynchronous I/O
        // starts until the fully initialized Request has been returned.
        if unsafe {
            WinHttpSetOption(
                handle.0,
                WINHTTP_OPTION_CONTEXT_VALUE,
                (&context_value as *const usize).cast(),
                size_of::<usize>() as u32,
            )
        } == 0
        {
            let error = network_error();
            // There is no callback and no I/O on this construction error path.
            release_registration(&context, pointer);
            return Err(error);
        }
        let previous = unsafe {
            WinHttpSetStatusCallback(
                handle.0,
                Some(callback),
                WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE
                    | WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE
                    | WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE
                    | WINHTTP_CALLBACK_FLAG_READ_COMPLETE
                    | WINHTTP_CALLBACK_FLAG_REQUEST_ERROR
                    | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
                0,
            )
        };
        // WinHTTP uses the sentinel function pointer -1 for registration error;
        // None is the normal previous callback of a newly opened request.
        if previous.is_some_and(|function| function as usize == usize::MAX) {
            let error = network_error();
            // Registration failed before any operation was started.
            release_registration(&context, pointer);
            return Err(error);
        }
        Ok(Self {
            handle: Some(handle),
            context,
        })
    }

    pub(super) fn raw(&self) -> *mut c_void {
        self.handle.as_ref().expect("request is open").0
    }

    fn prepare<C: Fn() -> bool>(&self, deadline: Instant, cancel: &C) -> Result<(), String> {
        check_cancel(cancel)?;
        set_timeouts(
            self.handle.as_ref().expect("request is open"),
            remaining_ms(deadline)?,
        )
    }

    pub(super) fn send<C: Fn() -> bool>(
        &self,
        deadline: Instant,
        cancel: &C,
    ) -> Result<(), String> {
        self.prepare(deadline, cancel)?;
        let length =
            u32::try_from(self.context.body.len()).map_err(|_| "请求内容过长。".to_owned())?;
        started(unsafe {
            WinHttpSendRequest(
                self.raw(),
                self.context.headers.as_ptr(),
                (self.context.headers.len() - 1) as u32,
                self.context.body.as_ptr().cast(),
                length,
                length,
                Arc::as_ptr(&self.context) as usize,
            )
        })?;
        match self.wait(deadline, cancel)? {
            Completion::Sent => Ok(()),
            _ => Err(unexpected_completion()),
        }
    }

    pub(super) fn receive<C: Fn() -> bool>(
        &self,
        deadline: Instant,
        cancel: &C,
    ) -> Result<(), String> {
        self.prepare(deadline, cancel)?;
        started(unsafe { WinHttpReceiveResponse(self.raw(), null_mut()) })?;
        match self.wait(deadline, cancel)? {
            Completion::Headers => Ok(()),
            _ => Err(unexpected_completion()),
        }
    }

    pub(super) fn available<C: Fn() -> bool>(
        &self,
        deadline: Instant,
        cancel: &C,
    ) -> Result<u32, String> {
        self.prepare(deadline, cancel)?;
        // Output must be NULL in asynchronous mode. The callback copies the
        // available count rather than retaining WinHTTP's temporary pointer.
        started(unsafe { WinHttpQueryDataAvailable(self.raw(), null_mut()) })?;
        match self.wait(deadline, cancel)? {
            Completion::Available(bytes) => Ok(bytes),
            _ => Err(unexpected_completion()),
        }
    }

    pub(super) fn read<C: Fn() -> bool>(
        &self,
        available: u32,
        deadline: Instant,
        cancel: &C,
        consume: impl FnOnce(&[u8]) -> Result<(), String>,
    ) -> Result<usize, String> {
        self.prepare(deadline, cancel)?;
        let mut buffer = self
            .context
            .read_buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Only this worker starts reads, and it does not examine the buffer
        // until READ_COMPLETE. On cancellation the context still owns the Box
        // until the final HANDLE_CLOSING callback releases the registration owner.
        started(unsafe {
            WinHttpReadData(
                self.raw(),
                buffer.as_mut_ptr().cast(),
                available.min(BUFFER_SIZE as u32),
                null_mut(),
            )
        })?;
        let count = match self.wait(deadline, cancel)? {
            Completion::Read(bytes) if bytes as usize <= BUFFER_SIZE => bytes as usize,
            _ => return Err(unexpected_completion()),
        };
        consume(&buffer[..count])?;
        Ok(count)
    }

    fn wait<C: Fn() -> bool>(&self, deadline: Instant, cancel: &C) -> Result<Completion, String> {
        loop {
            check_cancel(cancel)?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| "翻译请求已超时，请稍后重试。".to_owned())?;
            let mut state = self
                .context
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(completion) = state.completions.pop_front() {
                drop(state);
                check_cancel(cancel)?;
                return match completion {
                    Completion::Failed(code) => Err(network_error_code(code)),
                    completion => Ok(completion),
                };
            }
            if state.closed {
                return Err(unexpected_completion());
            }
            let _waited = self
                .context
                .changed
                .wait_timeout(state, remaining.min(CANCEL_INTERVAL))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // The worker closes its own request. This cancels pending WinHTTP
            // operations without closing a handle from a watchdog thread.
            drop(handle);
            // Do not wait here for HANDLE_CLOSING: the native asynchronous
            // operation may defer it. The dedicated registration Arc retains
            // all buffers until that final callback, independently of this
            // worker, so the latest selection can start immediately.
            // If native CloseHandle fails, retaining that reference is safer
            // than freeing possibly reachable memory; the per-client slot cap
            // bounds such retained contexts instead of allowing unbounded growth.
        }
    }
}

fn started(result: i32) -> Result<(), String> {
    if result != 0 {
        return Ok(());
    }
    // Capture immediately; an async API can report IO_PENDING instead of TRUE.
    let error = unsafe { GetLastError() };
    if error == ERROR_IO_PENDING {
        Ok(())
    } else {
        Err(network_error_code(error))
    }
}

fn unexpected_completion() -> String {
    "翻译服务连接状态异常，请重试。".into()
}

unsafe extern "system" fn callback(
    _handle: *mut c_void,
    pointer: usize,
    status: u32,
    information: *mut c_void,
    length: u32,
) {
    if pointer == 0 {
        return;
    }
    let pointer = pointer as *const Context;
    // SAFETY: The dedicated registration Arc remains allocated until the
    // single final HANDLE_CLOSING notification. Each currently executing callback
    // also takes a reference before it can wake the worker or release that owner.
    Arc::increment_strong_count(pointer);
    let context = Arc::from_raw(pointer);
    let completion = match status {
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => Some(Completion::Sent),
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => Some(Completion::Headers),
        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE
            if !information.is_null() && length as usize >= size_of::<u32>() =>
        {
            Some(Completion::Available(read_unaligned(information.cast())))
        }
        WINHTTP_CALLBACK_STATUS_READ_COMPLETE => Some(Completion::Read(length)),
        WINHTTP_CALLBACK_STATUS_REQUEST_ERROR
            if !information.is_null() && length as usize >= size_of::<WINHTTP_ASYNC_RESULT>() =>
        {
            let result = read_unaligned(information.cast::<WINHTTP_ASYNC_RESULT>());
            Some(Completion::Failed(result.dwError))
        }
        _ => None,
    };
    let mut state = context
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if status == WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING {
        state.closed = true;
    } else if let Some(completion) = completion {
        state.completions.push_back(completion);
    }
    drop(state);
    context.changed.notify_all();
    if status == WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING {
        release_registration(&context, pointer);
    }
}

fn release_registration(context: &Context, pointer: *const Context) {
    if context.registration_owned.swap(false, Ordering::AcqRel) {
        // SAFETY: This is exactly the extra reference created with into_raw.
        // The caller's separate Arc keeps Context and its buffers alive until
        // callback return (or until a failed constructor has fully unwound).
        unsafe {
            drop(Arc::from_raw(pointer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> (HttpHandle, HttpHandle) {
        let agent = super::super::wide("LingoLeaf local handle-lifetime test");
        let host = super::super::wide("127.0.0.1");
        let session = HttpHandle::from_raw(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_ASYNC,
            )
        })
        .unwrap();
        // WinHttpConnect/OpenRequest do not connect to the service. These tests
        // exercise actual native callback registration/closing without traffic.
        let connection =
            HttpHandle::from_raw(unsafe { WinHttpConnect(session.0, host.as_ptr(), 9, 0) })
                .unwrap();
        (session, connection)
    }

    fn request(connection: &HttpHandle, slots: Arc<Slots>) -> Result<Request, String> {
        let method = super::super::wide("POST");
        let path = super::super::wide("/");
        let handle = HttpHandle::from_raw(unsafe {
            WinHttpOpenRequest(
                connection.0,
                method.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        })?;
        Request::new(handle, vec![0], Vec::new(), slots)
    }

    fn wait_for_count(slots: &Slots, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while slots.live() != expected && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(slots.live(), expected);
    }

    #[test]
    fn requests_closed_before_send_release_callback_owners_and_reuse_all_slots() {
        let (_session, connection) = connection();
        let slots = Arc::new(Slots::default());
        let mut requests = Vec::new();
        for _ in 0..MAX_LIVE_REQUESTS {
            requests.push(request(&connection, slots.clone()).unwrap());
        }
        assert_eq!(slots.live(), MAX_LIVE_REQUESTS);
        assert!(request(&connection, slots.clone()).is_err());
        assert_eq!(slots.live(), MAX_LIVE_REQUESTS);
        drop(requests.pop());
        wait_for_count(&slots, MAX_LIVE_REQUESTS - 1);
        requests.push(request(&connection, slots.clone()).unwrap());
        drop(requests);
        wait_for_count(&slots, 0);
        // A second full batch proves the cap is a live-resource bound rather
        // than a lifetime request counter that would disable a long-lived app.
        for _ in 0..MAX_LIVE_REQUESTS * 2 {
            drop(request(&connection, slots.clone()).unwrap());
            wait_for_count(&slots, 0);
        }
    }

    #[test]
    fn failed_native_context_registration_returns_the_slot_without_callbacks() {
        let slots = Arc::new(Slots::default());
        let invalid = HttpHandle(null_mut());
        assert!(Request::new(invalid, vec![0], Vec::new(), slots.clone()).is_err());
        assert_eq!(slots.live(), 0);
    }
}
