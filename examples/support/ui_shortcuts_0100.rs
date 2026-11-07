// Keyboard registration must change on Save, roll back conflicts, and release on Exit.
use super::*;
use windows_sys::Win32::UI::{
    Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    },
    WindowsAndMessaging::{GetWindowLongPtrW, ES_READONLY, GWL_STYLE},
};

const DETAIL: &str = "轻译 · 翻译与提问";
const RESERVED: i32 = 0x4c10;

struct Registration;
impl Drop for Registration {
    fn drop(&mut self) {
        unsafe { UnregisterHotKey(null_mut(), RESERVED) };
    }
}

fn chord(key: u16) -> Result<(), String> {
    let mut inputs: [INPUT; 6] = unsafe { zeroed() };
    for (input, (key, flags)) in inputs.iter_mut().zip([
        (0x11, 0),
        (0x12, 0),
        (key, 0),
        (key, KEYEVENTF_KEYUP),
        (0x12, KEYEVENTF_KEYUP),
        (0x11, KEYEVENTF_KEYUP),
    ]) {
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki.wVk = key;
        input.Anonymous.ki.dwFlags = flags;
    }
    if unsafe { SendInput(6, inputs.as_ptr(), size_of::<INPUT>() as i32) } != 6 {
        return Err("Cannot send the real manual-translation shortcut.".into());
    }
    Ok(())
}

fn wait(label: &str, mut predicate: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if predicate()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("Timed out waiting for {label}."));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn open(owned: &mut OwnedApplication, key: u16, settings: HWND) -> Result<HWND, String> {
    focus_settings_root(settings, window_thread(settings, owned.child.id())?)?;
    chord(key)?;
    let detail = owned.wait_for_window(DETAIL, true)?;
    require_centered(detail)?;
    require_visible(detail, 302)?;
    if unsafe { GetWindowLongPtrW(control(detail, 222)?, GWL_STYLE) } & ES_READONLY as isize != 0 {
        return Err("Global manual shortcut opened a read-only editor.".into());
    }
    wait("manual editor activation", || {
        Ok(unsafe { GetForegroundWindow() } == detail)
    })?;
    Ok(detail)
}

pub(super) fn check(owned: &mut OwnedApplication, settings: HWND) -> Result<(), String> {
    show_preferences(settings)?;
    require_text(settings, MANUAL_HOTKEY, "Ctrl + Alt + Space")?;
    let initial = saved_settings_bytes(owned)?;
    let detail = open(owned, 0x20, settings)?;
    send(detail, WM_CLOSE, 0, 0)?;
    require_settings_bytes(owned, &initial, "opening and closing the manual editor")?;

    set_control_text(settings, MANUAL_HOTKEY, "Ctrl+Alt+M")?;
    save_settings(owned, settings)?;
    let saved = saved_settings_bytes(owned)?;
    let value = saved_settings(owned)?;
    if value["manual_hotkey"] != serde_json::json!({"modifiers": 3, "virtual_key": 77}) {
        return Err("Saving the manual shortcut did not persist Ctrl+Alt+M.".into());
    }
    let detail = open(owned, b'M' as u16, settings)?;
    send(detail, WM_CLOSE, 0, 0)?;
    focus_settings_root(settings, window_thread(settings, owned.child.id())?)?;
    chord(0x20)?;
    thread::sleep(Duration::from_millis(300));
    if unsafe { IsWindowVisible(detail) } != 0 {
        return Err("The old manual shortcut remained active after a live Save.".into());
    }

    if unsafe {
        RegisterHotKey(
            null_mut(),
            RESERVED,
            MOD_ALT | MOD_CONTROL | MOD_NOREPEAT,
            u32::from(b'N'),
        )
    } == 0
    {
        return Err("Cannot reserve the isolated shortcut-conflict fixture.".into());
    }
    let reservation = Registration;
    set_control_text(settings, MANUAL_HOTKEY, "Ctrl+Alt+N")?;
    if unsafe { PostMessageW(control(settings, SAVE)?, BM_CLICK, 0, 0) } == 0 {
        return Err("Cannot submit a conflicting manual shortcut.".into());
    }
    wait("the explicit shortcut-conflict result", || {
        Ok(control_label(settings, 215)?.contains("快捷键已被占用"))
    })?;
    require_settings_bytes(owned, &saved, "rejecting an occupied shortcut")?;
    let detail = open(owned, b'M' as u16, settings)?;
    send(detail, WM_CLOSE, 0, 0)?;
    require_settings_bytes(
        owned,
        &saved,
        "verifying the previous shortcut after conflict rollback",
    )?;
    drop(reservation);

    set_control_text(settings, MANUAL_HOTKEY, "Ctrl+Alt+Space")?;
    save_settings(owned, settings)?;
    let detail = open(owned, 0x20, settings)?;
    send(detail, WM_CLOSE, 0, 0)?;
    show_service(settings)?;
    println!("Manual shortcut settings passed: real key activation, live replacement, old key removal, occupied-key rollback without a disk write, and restoring the default.");
    Ok(())
}

pub(super) fn require_released() -> Result<(), String> {
    for key in [0x20, u32::from(b'M')] {
        if unsafe {
            RegisterHotKey(
                null_mut(),
                RESERVED,
                MOD_ALT | MOD_CONTROL | MOD_NOREPEAT,
                key,
            )
        } == 0
        {
            return Err(
                "An exited app retained a manual-translation shortcut registration.".into(),
            );
        }
        unsafe { UnregisterHotKey(null_mut(), RESERVED) };
    }
    println!("Exit released the current and previously assigned manual shortcuts.");
    Ok(())
}
