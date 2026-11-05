//! Screen color sampling without an overlay or screenshot. Temporary low-level
//! hooks consume the confirmation click; only the worker reads the real pixel.

use std::{
    cell::Cell,
    mem::zeroed,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        HiDpi::{
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON},
        WindowsAndMessaging::*,
    },
};

const PICK_PIXEL: u32 = WM_APP + 71;
const CANCEL_PICK: u32 = WM_APP + 72;
static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct PickerState {
    thread_id: u32,
    left_pressed: bool,
    right_pressed: bool,
    ignore_left_release: bool,
    ignore_right_release: bool,
    finishing: bool,
    cross_cursor: isize,
}

impl PickerState {
    const fn empty() -> Self {
        Self {
            thread_id: 0,
            left_pressed: false,
            right_pressed: false,
            ignore_left_release: false,
            ignore_right_release: false,
            finishing: false,
            cross_cursor: 0,
        }
    }
}

thread_local! {
    static STATE: Cell<PickerState> = const { Cell::new(PickerState::empty()) };
}

struct Busy;

impl Busy {
    fn acquire() -> Result<Self, String> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "屏幕取色已经打开，请点击屏幕或按 Esc 取消。".into())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

struct ThreadDpi(DPI_AWARENESS_CONTEXT);

impl Drop for ThreadDpi {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

struct NativeHooks {
    mouse: HHOOK,
    keyboard: HHOOK,
    previous_cursor: HCURSOR,
}

impl Drop for NativeHooks {
    fn drop(&mut self) {
        unsafe {
            if !self.mouse.is_null() {
                UnhookWindowsHookEx(self.mouse);
            }
            if !self.keyboard.is_null() {
                UnhookWindowsHookEx(self.keyboard);
            }
            STATE.set(PickerState::empty());
            if !self.previous_cursor.is_null() {
                SetCursor(self.previous_cursor);
            }
        }
    }
}

/// Read the pixel chosen by the next complete left click. Esc or a complete
/// right click cancels. Run this on a dedicated worker with no other UI work.
///
/// No covering window, bitmap, screenshot, clipboard or system-cursor setting is
/// created. The result is RGB (`0xRRGGBB`), not Win32's COLORREF byte ordering.
/// A temporary cross cursor is best effort: the source app may set its cursor.
pub(crate) fn pick_screen_color() -> Result<Option<u32>, String> {
    let _busy = Busy::acquire()?;
    unsafe { run_picker() }
}

unsafe fn run_picker() -> Result<Option<u32>, String> {
    // Keep virtual-desktop coordinates physical, including negative positions
    // and mixed-DPI monitors. Restore this worker's previous context on exit.
    let _dpi = ThreadDpi(SetThreadDpiAwarenessContext(
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    ));
    let mut queued: MSG = zeroed();
    // PostThreadMessage requires a pre-existing message queue.
    PeekMessageW(&mut queued, ptr::null_mut(), 0, 0, PM_NOREMOVE);
    if GetAsyncKeyState(VK_ESCAPE as i32) < 0 {
        return Ok(None);
    }
    let cross = LoadCursorW(ptr::null_mut(), IDC_CROSS);
    let mut state = PickerState::empty();
    state.thread_id = GetCurrentThreadId();
    // Do not steal the release of a click/drag that began before picking.
    state.ignore_left_release = GetAsyncKeyState(VK_LBUTTON as i32) < 0;
    state.ignore_right_release = GetAsyncKeyState(VK_RBUTTON as i32) < 0;
    state.cross_cursor = cross as isize;
    STATE.set(state);
    let mut hooks = NativeHooks {
        mouse: ptr::null_mut(),
        keyboard: ptr::null_mut(),
        previous_cursor: GetCursor(),
    };
    let instance = GetModuleHandleW(ptr::null());
    hooks.keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), instance, 0);
    if hooks.keyboard.is_null() {
        return Err(format!(
            "无法监听取色取消键（系统错误 {}）。",
            GetLastError()
        ));
    }
    hooks.mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), instance, 0);
    if hooks.mouse.is_null() {
        return Err(format!(
            "无法监听屏幕取色点击（系统错误 {}）。",
            GetLastError()
        ));
    }
    if !cross.is_null() {
        SetCursor(cross);
    }
    let mut message: MSG = zeroed();
    loop {
        let status = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
        if status < 0 {
            return Err("屏幕取色消息处理失败，请重试。".into());
        }
        if status == 0 || message.message == CANCEL_PICK {
            return Ok(None);
        }
        if message.message == PICK_PIXEL {
            let point = POINT {
                x: message.wParam as isize as i32,
                y: message.lParam as i32,
            };
            // Stop intercepting before querying the screen. No overlay has
            // obscured the pixel, so no compositor repaint/flush is required.
            drop(hooks);
            return sample_pixel(point).map(Some);
        }
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

fn post_result(message: u32, point: POINT) {
    let mut state = STATE.get();
    if state.finishing || state.thread_id == 0 {
        return;
    }
    state.finishing = true;
    STATE.set(state);
    // The queue belongs to this hook thread and was created before installing.
    if unsafe {
        PostThreadMessageW(
            state.thread_id,
            message,
            point.x as isize as usize,
            point.y as isize,
        )
    } == 0
    {
        // A failed post must not leave a picker that can never finish.
        let mut state = STATE.get();
        state.finishing = false;
        STATE.set(state);
    }
}

unsafe extern "system" fn mouse_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code < 0 || data == 0 {
        return CallNextHookEx(ptr::null_mut(), code, message, data);
    }
    let mouse = &*(data as *const MSLLHOOKSTRUCT);
    let mut state = STATE.get();
    if state.thread_id == 0 || state.finishing {
        return CallNextHookEx(ptr::null_mut(), code, message, data);
    }
    match message as u32 {
        WM_MOUSEMOVE => {
            // This changes only the current cursor, never a system cursor. The
            // source application's own WM_SETCURSOR handling may override it.
            if state.cross_cursor != 0 {
                SetCursor(state.cross_cursor as HCURSOR);
            }
        }
        WM_LBUTTONDOWN if !state.ignore_left_release => {
            state.left_pressed = true;
            STATE.set(state);
            return 1;
        }
        WM_LBUTTONUP => {
            if state.ignore_left_release {
                state.ignore_left_release = false;
                state.left_pressed = false;
                STATE.set(state);
            } else if state.left_pressed {
                state.left_pressed = false;
                STATE.set(state);
                post_result(PICK_PIXEL, mouse.pt);
                return 1;
            }
        }
        WM_RBUTTONDOWN if !state.ignore_right_release => {
            state.right_pressed = true;
            STATE.set(state);
            return 1;
        }
        WM_RBUTTONUP => {
            if state.ignore_right_release {
                state.ignore_right_release = false;
                state.right_pressed = false;
                STATE.set(state);
            } else if state.right_pressed {
                state.right_pressed = false;
                STATE.set(state);
                post_result(CANCEL_PICK, mouse.pt);
                return 1;
            }
        }
        _ => {}
    }
    CallNextHookEx(ptr::null_mut(), code, message, data)
}

