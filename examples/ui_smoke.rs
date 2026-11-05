//! End-to-end native UI regression check; never uses a real API credential.
//! Run on Windows (or a separately identified Wine compatibility test):
//! `cargo run --example ui_smoke -- path/to/lingoleaf.exe`

#[cfg(not(windows))]
fn main() {
    eprintln!("ui_smoke requires Windows or a Windows-compatible runtime.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        eprintln!("UI smoke check failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        fs,
        mem::{size_of, zeroed},
        os::windows::io::AsRawHandle,
        path::PathBuf,
        process::{Child, Command},
        ptr::{null, null_mut},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::{
        Foundation::{FILETIME, HANDLE, HWND},
        System::Threading::{AttachThreadInput, GetCurrentThreadId, GetProcessTimes},
        UI::Input::KeyboardAndMouse::SetFocus,
        UI::WindowsAndMessaging::{
            FindWindowW, GetDlgItem, GetGUIThreadInfo, GetWindowRect, GetWindowThreadProcessId,
            IsWindowVisible, PeekMessageW, PostMessageW, SendMessageTimeoutW, SetForegroundWindow,
            BM_CLICK, CBN_SELCHANGE, CB_ERR, CB_GETCURSEL, CB_SETCURSEL, GUITHREADINFO, MSG,
            PM_NOREMOVE, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_COMMAND, WM_GETTEXT, WM_SETTEXT,
        },
    };

    const CLASS: &str = "LingoLeaf.NativeWindow";
    const SETTINGS_TITLE: &str = "轻译 · 翻译服务设置";
    const BACKGROUND_TITLE: &str = "轻译 · 后台";
    const APPEARANCE_TITLE: &str = "轻译 · 图标外观";
    const PROVIDER: i32 = 208;
    const KEY: i32 = 211;
    const SAVE: i32 = 105;
    const EXIT: usize = 102;
    const APPEARANCE: i32 = 108;
    const ICON_STYLE: i32 = 230;
    const ICON_APPLY: i32 = 232;
    const ICON_RESET: i32 = 233;
    const SELECTION_MODE: i32 = 216;
    const COMPATIBILITY_CAPTURE: i32 = 217;
    const ICON_SHAPE: i32 = 236;
    const ICON_SIZE: i32 = 237;
    const SCREEN_COLOR: i32 = 238;

    pub(super) fn run() -> Result<(), String> {
        let executable = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .ok_or_else(|| "Pass the application EXE as the first argument.".to_owned())?
            .canonicalize()
            .map_err(|error| format!("Cannot locate the application EXE: {error}"))?;

        // Checking every top-level window in the app's class also covers an
        // existing hidden background window. Never drive a user's instance.
        if !unsafe { FindWindowW(wide(CLASS).as_ptr(), null()) }.is_null() {
            return Err("LingoLeaf is already running; close it before this check.".into());
        }

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("Cannot create an isolated test directory: {error}"))?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("lingoleaf-ui-smoke-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory)
            .map_err(|error| format!("Cannot create an isolated test directory: {error}"))?;
        let child = match Command::new(&executable)
            .env("LOCALAPPDATA", &directory)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_dir(&directory);
                return Err(format!("Cannot start the test application: {error}"));
            }
        };
        let mut owned = OwnedApplication {
            child,
            executable,
            directory,
            stopped: false,
        };
        let settings = owned.wait_for_settings()?;
        let ui_thread = window_thread(settings, owned.child.id())?;
        focus_settings_root(settings, ui_thread)?;

        thread::sleep(Duration::from_millis(500));
        require_root_focus(settings, ui_thread)?;
        let before = process_cpu(&owned.child)?;
        thread::sleep(Duration::from_secs(2));
        let after = process_cpu(&owned.child)?;
        require_root_focus(settings, ui_thread)?;
        if owned
            .child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("The application exited during the idle repaint check.".into());
        }
        let elapsed_cpu = after.saturating_sub(before);
        println!(
            "Settings root idle CPU: {:.4} seconds over 2 seconds.",
            elapsed_cpu.as_secs_f64()
        );
        if elapsed_cpu >= Duration::from_millis(300) {
            return Err(
                "Settings root focus caused excessive idle CPU; possible repaint loop.".into(),
            );
        }

        select_provider(settings, 0)?;
        require_text(settings, 210, "https://api.deepseek.com/v1")?;
        require_text(settings, 212, "deepseek-chat")?;
        require_text(settings, 213, "30")?;
        require_text(settings, KEY, "")?;

        let marker = wide("ui-smoke-placeholder-not-a-secret");
        send(
            control(settings, KEY)?,
            WM_SETTEXT,
            0,
            marker.as_ptr() as isize,
        )?;
        require_text(settings, KEY, "ui-smoke-placeholder-not-a-secret")?;
        select_provider(settings, 1)?;
        require_text(settings, 210, "https://api.openai.com/v1")?;
        require_text(settings, 212, "gpt-4o-mini")?;
        require_text(settings, 213, "20")?;
        require_text(settings, KEY, "")?;
        require_text(settings, 205, "模型")?;
        require_visible(settings, 205)?;
        require_visible(settings, SAVE)?;
        println!("Provider presets, credential clearing, model label and Save visibility passed.");

        check_icon_appearance(&mut owned, settings)?;

        owned.stop()?;
        println!("UI smoke check passed; no API request was sent.");
        Ok(())
    }

    struct OwnedApplication {
        child: Child,
        executable: PathBuf,
        directory: PathBuf,
        stopped: bool,
    }

    impl OwnedApplication {
        fn restart(&mut self) -> Result<HWND, String> {
            self.stop()?;
            self.child = Command::new(&self.executable)
                .env("LOCALAPPDATA", &self.directory)
                .spawn()
                .map_err(|error| format!("Cannot restart the test application: {error}"))?;
            self.stopped = false;
            let background = self.wait_for_window(BACKGROUND_TITLE, false)?;
            // Post rather than send: the window can exist before startup has
            // installed its App state, while posted commands wait for the loop.
            unsafe { PostMessageW(background, WM_COMMAND, 100, 0) };
            self.wait_for_settings()
        }

        fn wait_for_window(&mut self, title: &str, visible: bool) -> Result<HWND, String> {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let window = unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(title).as_ptr()) };
                if !window.is_null()
                    && window_thread(window, self.child.id()).is_ok()
                    && (!visible || unsafe { IsWindowVisible(window) } != 0)
                {
                    return Ok(window);
                }
                if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                    return Err(format!(
                        "The test application exited while waiting for its window: {status}"
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "The test application did not show the expected window: {title}"
                    ));
                }
                thread::sleep(Duration::from_millis(40));
            }
        }

        fn wait_for_settings(&mut self) -> Result<HWND, String> {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let settings =
                    unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(SETTINGS_TITLE).as_ptr()) };
                if !settings.is_null()
                    && window_thread(settings, self.child.id()).is_ok()
                    && unsafe { IsWindowVisible(settings) } != 0
                {
                    return Ok(settings);
                }
                if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                    return Err(format!(
                        "The test application exited before settings appeared: {status}"
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(
                        "The first-run settings window did not appear within 20 seconds.".into(),
                    );
                }
                thread::sleep(Duration::from_millis(40));
            }
        }

        fn stop(&mut self) -> Result<(), String> {
            if self.stopped {
                return Ok(());
            }
            let main =
                unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(BACKGROUND_TITLE).as_ptr()) };
            // Recheck the owner immediately before messaging the hidden window.
            if !main.is_null() && window_thread(main, self.child.id()).is_ok() {
                unsafe { PostMessageW(main, WM_COMMAND, EXIT, 0) };
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                    self.stopped = true;
                    if status.success() {
                        return Ok(());
                    }
                    return Err(format!("The test application exited with {status}."));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err("The test application did not exit after its own tray Exit command.".into())
        }
    }

    fn check_icon_appearance(owned: &mut OwnedApplication, settings: HWND) -> Result<(), String> {
        // Save a key-free baseline first. Appearance saves must preserve every
        // unrelated field, including provider, timeout and selection preference.
        require_visible(settings, APPEARANCE)?;
        require_visible(settings, SELECTION_MODE)?;
        require_visible(settings, COMPATIBILITY_CAPTURE)?;
        require_combo(settings, SELECTION_MODE, 0)?;
        select_combo(settings, SELECTION_MODE, 1)?;
        click(settings, SAVE)?;
        let mut baseline = saved_settings(owned)?;
        if baseline
            .get("compatibility_capture")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
        {
            return Err(
                "Compatibility capture must remain disabled until explicitly chosen.".into(),
            );
        }
        if baseline
            .get("selection_mode")
            .and_then(serde_json::Value::as_str)
            != Some("direct")
        {
            return Err(
                "Saving direct selection mode did not persist selection_mode=direct.".into(),
            );
        }
        // Drive the owner-drawn checkbox through its real command path. Provider
        // presets must preserve this choice, as must the appearance saves below.
        click(settings, COMPATIBILITY_CAPTURE)?;
        select_provider(settings, 0)?;
        select_provider(settings, 1)?;
        click(settings, SAVE)?;
        let enabled = saved_settings(owned)?;
        baseline["compatibility_capture"] = serde_json::Value::Bool(true);
        if enabled != baseline {
            return Err(
                "Enabling compatibility capture did not preserve the other settings.".into(),
            );
        }
        let appearance = open_appearance(owned, settings)?;
        for id in [
            ICON_STYLE,
            ICON_APPLY,
            ICON_RESET,
            ICON_SHAPE,
            ICON_SIZE,
            SCREEN_COLOR,
            234,
            235,
        ] {
            require_visible(appearance, id)?;
        }
        select_combo(appearance, ICON_STYLE, 2)?;
        // Both supported size endpoints must save, together with each shape.
        for (index, shape, size) in [(0, "rounded", 16_u32), (1, "square", 64)] {
            select_combo(appearance, ICON_SHAPE, index)?;
            set_control_text(appearance, ICON_SIZE, &size.to_string())?;
            click(appearance, ICON_APPLY)?;
            let saved = saved_settings(owned)?;
            require_icon_geometry(&saved, shape, size)?;
            require_same_nonappearance_settings(&baseline, &saved)?;
        }
        select_combo(appearance, ICON_SHAPE, 2)?;
        set_control_text(appearance, ICON_SIZE, "48")?;
        click(appearance, ICON_APPLY)?;
        let letter = saved_settings(owned)?;
        if letter.get("icon_style").and_then(serde_json::Value::as_str) != Some("letter") {
            return Err("Applying the letter icon did not persist icon_style=letter.".into());
        }
        require_icon_geometry(&letter, "circle", 48)?;
        require_same_nonappearance_settings(&baseline, &letter)?;
        for invalid in ["15", "65"] {
            set_control_text(appearance, ICON_SIZE, invalid)?;
            click(appearance, ICON_APPLY)?;
            if saved_settings(owned)? != letter {
                return Err("An out-of-range icon size was persisted.".into());
            }
        }

        // A restart distinguishes persisted settings from merely updated draft
        // controls. The restarted process must reopen the saved style as A.
        let settings = owned.restart()?;
        require_text(settings, 210, "https://api.openai.com/v1")?;
        require_text(settings, 212, "gpt-4o-mini")?;
        require_combo(settings, SELECTION_MODE, 1)?;
        // A second click must disable the choice restored from disk. Checking
        // the whole document also protects mode, provider and saved icon values.
        click(settings, COMPATIBILITY_CAPTURE)?;
        click(settings, SAVE)?;
        let disabled = saved_settings(owned)?;
        let mut expected = letter.clone();
        expected["compatibility_capture"] = serde_json::Value::Bool(false);
        if disabled != expected {
            return Err("Restart did not restore the compatibility checkbox or toggling it changed unrelated settings.".into());
        }
        baseline["compatibility_capture"] = serde_json::Value::Bool(false);
        let appearance = open_appearance(owned, settings)?;
        require_combo(appearance, ICON_STYLE, 2)?;
        require_combo(appearance, ICON_SHAPE, 2)?;
        require_text(appearance, ICON_SIZE, "48")?;
        click(appearance, ICON_RESET)?;
        click(appearance, ICON_APPLY)?;
        let restored = saved_settings(owned)?;
        for field in ["icon_style", "icon_color", "icon_shape", "icon_size"] {
            if restored.get(field) != baseline.get(field) {
                return Err("Reset/apply did not restore the default icon appearance.".into());
            }
        }
        require_same_nonappearance_settings(&baseline, &restored)?;
        select_combo(settings, SELECTION_MODE, 0)?;
        click(settings, SAVE)?;
        if saved_settings(owned)?
            .get("selection_mode")
            .and_then(serde_json::Value::as_str)
            != Some("click")
        {
            return Err("Saving click selection mode did not persist selection_mode=click.".into());
        }
        println!("Selection modes, compatibility capture, icon styles/shapes, size limits, persistence, restart and default reset passed.");
        Ok(())
    }

    fn require_icon_geometry(
        saved: &serde_json::Value,
        shape: &str,
        size: u32,
    ) -> Result<(), String> {
        if saved.get("icon_shape").and_then(serde_json::Value::as_str) != Some(shape)
            || saved.get("icon_size").and_then(serde_json::Value::as_u64) != Some(u64::from(size))
        {
            return Err("The icon shape or size was not persisted as requested.".into());
        }
        Ok(())
    }

    fn saved_settings(owned: &OwnedApplication) -> Result<serde_json::Value, String> {
        let bytes = fs::read(owned.directory.join("LingoLeaf").join("settings.json"))
            .map_err(|_| "The test application's isolated settings were not saved.".to_owned())?;
        serde_json::from_slice(&bytes)
            .map_err(|_| "The test application's isolated settings are not valid JSON.".to_owned())
    }

    fn require_same_nonappearance_settings(
        before: &serde_json::Value,
        after: &serde_json::Value,
    ) -> Result<(), String> {
        let without_appearance = |value: &serde_json::Value| {
            value.as_object().map(|object| {
                let mut copy = object.clone();
                copy.remove("icon_style");
                copy.remove("icon_color");
                copy.remove("icon_shape");
                copy.remove("icon_size");
                copy
            })
        };
        if without_appearance(before).is_none()
            || without_appearance(before) != without_appearance(after)
        {
            return Err("Saving icon appearance changed unrelated translation settings.".into());
        }
        Ok(())
    }

    fn open_appearance(owned: &mut OwnedApplication, settings: HWND) -> Result<HWND, String> {
        send(settings, WM_COMMAND, APPEARANCE as usize, 0)?;
        owned.wait_for_window(APPEARANCE_TITLE, true)
    }

    fn click(parent: HWND, id: i32) -> Result<(), String> {
        send(control(parent, id)?, BM_CLICK, 0, 0)?;
        Ok(())
    }

    impl Drop for OwnedApplication {
        fn drop(&mut self) {
            if !self.stopped {
                let _ = self.stop();
            }
            if !self.stopped {
                // Child owns this process handle; never kill by discovered PID.
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn focus_settings_root(settings: HWND, ui_thread: u32) -> Result<(), String> {
        // AttachThreadInput requires that both threads already have queues.
        let mut pending: MSG = unsafe { zeroed() };
        unsafe { PeekMessageW(&mut pending, null_mut(), 0, 0, PM_NOREMOVE) };
        let calling_thread = unsafe { GetCurrentThreadId() };
        if unsafe { AttachThreadInput(calling_thread, ui_thread, 1) } == 0 {
            return Err("Cannot attach to the test application's input queue.".into());
        }
        struct Attached(u32, u32);
        impl Drop for Attached {
            fn drop(&mut self) {
                unsafe { AttachThreadInput(self.0, self.1, 0) };
            }
        }
        let _attached = Attached(calling_thread, ui_thread);
        unsafe {
            SetForegroundWindow(settings);
            SetFocus(settings);
        }
        require_root_focus(settings, ui_thread)
    }

    fn require_root_focus(settings: HWND, ui_thread: u32) -> Result<(), String> {
        let mut info: GUITHREADINFO = unsafe { zeroed() };
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        if unsafe { GetGUIThreadInfo(ui_thread, &mut info) } == 0 || info.hwndFocus != settings {
            return Err("The settings top-level window did not retain focus; repaint regression was not exercised.".into());
        }
        Ok(())
    }

    fn process_cpu(child: &Child) -> Result<Duration, String> {
        let mut creation: FILETIME = unsafe { zeroed() };
        let mut exit: FILETIME = unsafe { zeroed() };
        let mut kernel: FILETIME = unsafe { zeroed() };
        let mut user: FILETIME = unsafe { zeroed() };
        if unsafe {
            GetProcessTimes(
                child.as_raw_handle() as HANDLE,
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err("Cannot read the test application's CPU time.".into());
        }
        let ticks = |value: FILETIME| {
            (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
        };
        Ok(Duration::from_nanos(
            ticks(kernel)
                .saturating_add(ticks(user))
                .saturating_mul(100),
        ))
    }

    fn window_thread(window: HWND, expected_pid: u32) -> Result<u32, String> {
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if thread == 0 || pid != expected_pid {
            return Err("Refusing to operate on a window outside the spawned test process.".into());
        }
        Ok(thread)
    }

    fn control(settings: HWND, id: i32) -> Result<HWND, String> {
        let child = unsafe { GetDlgItem(settings, id) };
        if child.is_null() {
            return Err(format!("Required settings control {id} was not created."));
        }
        Ok(child)
    }

    fn send(window: HWND, message: u32, wparam: usize, lparam: isize) -> Result<usize, String> {
        let mut result = 0;
        if unsafe {
            SendMessageTimeoutW(
                window,
                message,
                wparam,
                lparam,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                3_000,
                &mut result,
            )
        } == 0
        {
            return Err("The test application's UI did not respond within three seconds.".into());
        }
        Ok(result)
    }

    fn select_provider(settings: HWND, index: usize) -> Result<(), String> {
        select_combo(settings, PROVIDER, index)
    }

    fn select_combo(parent: HWND, id: i32, index: usize) -> Result<(), String> {
        if send(control(parent, id)?, CB_SETCURSEL, index, 0)? as isize == CB_ERR as isize {
            return Err("The requested provider preset is unavailable.".into());
        }
        send(
            parent,
            WM_COMMAND,
            id as usize | ((CBN_SELCHANGE as usize) << 16),
            0,
        )?;
        Ok(())
    }

    fn require_combo(parent: HWND, id: i32, expected: usize) -> Result<(), String> {
        if send(control(parent, id)?, CB_GETCURSEL, 0, 0)? != expected {
            return Err(format!(
                "Control {id} did not restore the expected selection."
            ));
        }
        Ok(())
    }

    fn set_control_text(parent: HWND, id: i32, value: &str) -> Result<(), String> {
        let value = wide(value);
        send(control(parent, id)?, WM_SETTEXT, 0, value.as_ptr() as isize)?;
        Ok(())
    }

    fn require_text(settings: HWND, id: i32, expected: &str) -> Result<(), String> {
        let mut buffer = vec![0_u16; 4096];
        let count = send(
            control(settings, id)?,
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
        )?;
        if count >= buffer.len() || String::from_utf16_lossy(&buffer[..count]) != expected {
            // Do not echo a control's contents, especially the API key field.
            return Err(format!(
                "Settings control {id} did not contain the expected preset value."
            ));
        }
        Ok(())
    }

    fn require_visible(settings: HWND, id: i32) -> Result<(), String> {
        let child = control(settings, id)?;
        let mut bounds = unsafe { zeroed() };
        if unsafe { IsWindowVisible(child) } == 0
            || unsafe { GetWindowRect(child, &mut bounds) } == 0
            || bounds.right <= bounds.left
            || bounds.bottom <= bounds.top
        {
            return Err(format!("Settings control {id} is not visibly laid out."));
        }
        Ok(())
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
