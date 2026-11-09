//! Lightweight mouse observation. Every completed external left click is only a
//! candidate; the controller checks actual selection and manages its session.
//! No UI Automation, clipboard read, or HTTP work happens inside the hook.

use std::{
    cell::RefCell,
    mem::zeroed,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::diagnostics::{self, Counter, Outcome, Stage};

use windows_sys::Win32::{
    Foundation::{GetLastError, HINSTANCE, LPARAM, LRESULT, POINT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{GetCurrentProcessId, GetCurrentThreadId},
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetDoubleClickTime, VK_LBUTTON, VK_RBUTTON, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetAncestor, GetCursorPos, GetMessageW,
            GetSystemMetrics, GetWindowThreadProcessId, KillTimer, PeekMessageW, PostMessageW,
            PostThreadMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint,
            GA_ROOTOWNER, HHOOK, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, SM_CXDOUBLECLK, SM_CYDOUBLECLK,
            WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_TIMER,
        },
    },
};

static ENABLED: AtomicBool = AtomicBool::new(true);
static ENABLED_EPOCH: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);
const WATCHDOG_INTERVAL_MS: u32 = 250;
const WATCHDOG_GAP: Duration = Duration::from_millis(500);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);

struct HookContext {
    target: isize,
    message: u32,
    press: Option<Press>,
    last_click: Option<Click>,
}

struct Press {
    point: (i32, i32),
    time: u32,
    shift: bool,
    moved: bool,
}

struct Click {
    point: (i32, i32),
    time: u32,
}

struct ButtonEvent {
    own_window: bool,
    point: (i32, i32),
    time: u32,
    shift: bool,
}

#[derive(Clone, Copy)]
struct ClickSettings {
    max_delay: u32,
    half_width: i64,
    half_height: i64,
}

#[derive(Debug, PartialEq, Eq)]
enum Notice {
    Cancel((i32, i32)),
    Candidate { point: (i32, i32), strong: bool },
}

impl HookContext {
    fn observe(&mut self, down: bool, event: ButtonEvent, clicks: ClickSettings) -> Option<Notice> {
        if down {
            if event.own_window {
                self.press = None;
                self.last_click = None;
                return None;
            }
            self.press = Some(Press {
                point: event.point,
                time: event.time,
                shift: event.shift,
                moved: false,
            });
            return Some(Notice::Cancel(event.point));
        }
        // Only the press origin decides eligibility. An external drag can end
        // over an existing translation popup without losing the new selection.
        let press = self.press.take()?;
        let moved = press.moved || press.point != event.point;
        let multiple_click = self.last_click.as_ref().is_some_and(|last| {
            press.time.wrapping_sub(last.time) <= clicks.max_delay
                && (i64::from(press.point.0) - i64::from(last.point.0)).abs() <= clicks.half_width
                && (i64::from(press.point.1) - i64::from(last.point.1)).abs() <= clicks.half_height
        });
        self.last_click = (!moved).then_some(Click {
            point: press.point,
            time: press.time,
        });
        // This flag is a hint for immediate selection feedback, never a filter:
        // ordinary clicks still reach the selection provider for validation.
        Some(Notice::Candidate {
            point: event.point,
            strong: moved || press.shift || event.shift || multiple_click,
        })
    }

    fn observe_move(&mut self, point: (i32, i32)) {
        if let Some(press) = self.press.as_mut() {
            press.moved |= press.point != point;
        }
    }
}

impl Notice {
    fn parameters(&self) -> (WPARAM, LPARAM) {
        match *self {
            Self::Cancel((x, y)) => (
                (u64::from(x as u32) | (u64::from(y as u32) << 32)) as usize,
                isize::MIN,
            ),
            Self::Candidate {
                point: (x, y),
                strong,
            } => (
                (u64::from(x as u32) | (u64::from(strong) << 32)) as usize,
                y as isize,
            ),
        }
    }
}

/// Decode the press origin carried by a cancel notification on the x64 target.
pub fn press_point(wparam: usize) -> POINT {
    POINT {
        x: wparam as u32 as i32,
        y: ((wparam as u64) >> 32) as u32 as i32,
    }
}

