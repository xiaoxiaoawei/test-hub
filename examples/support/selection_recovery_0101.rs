// Recovery regressions use actual mouse/keyboard input against our own EDIT.
// The delayed source publishes its native selection on its own UI timer; the
// runner never sends the application's selection-controller messages.
use super::*;
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, AtomicUsize};
use windows_sys::Win32::UI::{
    Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
    Input::KeyboardAndMouse::{MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK},
    Shell::{DefSubclassProc, SetWindowSubclass},
};

const SOURCE_DELAY: u32 = WM_APP + 51;
const SOURCE_TIMER: usize = 101;
const SUBCLASS: usize = 101;
#[derive(Clone, Copy, Default)]
struct Deferred {
    delay: u32,
    start: u32,
    end: u32,
}
thread_local! { static DEFERRED: Cell<Deferred> = Cell::new(Deferred::default()); }
static COLLAPSED: AtomicBool = AtomicBool::new(false);
static PUBLISHED: AtomicBool = AtomicBool::new(false);
static WATCH_INLINE: AtomicUsize = AtomicUsize::new(0);
static WATCH_CHIP: AtomicUsize = AtomicUsize::new(0);
static SHOWS: AtomicU32 = AtomicU32::new(0);

pub(super) unsafe fn install_source(edit: HWND) -> bool {
    SetWindowSubclass(edit, Some(source_proc), SUBCLASS, 0) != 0
}

unsafe extern "system" fn source_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if message == SOURCE_DELAY {
        KillTimer(window, SOURCE_TIMER);
        DEFERRED.set(Deferred {
            delay: wparam as u32,
            ..Deferred::default()
        });
        COLLAPSED.store(false, Ordering::Release);
        PUBLISHED.store(false, Ordering::Release);
        return 0;
    }
    if message == WM_LBUTTONUP {
        let result = DefSubclassProc(window, message, wparam, lparam);
        let mut state = DEFERRED.get();
        if state.delay != 0 {
            SendMessageW(
                window,
                EM_GETSEL,
                &mut state.start as *mut _ as usize,
                &mut state.end as *mut _ as isize,
            );
            if state.start != state.end {
                // This is source-application behavior after an actual drag,
                // modelling an asynchronously published browser/PDF range.
                DEFERRED.set(state);
                SendMessageW(window, EM_SETSEL, state.end as usize, state.end as isize);
                COLLAPSED.store(true, Ordering::Release);
                SetTimer(window, SOURCE_TIMER, state.delay, None);
            }
        }
        return result;
    }
    if message == WM_TIMER && wparam == SOURCE_TIMER {
        KillTimer(window, SOURCE_TIMER);
        let state = DEFERRED.get();
        SendMessageW(window, EM_SETSEL, state.start as usize, state.end as isize);
        PUBLISHED.store(true, Ordering::Release);
        // The source publishes the real accessibility event for its own range.
        // No application controller or capture-complete message is injected.
        windows_sys::Win32::UI::Accessibility::NotifyWinEvent(
            EVENT_OBJECT_TEXTSELECTIONCHANGED,
            window,
            OBJID_CLIENT,
            0,
        );
        return 0;
    }
    DefSubclassProc(window, message, wparam, lparam)
}

