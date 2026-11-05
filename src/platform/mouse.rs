//! Lightweight mouse observation. Every completed external left click is only a
//! candidate; the controller checks actual selection and deduplicates its text.
//! No UI Automation, clipboard read, or HTTP work happens inside the hook.

use std::{
    cell::RefCell,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, HINSTANCE, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
    UI::WindowsAndMessaging::{
        CallNextHookEx, GetAncestor, GetWindowThreadProcessId, PostMessageW, SetWindowsHookExW,
        UnhookWindowsHookEx, WindowFromPoint, GA_ROOTOWNER, HHOOK, MSLLHOOKSTRUCT, WH_MOUSE_LL,
        WM_LBUTTONDOWN, WM_LBUTTONUP,
    },
};

static ENABLED: AtomicBool = AtomicBool::new(true);

struct HookContext {
    target: isize,
    message: u32,
    external_press: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Notice {
    Cancel,
    Candidate,
}

impl HookContext {
    fn observe(&mut self, own_window: bool, down: bool) -> Option<Notice> {
        if own_window {
            self.external_press = false;
            return None;
        }
        if down {
            self.external_press = true;
            return Some(Notice::Cancel);
        }
        // Do not depend on a 4px distance or a double-click timeout. Those
        // thresholds dropped tiny drags, triple clicks and Shift+click ranges.
        // A matching down prevents dragging our own popup into a source app from
        // being treated as a new text selection.
        let pressed = std::mem::take(&mut self.external_press);
        pressed.then_some(Notice::Candidate)
    }
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
            external_press: false,
        });
    });
    Ok(MouseHook(hook))
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.external_press = false;
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
    let mouse = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
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
    let notification = CONTEXT.with(|context| {
        let Ok(mut context) = context.try_borrow_mut() else {
            return None;
        };
        let context = context.as_mut()?;
        context
            .observe(own_window, message as u32 == WM_LBUTTONDOWN)
            .map(|notice| (context.target, context.message, notice))
    });
    if let Some((target, notification, notice)) = notification {
        let (x, y) = match notice {
            Notice::Cancel => (0, isize::MIN),
            Notice::Candidate => (mouse.pt.x as usize, mouse.pt.y as isize),
        };
        unsafe { PostMessageW(target as _, notification, x, y) };
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
}

#[cfg(test)]
mod tests {
    use super::{HookContext, Notice};

    fn context() -> HookContext {
        HookContext {
            target: 1,
            message: 2,
            external_press: false,
        }
    }

    #[test]
    fn short_drags_shift_clicks_and_all_three_click_releases_are_candidates() {
        let mut hook = context();
        // The selection provider, not cursor distance, decides whether these
        // releases really contain text. This also preserves all three clicks.
        for _ in 0..3 {
            assert_eq!(hook.observe(false, true), Some(Notice::Cancel));
            assert_eq!(hook.observe(false, false), Some(Notice::Candidate));
        }
        assert_eq!(hook.observe(false, false), None);
    }

    #[test]
    fn app_clicks_and_drags_from_app_to_source_never_create_candidates() {
        let mut hook = context();
        assert_eq!(hook.observe(true, true), None);
        assert_eq!(hook.observe(false, false), None);
        assert_eq!(hook.observe(false, true), Some(Notice::Cancel));
        assert_eq!(hook.observe(true, false), None);
        assert_eq!(hook.observe(false, false), None);
    }
}