/// Whether a release involved a drag, Shift, or a system-timed multiple click.
pub fn is_selection_gesture(wparam: usize) -> bool {
    ((wparam as u64) >> 32) & 1 != 0
}

/// Independently polled cursor position and physical button safety gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MouseState {
    point: (i32, i32),
    button_down: bool,
}

struct Observation {
    point: (i32, i32),
    mismatch_since: Option<Instant>,
}

impl Observation {
    fn record(&mut self, point: (i32, i32)) {
        self.point = point;
        self.mismatch_since = None;
    }

    fn needs_rebind(&mut self, actual: MouseState, now: Instant) -> bool {
        if actual.point == self.point {
            self.mismatch_since = None;
            return false;
        }
        let since = *self.mismatch_since.get_or_insert(now);
        // Never cut an ongoing drag in two, and never replay a gesture whose
        // origin was lost. Recovery only enables the next physical gesture.
        !actual.button_down && now.saturating_duration_since(since) >= WATCHDOG_GAP
    }
}

struct WorkerContext {
    hook: HookContext,
    control: Arc<Control>,
    epoch: u64,
    observed: Observation,
}

impl WorkerContext {
    fn synchronize_enabled(&mut self, epoch: u64) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.hook.press = None;
            self.hook.last_click = None;
        }
    }
}

#[derive(Default)]
struct Control {
    closed: AtomicBool,
    thread_id: AtomicU32,
    restart_requested: AtomicBool,
    #[cfg(test)]
    timer_ticks: AtomicU32,
    #[cfg(test)]
    rebinds: AtomicU32,
}

thread_local! {
    static CONTEXT: RefCell<Option<WorkerContext>> = const { RefCell::new(None) };
}

/// The hook and its message pump belong to a dedicated thread. UI rendering,
/// dialogs and clipboard operations cannot consume LowLevelHooksTimeout.
pub struct MouseHook {
    control: Arc<Control>,
    finished: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
}

pub fn install(target_window: isize, message: u32) -> Result<MouseHook, String> {
    if target_window == 0 {
        diagnostics::record(Stage::HookInstall, Outcome::Ineligible);
        return Err("无法安装划词监听：缺少接收窗口。".into());
    }
    if INSTALLED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        diagnostics::record(Stage::HookInstall, Outcome::Busy);
        return Err("已经安装划词监听。".into());
    }
    let control = Arc::new(Control::default());
    let worker_control = Arc::clone(&control);
    let (started, startup) = mpsc::sync_channel(1);
    let (completed, finished) = mpsc::channel();
    let worker = match thread::Builder::new()
        .name("mouse-hook".into())
        .spawn(move || {
            struct Installation(Arc<Control>);
            impl Drop for Installation {
                fn drop(&mut self) {
                    self.0.thread_id.store(0, Ordering::Release);
                    INSTALLED.store(false, Ordering::Release);
                }
            }
            let installed = Installation(Arc::clone(&worker_control));
            run_hook(target_window, message, worker_control, started);
            drop(installed);
            let _ = completed.send(());
        }) {
        Ok(worker) => worker,
        Err(error) => {
            INSTALLED.store(false, Ordering::Release);
            if let Some(code) = error
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok())
            {
                diagnostics::record_error(Stage::HookInstall, Outcome::Failed, code);
            } else {
                diagnostics::record(Stage::HookInstall, Outcome::Failed);
            }
            return Err("无法启动划词监听线程。".into());
        }
    };
    let guard = MouseHook {
        control,
        finished,
        worker: Some(worker),
    };
    match startup.recv_timeout(INSTALL_TIMEOUT) {
        Ok(Ok(())) => Ok(guard),
        Ok(Err(error)) => Err(error),
        Err(error) => {
            diagnostics::record(
                Stage::HookInstall,
                match error {
                    mpsc::RecvTimeoutError::Timeout => Outcome::TimedOut,
                    mpsc::RecvTimeoutError::Disconnected => Outcome::WorkerStopped,
                },
            );
            Err("划词监听启动超时，请重新启动应用。".into())
        }
    }
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
    // The UI no longer owns the callback's thread-local context. An epoch also
    // covers pause -> resume between two callbacks, so an old down cannot join
    // a later up and accidentally read an unrelated selection.
    ENABLED_EPOCH.fetch_add(1, Ordering::AcqRel);
}