unsafe extern "system" fn shown(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_OBJECT_SHOW
        && object == OBJID_WINDOW
        && child == 0
        && [
            WATCH_INLINE.load(Ordering::Acquire),
            WATCH_CHIP.load(Ordering::Acquire),
        ]
        .contains(&(window as usize))
    {
        SHOWS.fetch_add(1, Ordering::AcqRel);
    }
}
struct Watch(HWINEVENTHOOK);
impl Watch {
    fn start(app: &Application, inline: HWND, chip: HWND) -> Result<Self, String> {
        WATCH_INLINE.store(inline as usize, Ordering::Release);
        WATCH_CHIP.store(chip as usize, Ordering::Release);
        SHOWS.store(0, Ordering::Release);
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_SHOW,
                EVENT_OBJECT_SHOW,
                null_mut(),
                Some(shown),
                app.child.id(),
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_null() {
            Err("Cannot observe the owned recovery popups.".into())
        } else {
            Ok(Self(hook))
        }
    }
    fn reset(&self) {
        pump();
        SHOWS.store(0, Ordering::Release);
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        unsafe { UnhookWinEvent(self.0) };
    }
}
fn pump() {
    let mut message: MSG = unsafe { zeroed() };
    while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
fn until(mut condition: impl FnMut() -> Result<bool, String>, label: &str) -> Result<(), String> {
    let started = Instant::now();
    loop {
        pump();
        if condition()? {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(5) {
            return Err(format!("Recovery timeout: {label}"));
        }
        thread::sleep(Duration::from_millis(10));
    }
}
fn chord() -> Result<(), String> {
    let keys = [0x11, 0x12, 0x20, 0x20, 0x12, 0x11];
    let mut input: [INPUT; 6] = unsafe { zeroed() };
    for (i, event) in input.iter_mut().enumerate() {
        event.r#type = INPUT_KEYBOARD;
        event.Anonymous.ki.wVk = keys[i];
        event.Anonymous.ki.dwFlags = if i >= 3 { KEYEVENTF_KEYUP } else { 0 };
    }
    if unsafe { SendInput(6, input.as_ptr(), size_of::<INPUT>() as i32) } != 6 {
        return Err("Cannot send the owned manual shortcut.".into());
    }
    Ok(())
}
fn movement(point: POINT, flags: u32) -> Result<(), String> {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width < 2 || height < 2 {
        return Err("Invalid actual-input desktop bounds.".into());
    }
    let mut input: INPUT = unsafe { zeroed() };
    input.r#type = INPUT_MOUSE;
    input.Anonymous.mi.dx = (point.x - left) * 65_535 / (width - 1);
    input.Anonymous.mi.dy = (point.y - top) * 65_535 / (height - 1);
    input.Anonymous.mi.dwFlags =
        flags | MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        return Err("Cannot inject owned recovery mouse input.".into());
    }
    Ok(())
}
fn actual_drag(source: &Source, blank: bool) -> Result<(), String> {
    source.focus(false)?;
    let edit = source.edit as HWND;
    let mut first = character(edit, if blank { 10 } else { 5 })?;
    let mut last = character(edit, 10)?;
    first.x += if blank { 40 } else { 1 };
    first.y += 6;
    last.x += if blank { 160 } else { 1 };
    last.y += 6;
    let owned = |point| unsafe { WindowFromPoint(point) == edit };
    for step in 0..=14 {
        let point = POINT {
            x: first.x + (last.x - first.x) * step / 14,
            y: first.y + (last.y - first.y) * step / 14,
        };
        if !owned(point) {
            return Err("Recovery source is obscured; no foreign mouse input sent.".into());
        }
    }
    movement(first, 0)?;
    thread::sleep(Duration::from_millis(20));
    movement(first, MOUSEEVENTF_LEFTDOWN)?;
    let result = (|| {
        for step in 1..=14 {
            let point = POINT {
                x: first.x + (last.x - first.x) * step / 14,
                y: first.y + (last.y - first.y) * step / 14,
            };
            if !owned(point) {
                return Err("Recovery source became obscured during its drag.".into());
            }
            movement(point, 0)?;
            thread::sleep(Duration::from_millis(12));
        }
        movement(last, MOUSEEVENTF_LEFTUP)
    })();
    if result.is_err() {
        let mut up: INPUT = unsafe { zeroed() };
        up.r#type = INPUT_MOUSE;
        up.Anonymous.mi.dwFlags = MOUSEEVENTF_LEFTUP;
        unsafe { SendInput(1, &up, size_of::<INPUT>() as i32) };
    }
    result
}
fn require_range(source: &Source, start: u32, end: u32) -> Result<(), String> {
    let (mut actual_start, mut actual_end) = (0u32, 0u32);
    send(
        source.edit as HWND,
        EM_GETSEL,
        &mut actual_start as *mut _ as usize,
        &mut actual_end as *mut _ as isize,
    )?;
    if (actual_start, actual_end) != (start, end) {
        return Err(format!("Actual recovery gesture selected {actual_start}..{actual_end}, expected {start}..{end}."));
    }
    Ok(())
}

// A debugging-only scheduling pause of this exact test child's UI thread.
// Nothing is injected into production; a separate helper thread always resumes
// the owned thread, even if the source gesture fails midway.
struct UiPause(Option<JoinHandle<Result<(), String>>>);
impl UiPause {
    fn start(app: &Application) -> Result<Self, String> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
        let window = app.window(BACKGROUND)?;
        let mut pid = 0;
        let tid = unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid != app.child.id() || tid == 0 {
            return Err("Refusing to pause a foreign UI thread.".into());
        }
        let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, tid) };
        if handle.is_null() {
            return Err("Cannot open the owned UI thread for a scheduling-pause test.".into());
        }
        if unsafe { SuspendThread(handle) } == u32::MAX {
            unsafe {
                CloseHandle(handle);
            }
            return Err("Cannot pause the owned UI thread.".into());
        }
        let value = handle as usize;
        let worker = thread::Builder::new()
            .name("owned-ui-resumer".into())
            .spawn(move || {
                thread::sleep(Duration::from_millis(1500));
                let handle = value as windows_sys::Win32::Foundation::HANDLE;
                let resumed = unsafe { ResumeThread(handle) };
                unsafe {
                    CloseHandle(handle);
                }
                if resumed == u32::MAX {
                    Err("Cannot resume the owned UI thread.".into())
                } else {
                    Ok(())
                }
            });
        match worker {
            Ok(worker) => Ok(Self(Some(worker))),
            Err(_) => {
                unsafe {
                    ResumeThread(handle);
                    CloseHandle(handle);
                }
                Err("Cannot start the owned UI resumer.".into())
            }
        }
    }
    fn finish(mut self) -> Result<(), String> {
        self.0
            .take()
            .unwrap()
            .join()
            .map_err(|_| "Owned UI resumer panicked.".to_owned())?
    }
}
impl Drop for UiPause {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn check(
    source: &Source,
    app: &Application,
    inline: HWND,
    mock: &Mock,
) -> Result<(), String> {
    let detail = app.window("轻译 · 翻译与提问")?;
    let chip = app.window("轻译")?;
    let watch = Watch::start(app, inline, chip)?;
    let before = mock.request_count()?;
    let draft = text(detail, 222)?;
    if unsafe { IsWindowVisible(detail) } == 0
        || unsafe { GetWindowLongPtrW(GetDlgItem(detail, 222), GWL_STYLE) } & ES_READONLY as isize
            != 0
    {
        return Err("Manual recovery precondition requires an editable manual window.".into());
    }
    let fixture = wide(SOURCE_TEXT);
    send(
        source.edit as HWND,
        WM_SETTEXT,
        0,
        fixture.as_ptr() as isize,
    )?;
    unsafe {
        ShowWindowAsync(detail, SW_MINIMIZE);
    }
    until(
        || Ok(unsafe { IsIconic(detail) } != 0),
        "native manual minimization",
    )?;
    actual_drag(source, false)?;
    require_range(source, 5, 10)?;
    until(
        || {
            Ok(unsafe { IsWindowVisible(inline) } != 0
                && text(inline, 251)? == "新译文"
                && text(inline, 254)?.contains("缓存"))
        },
        "automatic selection after manual minimization",
    )?;
    until(
        || Ok(SHOWS.load(Ordering::Acquire) > 0),
        "native SHOW positive calibration",
    )?;
    if mock.request_count()? != before {
        return Err(
            "Recovery unexpectedly called the model instead of reusing the cached word.".into(),
        );
    }
    source.focus(false)?;
    chord()?;
    until(
        || {
            Ok(unsafe { IsWindowVisible(detail) } != 0
                && unsafe { IsIconic(detail) } == 0
                && text(detail, 222)? == draft)
        },
        "manual draft restored after minimizing",
    )?;
    send(detail, WM_CLOSE, 0, 0)?;
    send(inline, WM_CLOSE, 0, 0)?;
    source.focus(false)?;
    send(source.edit as HWND, SOURCE_DELAY, 250, 0)?;
    watch.reset();
    actual_drag(source, false)?;
    until(
        || Ok(COLLAPSED.load(Ordering::Acquire)),
        "source temporarily collapsed its delayed range",
    )?;
    let publication_started = Instant::now();
    while !PUBLISHED.load(Ordering::Acquire) {
        if publication_started.elapsed() > Duration::from_secs(3) {
            return Err("The owned source timer did not publish its delayed range.".into());
        }
        pump();
        if unsafe { IsWindowVisible(inline) } != 0
            || unsafe { IsWindowVisible(chip) } != 0
            || SHOWS.load(Ordering::Acquire) != 0
        {
            return Err(
                "A delayed source showed a speculative popup before publishing text.".into(),
            );
        }
        thread::sleep(Duration::from_millis(5));
    }
    require_range(source, 5, 10)?;
    until(
        || Ok(unsafe { IsWindowVisible(inline) } != 0 && text(inline, 251)? == "新译文"),
        "250ms delayed native range recovered",
    )?;
    send(inline, WM_CLOSE, 0, 0)?;
    source.focus(false)?;
    send(source.edit as HWND, SOURCE_DELAY, 250, 0)?;
    watch.reset();
    actual_drag(source, true)?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(6500) {
        pump();
        if unsafe { IsWindowVisible(inline) } != 0
            || unsafe { IsWindowVisible(chip) } != 0
            || SHOWS.load(Ordering::Acquire) != 0
        {
            return Err("A strong empty drag caused a popup during recovery retries.".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    require_range(source, 11, 11)?;
    if mock.request_count()? != before {
        return Err("Recovery or a blank drag unexpectedly sent a model request.".into());
    }
    send(source.edit as HWND, SOURCE_DELAY, 0, 0)?;
    send(inline, WM_CLOSE, 0, 0)?;
    source.focus(false)?;
    let main = app.window(BACKGROUND)?;
    let pause = UiPause::start(app)?;
    let mut ignored = 0;
    let responsive = unsafe {
        SendMessageTimeoutW(
            main,
            WM_NULL,
            0,
            0,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            100,
            &mut ignored,
        )
    };
    if responsive != 0 {
        return Err("The recovery test did not actually block the UI thread.".into());
    }
    let gesture = actual_drag(source, false);
    pause.finish()?;
    gesture?;
    require_range(source, 5, 10)?;
    until(
        || Ok(unsafe { IsWindowVisible(inline) } != 0 && text(inline, 251)? == "新译文"),
        "selection through a 1500ms main UI scheduling pause",
    )?;
    send(inline, WM_CLOSE, 0, 0)?;
    actual_drag(source, false)?;
    require_range(source, 5, 10)?;
    until(
        || Ok(unsafe { IsWindowVisible(inline) } != 0 && text(inline, 251)? == "新译文"),
        "mouse listener still works after the main UI pause",
    )?;
    if mock.request_count()? != before {
        return Err("The UI-pause recovery unexpectedly sent a model request.".into());
    }
    println!("Recovery actual input passed: minimized manual window resumes selection, draft restored, 250ms source publication recovered without speculative SHOW, calibrated 6500ms strong-blank zero popup/POST, listener survives 1500ms main-UI scheduling pause and next gesture; 14-step genuine SendInput, no selection-controller messages.");
    Ok(())
}
