//! WinEvent notifications carry window metadata to the UI controller. They are
//! hints only: the controller must validate the foreground and active gesture
//! before reading a selection. No text is read inside a callback.

use std::{
    cell::RefCell,
    marker::PhantomData,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND},
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            IsWindow, PostMessageW, EVENT_OBJECT_TEXTSELECTIONCHANGED, EVENT_SYSTEM_FOREGROUND,
            WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
        },
    },
};

pub const FOREGROUND: u32 = EVENT_SYSTEM_FOREGROUND;
pub const SELECTION: u32 = EVENT_OBJECT_TEXTSELECTIONCHANGED;

static INSTALLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct HookContext {
    target: isize,
    message: u32,
    foreground: HWINEVENTHOOK,
    selection: HWINEVENTHOOK,
}

thread_local! {
    static CONTEXT: RefCell<Option<HookContext>> = const { RefCell::new(None) };
}

/// Keep this guard on the installing UI thread, which must run a message loop.
/// Out-of-context WinEvents are delivered on that thread. The guard deliberately
/// cannot be sent or shared across threads because unhooking is thread-bound.
pub struct SelectionEvents {
    foreground: HWINEVENTHOOK,
    selection: HWINEVENTHOOK,
    _ui_thread: PhantomData<Rc<()>>,
}

pub fn install(target: isize, message: u32) -> Result<SelectionEvents, String> {
    if target == 0 || unsafe { IsWindow(target as HWND) } == 0 {
        return Err("无法安装选区通知：接收窗口已经失效。".into());
    }
    if INSTALLED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("已经安装选区通知监听。".into());
    }

    // Use two single-event hooks instead of subscribing to every event between
    // foreground changes and text selections. The callback only posts metadata.
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    let foreground = unsafe {
        SetWinEventHook(
            FOREGROUND,
            FOREGROUND,
            ptr::null_mut(),
            Some(event_callback),
            0,
            0,
            flags,
        )
    };
    if foreground.is_null() {
        let error = unsafe { GetLastError() };
        INSTALLED.store(false, Ordering::Release);
        return Err(format!("无法安装前台窗口通知（系统错误 {error}）。"));
    }
    let selection = unsafe {
        SetWinEventHook(
            SELECTION,
            SELECTION,
            ptr::null_mut(),
            Some(event_callback),
            0,
            0,
            flags,
        )
    };
    if selection.is_null() {
        let error = unsafe { GetLastError() };
        unsafe { UnhookWinEvent(foreground) };
        INSTALLED.store(false, Ordering::Release);
        return Err(format!("无法安装文字选区通知（系统错误 {error}）。"));
    }

    CONTEXT.with(|context| {
        *context.borrow_mut() = Some(HookContext {
            target,
            message,
            foreground,
            selection,
        });
    });
    Ok(SelectionEvents {
        foreground,
        selection,
        _ui_thread: PhantomData,
    })
}

impl Drop for SelectionEvents {
    fn drop(&mut self) {
        // Disable dispatch before unhooking in case teardown pumps messages.
        CONTEXT.with(|context| *context.borrow_mut() = None);
        unsafe {
            UnhookWinEvent(self.selection);
            UnhookWinEvent(self.foreground);
        }
        INSTALLED.store(false, Ordering::Release);
    }
}

unsafe extern "system" fn event_callback(
    hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _source_thread: u32,
    _time: u32,
) {
    if !matches!(event, FOREGROUND | SELECTION)
        || window.is_null()
        || unsafe { IsWindow(window) } == 0
    {
        return;
    }
    let notification = CONTEXT.with(|context| {
        let context = context.try_borrow().ok()?;
        let context = context.as_ref()?;
        let expected_hook = if event == FOREGROUND {
            context.foreground
        } else {
            context.selection
        };
        (hook == expected_hook).then_some((context.target, context.message))
    });
    if let Some((target, message)) = notification {
        // HWND is an untrusted hint; the receiver rechecks ownership, foreground
        // and the current gesture before doing any accessibility or HTTP work.
        unsafe { PostMessageW(target as HWND, message, event as usize, window as isize) };
    }
}