impl MouseHook {
    /// Ask the existing worker to rebind once both physical buttons are up.
    /// This never replaces a running thread or replays an interrupted gesture.
    pub fn request_restart(&self) -> bool {
        if self.control.closed.load(Ordering::Acquire)
            || self.control.thread_id.load(Ordering::Acquire) == 0
            || self.worker.as_ref().is_none_or(JoinHandle::is_finished)
        {
            diagnostics::record(Stage::HookHealth, Outcome::WorkerStopped);
            return false;
        }
        self.control
            .restart_requested
            .store(true, Ordering::Release);
        diagnostics::record(Stage::HookHealth, Outcome::Retrying);
        true
    }
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        self.control.closed.store(true, Ordering::Release);
        let thread_id = self.control.thread_id.load(Ordering::Acquire);
        if thread_id != 0 {
            unsafe {
                PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
            }
        }
        let _ = self.finished.recv_timeout(SHUTDOWN_TIMEOUT);
        if let Some(worker) = self.worker.take() {
            // Joining only an already-finished worker prevents a stuck OS hook
            // call from hanging the UI. A detached worker retains all its own
            // context/handles until that call returns and cleanup can finish.
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

struct HookHandle(HHOOK);
impl HookHandle {
    fn new(stage: Stage) -> Result<Self, String> {
        diagnostics::increment(Counter::HookInstallAttempt);
        diagnostics::record(stage, Outcome::Started);
        let module: HINSTANCE = unsafe { GetModuleHandleW(ptr::null()) };
        let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_callback), module, 0) };
        if hook.is_null() {
            let error = unsafe { GetLastError() };
            diagnostics::record_error(stage, Outcome::Failed, error);
            Err(format!("无法安装划词监听（系统错误 {}）。", error))
        } else {
            diagnostics::increment(Counter::HookInstallSuccess);
            diagnostics::record(stage, Outcome::Succeeded);
            Ok(Self(hook))
        }
    }
}
impl Drop for HookHandle {
    fn drop(&mut self) {
        unsafe {
            UnhookWindowsHookEx(self.0);
        }
    }
}

unsafe fn current_mouse() -> Option<MouseState> {
    let mut point = zeroed();
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return None;
    }
    Some(MouseState {
        point: (point.x, point.y),
        // Async state is physical. Requiring both buttons up is correct for
        // either primary-button mapping and avoids rebinding during any drag.
        button_down: unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON)) } < 0
            || unsafe { GetAsyncKeyState(i32::from(VK_RBUTTON)) } < 0,
    })
}