unsafe extern "system" fn keyboard_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 && data != 0 {
        let key = &*(data as *const KBDLLHOOKSTRUCT);
        if key.vkCode == VK_ESCAPE as u32 {
            if matches!(message as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
                post_result(CANCEL_PICK, POINT { x: 0, y: 0 });
            }
            return 1;
        }
    }
    CallNextHookEx(ptr::null_mut(), code, message, data)
}

unsafe fn sample_pixel(point: POINT) -> Result<u32, String> {
    let dc = GetDC(ptr::null_mut());
    if dc.is_null() {
        return Err("无法读取当前屏幕像素。".into());
    }
    let color = GetPixel(dc, point.x, point.y);
    ReleaseDC(ptr::null_mut(), dc);
    if color == CLR_INVALID {
        return Err("这个屏幕位置无法取色，请换一个位置。".into());
    }
    Ok(colorref_to_rgb(color))
}

fn colorref_to_rgb(color: u32) -> u32 {
    ((color & 255) << 16) | (color & 0xFF00) | ((color >> 16) & 255)
}

#[cfg(test)]
mod tests {
    use super::colorref_to_rgb;

    #[test]
    fn screen_color_channels_preserve_black_white_and_asymmetric_rgb() {
        assert_eq!(colorref_to_rgb(0), 0);
        assert_eq!(colorref_to_rgb(0xFFFFFF), 0xFFFFFF);
        assert_eq!(colorref_to_rgb(0x5A78C8), 0xC8785A);
        assert_eq!(colorref_to_rgb(0xDEA448), 0x48A4DE);
    }
}
