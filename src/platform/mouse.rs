//! Lightweight mouse observation. Every completed external left click is only a
//! candidate; the controller checks actual selection and manages its session.
//! No UI Automation, clipboard read, or HTTP work happens inside the hook.

use std::{
    cell::RefCell,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use windows_sys::Win32::{
    Foundation::{GetLastError, HINSTANCE, LPARAM, LRESULT, POINT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, GetDoubleClickTime, VK_SHIFT},
        WindowsAndMessaging::{
            CallNextHookEx, GetAncestor, GetSystemMetrics, GetWindowThreadProcessId, PostMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint, GA_ROOTOWNER, HHOOK,
            MSLLHOOKSTRUCT, SM_CXDOUBLECLK, SM_CYDOUBLECLK, WH_MOUSE_LL, WM_LBUTTONDOWN,
            WM_LBUTTONUP, WM_MOUSEMOVE,
        },
    },
};

static ENABLED: AtomicBool = AtomicBool::new(true);

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
            press: None,
            last_click: None,
        });
    });
    Ok(MouseHook(hook))
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.press = None;
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
        || !matches!(message as u32, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE)
        || data == 0
    {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) };
    }
    let mouse = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
    let point = (mouse.pt.x, mouse.pt.y);
    if message as u32 == WM_MOUSEMOVE {
        CONTEXT.with(|context| {
            if let Ok(mut context) = context.try_borrow_mut() {
                if let Some(context) = context.as_mut() {
                    context.observe_move(point);
                }
            }
        });
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
            .map(|notice| (context.target, context.message, notice))
    });
    if let Some((target, notification, notice)) = notification {
        let (x, y) = notice.parameters();
        unsafe { PostMessageW(target as _, notification, x, y) };
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