fn run_hook(
    target: isize,
    message: u32,
    control: Arc<Control>,
    startup: mpsc::SyncSender<Result<(), String>>,
) {
    unsafe {
        let mut event: MSG = zeroed();
        // Force queue creation before publishing the ID used for shutdown.
        PeekMessageW(&mut event, ptr::null_mut(), 0, 0, PM_NOREMOVE);
        control
            .thread_id
            .store(GetCurrentThreadId(), Ordering::Release);
        if control.closed.load(Ordering::Acquire) {
            return;
        }
        let initial = current_mouse().unwrap_or(MouseState {
            point: (0, 0),
            button_down: false,
        });
        CONTEXT.with(|slot| {
            *slot.borrow_mut() = Some(WorkerContext {
                hook: HookContext {
                    target,
                    message,
                    press: None,
                    last_click: None,
                },
                control: Arc::clone(&control),
                epoch: ENABLED_EPOCH.load(Ordering::Acquire),
                observed: Observation {
                    point: initial.point,
                    mismatch_since: None,
                },
            })
        });
        let mut hook = match HookHandle::new(Stage::HookInstall) {
            Ok(hook) => Some(hook),
            Err(error) => {
                CONTEXT.with(|slot| *slot.borrow_mut() = None);
                let _ = startup.send(Err(error));
                return;
            }
        };
        let timer = SetTimer(ptr::null_mut(), 0, WATCHDOG_INTERVAL_MS, None);
        if timer == 0 {
            let error = GetLastError();
            diagnostics::record_error(Stage::HookHealth, Outcome::Unavailable, error);
            CONTEXT.with(|slot| *slot.borrow_mut() = None);
            let _ = startup.send(Err("无法启动划词监听状态检查。".into()));
            return;
        }
        if startup.send(Ok(())).is_ok() {
            let mut next_install = Instant::now();
            while !control.closed.load(Ordering::Acquire)
                && GetMessageW(&mut event, ptr::null_mut(), 0, 0) > 0
            {
                if event.message == WM_TIMER && event.hwnd.is_null() && event.wParam == timer {
                    #[cfg(test)]
                    control.timer_ticks.fetch_add(1, Ordering::Release);
                    let now = Instant::now();
                    if let Some(actual) = current_mouse() {
                        let rebind = CONTEXT.with(|slot| {
                            let mut slot = slot.borrow_mut();
                            let Some(context) = slot.as_mut() else {
                                return false;
                            };
                            context.synchronize_enabled(ENABLED_EPOCH.load(Ordering::Acquire));
                            context.observed.needs_rebind(actual, now)
                        });
                        if !actual.button_down
                            && now >= next_install
                            && (hook.is_none()
                                || rebind
                                || control.restart_requested.load(Ordering::Acquire))
                        {
                            // Windows exposes no hook-liveness query. Rebind
                            // after sustained independent input mismatch or an
                            // explicit request, never merely due to idle input.
                            control.restart_requested.store(false, Ordering::Release);
                            diagnostics::increment(Counter::HookRebind);
                            #[cfg(test)]
                            control.rebinds.fetch_add(1, Ordering::Release);
                            drop(hook.take());
                            CONTEXT.with(|slot| {
                                if let Some(context) = slot.borrow_mut().as_mut() {
                                    context.hook.press = None;
                                    context.hook.last_click = None;
                                    context.observed.record(actual.point);
                                }
                            });
                            hook = HookHandle::new(Stage::HookHealth).ok();
                            next_install = now + Duration::from_secs(1);
                        }
                    }
                } else {
                    DispatchMessageW(&event);
                }
            }
        }
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
        KillTimer(ptr::null_mut(), timer);
        drop(hook);
        diagnostics::record(Stage::HookHealth, Outcome::WorkerStopped);
    }
}

