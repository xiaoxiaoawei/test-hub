//! Lightweight mouse observation. The hook only posts notifications; selection
//! capture and HTTP requests must stay on the application's worker thread.

use std::{
    cell::RefCell,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, HINSTANCE, LPARAM, LRESULT, POINT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
    UI::{
        Input::KeyboardAndMouse::GetDoubleClickTime,
        WindowsAndMessaging::{
            CallNextHookEx, GetSystemMetrics, GetWindowThreadProcessId, PostMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint, HHOOK, MSLLHOOKSTRUCT,
            SM_CXDOUBLECLK, SM_CYDOUBLECLK, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
        },
    },
};

static ENABLED: AtomicBool = AtomicBool::new(true);

struct HookContext {
    target: isize,
    message: u32,
    pressed_at: Option<POINT>,
    last_click: Option<(POINT, u32)>,
}

thread_local! {
    static CONTEXT: RefCell<Option<HookContext>> = const { RefCell::new(None) };
}

/// Keep this guard on the installing UI thread, which must run a message loop.
pub struct MouseHook(HHOOK);

pub fn install(target_window: isize, message: u32) -> Result<MouseHook, String> {
    if target_window == 0 {
        return Err("无法安装划词监听：缺少接收窗口。".into());
    }
    if CONTEXT.with(|context| context.borrow().is_some()) {
        return Err("此线程已经安装划词监听。".into());
    }
    let module: HINSTANCE = unsafe { GetModuleHandleW(ptr::null()) };
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_callback), module, 0) };
    if hook.is_null() {
        return Err(format!(
            "无法安装划词监听（系统错误 {}）。",
            unsafe { GetLastError() }
        ));
    }
    CONTEXT.with(|context| {
        *context.borrow_mut() = Some(HookContext {
            target: target_window,
            message,
            pressed_at: None,
            last_click: None,
        });
    });
    Ok(MouseHook(hook))
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.pressed_at = None;
            context.last_click = None;
        }
    });
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        unsafe { UnhookWindowsHookEx(self.0) };
        CONTEXT.with(|context| *context.borrow_mut() = None);
    }
}

unsafe extern "system" fn mouse_callback(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code < 0
        || !ENABLED.load(Ordering::Relaxed)
        || !matches!(message as u32, WM_LBUTTONDOWN | WM_LBUTTONUP)
        || data == 0
    {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) };
    }
    // Windows owns this structure for the duration of the hook callback.
    let mouse = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
    let mut position_pid = 0;
    let position_window = unsafe { WindowFromPoint(mouse.pt) };
    unsafe { GetWindowThreadProcessId(position_window, &mut position_pid) };
    let own_window = position_pid != 0 && position_pid == unsafe { GetCurrentProcessId() };

    let notification = CONTEXT.with(|context| {
        // Never unwind across a Windows hook if a nested callback is delivered.
        let Ok(mut context) = context.try_borrow_mut() else {
            return None;
        };
        let context = context.as_mut()?;
        if own_window {
            context.pressed_at = None;
            context.last_click = None;
            return None;
        }
        if message as u32 == WM_LBUTTONDOWN {
            context.pressed_at = Some(mouse.pt);
            // Invalidate the previous selection immediately, even if this click
            // never becomes a drag. MIN cannot be a real screen coordinate.
            return Some((context.target, context.message, 0, isize::MIN));
        }
        let pressed = context.pressed_at.take()?;
        let dragged = distance(pressed.x, mouse.pt.x) >= 4 || distance(pressed.y, mouse.pt.y) >= 4;
        let double_click = context
            .last_click
            .map(|(previous, previous_time)| {
                let tolerance_x = (unsafe { GetSystemMetrics(SM_CXDOUBLECLK) } / 2).max(2) as i64;
                let tolerance_y = (unsafe { GetSystemMetrics(SM_CYDOUBLECLK) } / 2).max(2) as i64;
                mouse.time.wrapping_sub(previous_time) <= unsafe { GetDoubleClickTime() }
                    && distance(previous.x, mouse.pt.x) <= tolerance_x
                    && distance(previous.y, mouse.pt.y) <= tolerance_y
            })
            .unwrap_or(false);
        if dragged || double_click {
            context.last_click = None;
            Some((
                context.target,
                context.message,
                mouse.pt.x as usize,
                mouse.pt.y as isize,
            ))
        } else {
            context.last_click = Some((mouse.pt, mouse.time));
            None
        }
    });
    if let Some((target, notification, x, y)) = notification {
        unsafe { PostMessageW(target as _, notification, x, y) };
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
}

fn distance(a: i32, b: i32) -> i64 {
    (i64::from(a) - i64::from(b)).abs()
}