unsafe extern "system" fn mouse_callback(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code < 0
        || !matches!(message as u32, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE)
        || data == 0
    {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) };
    }
    diagnostics::increment(match message as u32 {
        WM_MOUSEMOVE => Counter::HookMove,
        WM_LBUTTONDOWN => Counter::HookDown,
        WM_LBUTTONUP => Counter::HookUp,
        _ => unreachable!("unobserved hook message filtered above"),
    });
    let mouse = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
    let point = (mouse.pt.x, mouse.pt.y);
    let enabled = ENABLED.load(Ordering::Acquire);
    let epoch = ENABLED_EPOCH.load(Ordering::Acquire);
    let usable = CONTEXT.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return false;
        };
        let Some(context) = slot.as_mut() else {
            return false;
        };
        context.synchronize_enabled(epoch);
        context.observed.record(point);
        if !enabled || context.control.closed.load(Ordering::Acquire) {
            return false;
        }
        if message as u32 == WM_MOUSEMOVE {
            context.hook.observe_move(point);
            return false;
        }
        true
    });
    if !usable {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) };
    }
    let window = unsafe { WindowFromPoint(mouse.pt) };
    let owner = unsafe { GetAncestor(window, GA_ROOTOWNER) };
    let own_pid = unsafe { GetCurrentProcessId() };
    let mut window_pid = 0;
    let mut owner_pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, &mut window_pid);
        GetWindowThreadProcessId(owner, &mut owner_pid);
    }
    let own_window = window_pid == own_pid || owner_pid == own_pid;
    if message as u32 == WM_LBUTTONDOWN && !own_window {
        diagnostics::increment(Counter::ExternalDown);
    }
    let shift = unsafe { GetAsyncKeyState(i32::from(VK_SHIFT)) } < 0;
    let clicks = ClickSettings {
        max_delay: unsafe { GetDoubleClickTime() },
        half_width: i64::from(unsafe { GetSystemMetrics(SM_CXDOUBLECLK) }.max(1)) / 2,
        half_height: i64::from(unsafe { GetSystemMetrics(SM_CYDOUBLECLK) }.max(1)) / 2,
    };
    let notification = CONTEXT.with(|context| {
        let Ok(mut context) = context.try_borrow_mut() else {
            return None;
        };
        let context = context.as_mut()?;
        context
            .hook
            .observe(
                message as u32 == WM_LBUTTONDOWN,
                ButtonEvent {
                    own_window,
                    point,
                    time: mouse.time,
                    shift,
                },
                clicks,
            )
            .map(|notice| (context.hook.target, context.hook.message, notice))
    });
    if let Some((target, notification, notice)) = notification {
        let (x, y) = notice.parameters();
        let posted = unsafe { PostMessageW(target as _, notification, x, y) };
        diagnostics::increment(if posted == 0 {
            Counter::NotifyFailed
        } else {
            Counter::NotifyPosted
        });
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
}

#[cfg(test)]
mod tests {
    use super::{
        is_selection_gesture, press_point, ButtonEvent, ClickSettings, HookContext, Notice,
    };

    const CLICKS: ClickSettings = ClickSettings {
        max_delay: 500,
        half_width: 2,
        half_height: 2,
    };

    fn context() -> HookContext {
        HookContext {
            target: 1,
            message: 2,
            press: None,
            last_click: None,
        }
    }

    fn event(own_window: bool, point: (i32, i32), time: u32, shift: bool) -> ButtonEvent {
        ButtonEvent {
            own_window,
            point,
            time,
            shift,
        }
    }

    fn click(hook: &mut HookContext, point: (i32, i32), time: u32, shift: bool) -> Option<Notice> {
        assert_eq!(
            hook.observe(true, event(false, point, time, shift), CLICKS),
            Some(Notice::Cancel(point))
        );
        hook.observe(false, event(false, point, time + 20, shift), CLICKS)
    }

    #[test]
    fn every_external_gesture_has_its_own_candidate_even_at_the_same_position() {
        let mut hook = context();
        // Text identity belongs to the controller. The hook must never suppress
        // a later selection session just because its coordinates are unchanged.
        for time in [0, 1000, 2000] {
            assert_eq!(
                click(&mut hook, (20, 30), time, false),
                Some(Notice::Candidate {
                    point: (20, 30),
                    strong: false,
                })
            );
        }
        assert_eq!(
            hook.observe(false, event(false, (20, 30), 3000, false), CLICKS),
            None
        );
    }

    #[test]
    fn a_one_pixel_drag_is_a_strong_candidate() {
        let mut hook = context();
        hook.observe(true, event(false, (20, 30), 0, false), CLICKS);
        assert_eq!(
            hook.observe(false, event(false, (21, 30), 20, false), CLICKS),
            Some(Notice::Candidate {
                point: (21, 30),
                strong: true,
            })
        );
    }

    #[test]
    fn a_drag_returning_to_its_press_point_is_still_strong() {
        let mut hook = context();
        hook.observe(true, event(false, (-20, 30), 0, false), CLICKS);
        hook.observe_move((-20, 31));
        hook.observe_move((-20, 30));
        assert_eq!(
            hook.observe(false, event(false, (-20, 30), 20, false), CLICKS),
            Some(Notice::Candidate {
                point: (-20, 30),
                strong: true,
            })
        );
    }

    #[test]
    fn shift_at_either_end_of_a_click_marks_a_selection_gesture() {
        for (down_shift, up_shift) in [(true, false), (false, true), (true, true)] {
            let mut hook = context();
            hook.observe(true, event(false, (20, 30), 0, down_shift), CLICKS);
            assert_eq!(
                hook.observe(false, event(false, (20, 30), 20, up_shift), CLICKS),
                Some(Notice::Candidate {
                    point: (20, 30),
                    strong: true,
                })
            );
        }
    }

    #[test]
    fn double_and_triple_clicks_respect_system_time_and_position_limits() {
        let mut hook = context();
        for (point, time, strong) in [
            ((20, 30), 0, false),
            ((21, 31), 100, true),
            ((20, 30), 200, true),
            ((20, 30), 1000, false),
            ((23, 30), 1100, false),
        ] {
            assert_eq!(
                click(&mut hook, point, time, false),
                Some(Notice::Candidate { point, strong })
            );
        }
    }

    #[test]
    fn multiple_click_timing_handles_the_windows_tick_counter_wrapping() {
        let mut hook = context();
        click(&mut hook, (20, 30), u32::MAX - 100, false);
        assert_eq!(
            click(&mut hook, (20, 30), 50, false),
            Some(Notice::Candidate {
                point: (20, 30),
                strong: true,
            })
        );
    }

    #[test]
    fn external_drag_ending_over_the_app_keeps_its_candidate() {
        let mut hook = context();
        hook.observe(true, event(false, (20, 30), 0, false), CLICKS);
        assert_eq!(
            hook.observe(false, event(true, (50, 60), 20, false), CLICKS),
            Some(Notice::Candidate {
                point: (50, 60),
                strong: true,
            })
        );
    }

    #[test]
    fn app_presses_and_drags_out_of_the_app_never_create_candidates() {
        let mut hook = context();
        click(&mut hook, (20, 30), 0, false);
        assert_eq!(
            hook.observe(true, event(true, (20, 30), 100, false), CLICKS),
            None
        );
        hook.observe_move((50, 60));
        assert_eq!(
            hook.observe(false, event(false, (50, 60), 120, false), CLICKS),
            None
        );
        // Interacting with the app also breaks the source's click sequence.
        assert_eq!(
            click(&mut hook, (20, 30), 200, false),
            Some(Notice::Candidate {
                point: (20, 30),
                strong: false,
            })
        );
    }

    fn mouse(point: (i32, i32), button_down: bool) -> super::MouseState {
        super::MouseState { point, button_down }
    }

    #[test]
    fn watchdog_does_not_rebind_a_quiet_or_normally_reporting_mouse() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let initial = mouse((20, 30), false);
        let mut observed = super::Observation {
            point: initial.point,
            mismatch_since: None,
        };
        assert!(!observed.needs_rebind(initial, start + Duration::from_secs(3600)));
        let moved = mouse((40, 60), false);
        assert!(!observed.needs_rebind(moved, start));
        observed.record(moved.point);
        assert!(!observed.needs_rebind(moved, start + Duration::from_secs(10)));
    }

    #[test]
    fn watchdog_requires_sustained_independent_mismatch_and_a_released_button() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let mut observed = super::Observation {
            point: (20, 30),
            mismatch_since: None,
        };
        let dragging = mouse((40, 60), true);
        assert!(!observed.needs_rebind(dragging, start));
        assert!(!observed.needs_rebind(dragging, start + Duration::from_secs(10)));
        // Rebinding cannot finish or replay the lost gesture. It only repairs
        // the listener, after release, for a later physical selection.
        assert!(observed.needs_rebind(mouse((40, 60), false), start + Duration::from_secs(10)));
        observed.record((40, 60));
        let next = mouse((50, 70), false);
        assert!(!observed.needs_rebind(next, start + Duration::from_secs(11)));
        assert!(!observed.needs_rebind(next, start + Duration::from_millis(11_499)));
        assert!(observed.needs_rebind(next, start + Duration::from_millis(11_500)));
    }

    #[test]
    fn a_new_callback_or_restored_mouse_state_cancels_watchdog_suspicion() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let initial = mouse((20, 30), false);
        let moved = mouse((40, 60), false);
        let mut observed = super::Observation {
            point: initial.point,
            mismatch_since: None,
        };
        assert!(!observed.needs_rebind(moved, start));
        assert!(!observed.needs_rebind(initial, start + Duration::from_secs(1)));
        assert!(!observed.needs_rebind(moved, start + Duration::from_secs(2)));
        observed.record(moved.point);
        assert!(!observed.needs_rebind(moved, start + Duration::from_secs(3)));
    }

    #[test]
    fn enable_epoch_prevents_a_press_from_crossing_pause_and_resume() {
        let mut worker = super::WorkerContext {
            hook: context(),
            control: std::sync::Arc::new(super::Control::default()),
            epoch: 1,
            observed: super::Observation {
                point: (20, 30),
                mismatch_since: None,
            },
        };
        worker
            .hook
            .observe(true, event(false, (20, 30), 0, false), CLICKS);
        worker.synchronize_enabled(3); // disable/enable can both happen before the next callback
        assert_eq!(
            worker
                .hook
                .observe(false, event(false, (50, 60), 10, false), CLICKS),
            None
        );
        assert_eq!(
            click(&mut worker.hook, (50, 60), 100, false),
            Some(Notice::Candidate {
                point: (50, 60),
                strong: false
            })
        );
    }

    #[test]
    fn dedicated_hook_pumps_without_the_installers_message_loop_and_releases_ownership() {
        use std::{
            sync::atomic::Ordering,
            thread,
            time::{Duration, Instant},
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, HWND_MESSAGE,
        };
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!window.is_null());
        let guard = super::install(window as isize, 0x8001).unwrap();
        let control = std::sync::Arc::clone(&guard.control);
        assert_ne!(control.thread_id.load(Ordering::Acquire), unsafe {
            super::GetCurrentThreadId()
        });
        let deadline = Instant::now() + Duration::from_secs(4);
        while control.timer_ticks.load(Ordering::Acquire) < 6 {
            // This thread never pumps messages. A UI-thread hook/poll would
            // be starved here; the independent installed worker must progress.
            assert!(
                Instant::now() < deadline,
                "The dedicated hook message pump stalled"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let worker_thread = control.thread_id.load(Ordering::Acquire);
        let previous_rebinds = control.rebinds.load(Ordering::Acquire);
        assert!(guard.request_restart());
        let deadline = Instant::now() + Duration::from_secs(3);
        while control.rebinds.load(Ordering::Acquire) == previous_rebinds
            || control.restart_requested.load(Ordering::Acquire)
        {
            assert!(
                Instant::now() < deadline,
                "The existing hook worker did not handle its restart request"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(control.thread_id.load(Ordering::Acquire), worker_thread);
        drop(guard);
        assert!(control.closed.load(Ordering::Acquire));
        let deadline = Instant::now() + Duration::from_secs(2);
        while super::INSTALLED.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "The hook worker did not release installation ownership"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(control.thread_id.load(Ordering::Acquire), 0);
        unsafe {
            DestroyWindow(window);
        }
    }

    #[test]
    fn notification_coordinates_preserve_sign_and_do_not_set_false_gesture_bits() {
        for (x, y) in [(0, 0), (-1920, -1080), (i32::MIN, i32::MAX)] {
            let (packed, marker) = Notice::Cancel((x, y)).parameters();
            assert_eq!(marker, isize::MIN);
            let point = press_point(packed);
            assert_eq!((point.x, point.y), (x, y));
            for strong in [false, true] {
                let (packed, packed_y) = Notice::Candidate {
                    point: (x, y),
                    strong,
                }
                .parameters();
                assert_eq!(packed as u32 as i32, x);
                assert_eq!(packed_y, y as isize);
                assert_eq!(is_selection_gesture(packed), strong);
            }
        }
    }
}
