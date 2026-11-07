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
    use lingoleaf::{
        config::{ThemeMode, ThemePreset},
        visual::Palette,
    };
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
        Foundation::{CloseHandle, FILETIME, HANDLE, HWND, POINT, RECT, WAIT_OBJECT_0},
        Graphics::Gdi::{
            GetDC, GetMonitorInfoW, GetPixel, MonitorFromPoint, ReleaseDC, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
        System::Threading::{
            AttachThreadInput, CreateEventW, GetCurrentThreadId, GetProcessTimes,
            WaitForSingleObject,
        },
        UI::Input::KeyboardAndMouse::{
            IsWindowEnabled, SendInput, SetFocus, INPUT, INPUT_KEYBOARD, INPUT_MOUSE,
            KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, VK_HOME, VK_RETURN,
        },
        UI::WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetCursorPos, GetDlgItem, GetForegroundWindow,
            GetGUIThreadInfo, GetWindow, GetWindowRect, GetWindowThreadProcessId, IsWindow,
            IsWindowVisible, PeekMessageW, PostMessageW, SendMessageTimeoutW, SetCursorPos,
            SetForegroundWindow, SetWindowPos, BM_CLICK, CBN_SELCHANGE, CB_ERR, CB_GETCURSEL,
            CB_GETDROPPEDSTATE, CB_SETCURSEL, GUITHREADINFO, GW_OWNER, MSG, PM_NOREMOVE,
            SMTO_ABORTIFHUNG, SMTO_BLOCK, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WM_APP,
            WM_CLOSE, WM_COMMAND, WM_GETTEXT, WM_SETTEXT,
        },
    };

    const CLASS: &str = "LingoLeaf.NativeWindow";
    const SETTINGS_TITLE: &str = "轻译 · 设置";
    const BACKGROUND_TITLE: &str = "轻译 · 后台";
    const APPEARANCE_TITLE: &str = "轻译 · 划词图标";
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
    const THEME: i32 = 109;
    const UPDATE: i32 = 110;
    const UPDATE_STATUS: i32 = 219;
    const SERVICE_TAB: i32 = 270;
    const PREFERENCES_TAB: i32 = 271;
    const APPEARANCE_TAB: i32 = 280;
    const PRESET_CARDS: [(i32, ThemePreset, &str); 4] = [
        (281, ThemePreset::Ocean, "ocean"),
        (282, ThemePreset::Aurora, "aurora"),
        (283, ThemePreset::Nebula, "nebula"),
        (284, ThemePreset::Amber, "amber"),
    ];
    const SOURCE_LANGUAGE: i32 = 272;
    const TARGET_LANGUAGE: i32 = 273;
    const PAUSE_SELECTION: i32 = 264;
    const SWAP_LANGUAGES: i32 = 265;
    const KEY_VISIBILITY: i32 = 267;
    const EM_GETPASSWORDCHAR: u32 = 0x00D2;
    const SAVE_TITLE: &str = "轻译 · 保存成功";
    const SAVE_BODY: &str = "设置保存成功，已立即生效。";

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
        // Exercise an actual pre-language profile. The application must migrate
        // missing preferences to English -> Simplified Chinese without a key.
        let config_directory = directory.join("LingoLeaf");
        fs::create_dir(&config_directory).map_err(|error| error.to_string())?;
        fs::write(
            config_directory.join("settings.json"),
            br#"{"version":1,"base_url":"https://api.openai.com/v1","model":"gpt-4o-mini","timeout_seconds":20,"api_key_encrypted":[]}"#,
        )
        .map_err(|error| error.to_string())?;
        let restart_nonce = format!("{nonce:032x}");
        let ready = unsafe {
            CreateEventW(
                null(),
                1,
                0,
                wide(&format!("Local\\LingoLeaf.Update.AppReady.{restart_nonce}")).as_ptr(),
            )
        };
        if ready.is_null() {
            let _ = fs::remove_dir(&directory);
            return Err("Cannot create the isolated update startup acknowledgement event.".into());
        }
        let ready = StartupEvent(ready);
        let startup = Instant::now();
        let child = match Command::new(&executable)
            .env("LOCALAPPDATA", &directory)
            .args(["--lingoleaf-update-restarted", &restart_nonce])
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
        if unsafe { WaitForSingleObject(ready.0, 3_000) } != WAIT_OBJECT_0 {
            return Err(
                "The real application did not acknowledge successful update startup.".into(),
            );
        }
        if startup.elapsed() >= Duration::from_secs(5) {
            return Err("Opening settings took over five seconds; startup update checking may be blocking the UI.".into());
        }
        require_centered(settings)?;
        for id in [THEME, SERVICE_TAB, PREFERENCES_TAB, APPEARANCE_TAB, SAVE] {
            require_visible(settings, id)?;
        }
        require_visible(settings, PROVIDER)?;
        require_hidden(settings, SOURCE_LANGUAGE)?;
        show_preferences(settings)?;
        require_combo(settings, SOURCE_LANGUAGE, 1)?;
        require_combo(settings, TARGET_LANGUAGE, 1)?;
        show_service(settings)?;
        check_repeated_launch(&mut owned, settings)?;
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

        check_native_combo_interaction(settings)?;
        select_provider(settings, 0)?;
        require_text(settings, 210, "https://api.deepseek.com/v1")?;
        require_text(settings, 212, "deepseek-chat")?;
        require_text(settings, 213, "30")?;
        require_text(settings, KEY, "")?;

        check_key_visibility(&owned, settings)?;
        select_provider(settings, 1)?;
        require_text(settings, 210, "https://api.openai.com/v1")?;
        require_text(settings, 212, "gpt-4o-mini")?;
        require_text(settings, 213, "20")?;
        require_text(settings, KEY, "")?;
        require_text(settings, 205, "模型")?;
        require_visible(settings, 205)?;
        require_visible(settings, SAVE)?;
        println!("Provider presets, credential clearing, model label and Save visibility passed.");

        let settings = check_languages(&mut owned, settings)?;
        let settings = check_preference_actions(&mut owned, settings)?;
        check_live_save(&mut owned, settings)?;
        let settings = check_theme(&mut owned, settings)?;
        let settings = check_theme_presets(&mut owned, settings)?;
        check_icon_appearance(&mut owned, settings)?;

        owned.stop()?;
        println!("UI smoke check passed; no translation API request was sent.");
        Ok(())
    }

    struct OwnedApplication {
        child: Child,
        executable: PathBuf,
        directory: PathBuf,
        stopped: bool,
    }

    struct StartupEvent(HANDLE);
    impl Drop for StartupEvent {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    impl OwnedApplication {
        fn restart(&mut self) -> Result<HWND, String> {
            self.stop()?;
            self.child = Command::new(&self.executable)
                .env("LOCALAPPDATA", &self.directory)
                .spawn()
                .map_err(|error| format!("Cannot restart the test application: {error}"))?;
            self.stopped = false;
            // A saved profile must still open settings automatically. Never
            // post an Open command here: it would hide a startup regression.
            let settings = self.wait_for_settings()?;
            require_centered(settings)?;
            Ok(settings)
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
                        "The settings window did not open automatically within 20 seconds.".into(),
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

    fn require_centered(window: HWND) -> Result<(), String> {
        let mut pointer: POINT = unsafe { zeroed() };
        let mut monitor: MONITORINFO = unsafe { zeroed() };
        monitor.cbSize = size_of::<MONITORINFO>() as u32;
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetCursorPos(&mut pointer) } == 0
            || unsafe {
                GetMonitorInfoW(
                    MonitorFromPoint(pointer, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                )
            } == 0
            || unsafe { GetWindowRect(window, &mut rect) } == 0
        {
            return Err("Cannot inspect the settings monitor and window bounds.".into());
        }
        let work = monitor.rcWork;
        let expected_x = i64::from(work.left)
            + (i64::from(work.right - work.left) - i64::from(rect.right - rect.left)) / 2;
        let expected_y = i64::from(work.top)
            + (i64::from(work.bottom - work.top) - i64::from(rect.bottom - rect.top)) / 2;
        if (i64::from(rect.left) - expected_x).abs() > 4
            || (i64::from(rect.top) - expected_y).abs() > 4
        {
            return Err(format!(
                "Settings were not centered on the pointer's monitor: actual ({}, {}), expected ({expected_x}, {expected_y}).",
                rect.left, rect.top
            ));
        }
        Ok(())
    }

    fn check_repeated_launch(owned: &mut OwnedApplication, settings: HWND) -> Result<(), String> {
        unsafe {
            SetWindowPos(
                settings,
                null_mut(),
                25,
                25,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        send(settings, WM_CLOSE, 0, 0)?;
        let mut duplicate = Command::new(&owned.executable)
            .env("LOCALAPPDATA", &owned.directory)
            .spawn()
            .map_err(|error| format!("Cannot test a repeated launch: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match duplicate.try_wait().map_err(|error| error.to_string())? {
                Some(status) if status.success() => break,
                Some(status) => return Err(format!("The repeated launch exited with {status}.")),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(30)),
                None => {
                    let _ = duplicate.kill();
                    let _ = duplicate.wait();
                    return Err(
                        "The repeated launch did not return to the original instance.".into(),
                    );
                }
            }
        }
        let shown = owned.wait_for_settings()?;
        if shown != settings {
            return Err("A repeated launch created a second settings window.".into());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if require_centered(settings).is_ok() && unsafe { GetForegroundWindow() } == settings {
                println!(
                    "Startup and repeated-launch settings visibility, centering and focus passed."
                );
                return Ok(());
            }
            thread::sleep(Duration::from_millis(30));
        }
        Err("A repeated launch did not center and focus the existing settings window.".into())
    }

    fn save_settings(owned: &OwnedApplication, settings: HWND) -> Result<(), String> {
        if unsafe { PostMessageW(control(settings, SAVE)?, BM_CLICK, 0, 0) } == 0 {
            return Err("Cannot request a settings save.".into());
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        let dialog = loop {
            let dialog = unsafe { FindWindowW(wide("#32770").as_ptr(), wide(SAVE_TITLE).as_ptr()) };
            if !dialog.is_null()
                && window_thread(dialog, owned.child.id()).is_ok()
                && unsafe { IsWindowVisible(dialog) } != 0
            {
                break dialog;
            }
            if Instant::now() >= deadline {
                return Err("Saving settings did not show its native success dialog.".into());
            }
            thread::sleep(Duration::from_millis(25));
        };
        if unsafe { GetWindow(dialog, GW_OWNER) } != settings
            || unsafe {
                FindWindowExW(
                    dialog,
                    null_mut(),
                    wide("Static").as_ptr(),
                    wide(SAVE_BODY).as_ptr(),
                )
            }
            .is_null()
        {
            return Err(
                "The save confirmation did not belong to settings or show its success message."
                    .into(),
            );
        }
        click(dialog, 1)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { IsWindow(dialog) } != 0 {
            if Instant::now() >= deadline {
                return Err("The native save confirmation did not close after OK.".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    fn check_live_save(owned: &mut OwnedApplication, settings: HWND) -> Result<(), String> {
        show_service(settings)?;
        select_provider(settings, 0)?;
        save_settings(owned, settings)?;
        set_control_text(settings, 212, "ui-smoke-unsaved-draft")?;
        // Bringing an already-visible settings window forward intentionally
        // preserves a user's draft. Closing and reopening reloads the active
        // in-memory settings and therefore exercises immediate application.
        send(settings, WM_CLOSE, 0, 0)?;
        let main = owned.wait_for_window(BACKGROUND_TITLE, false)?;
        unsafe { PostMessageW(main, WM_APP + 4, 0, 0) };
        let deadline = Instant::now() + Duration::from_secs(3);
        while require_text(settings, 212, "deepseek-chat").is_err() {
            if Instant::now() >= deadline {
                return Err("Saved API settings were not applied to the running process.".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        require_text(settings, 210, "https://api.deepseek.com/v1")?;
        select_provider(settings, 1)?;
        save_settings(owned, settings)?;
        println!("Native save confirmation and immediate API settings application passed.");
        Ok(())
    }

    fn check_key_visibility(owned: &OwnedApplication, settings: HWND) -> Result<(), String> {
        require_visible(settings, KEY_VISIBILITY)?;
        let input = control(settings, KEY)?;
        if send(input, EM_GETPASSWORDCHAR, 0, 0)? == 0 {
            return Err("The API key field was not masked before explicit reveal.".into());
        }
        // This is the same fixed public marker already used by the provider
        // regression. It is cleared by the following provider preset, and no
        // Save operation occurs while it is present. Never echo field contents.
        let marker = "ui-smoke-placeholder-not-a-secret";
        set_control_text(settings, KEY, marker)?;
        let saved = saved_settings_bytes(owned)?;
        click(settings, KEY_VISIBILITY)?;
        if send(input, EM_GETPASSWORDCHAR, 0, 0)? != 0 {
            return Err("Explicit API key reveal did not remove the password mask.".into());
        }
        require_text(settings, KEY, marker)?;
        require_settings_bytes(owned, &saved, "revealing the API key draft")?;
        click(settings, KEY_VISIBILITY)?;
        if send(input, EM_GETPASSWORDCHAR, 0, 0)? == 0 {
            return Err("The second API key eye click did not restore masking.".into());
        }
        require_text(settings, KEY, marker)?;
        require_settings_bytes(owned, &saved, "masking the API key draft")?;
        println!("Explicit API key reveal/mask preserved the draft and never saved it.");
        Ok(())
    }

    fn check_preference_actions(
        owned: &mut OwnedApplication,
        settings: HWND,
    ) -> Result<HWND, String> {
        show_preferences(settings)?;
        require_combo(settings, SOURCE_LANGUAGE, 0)?;
        require_combo(settings, TARGET_LANGUAGE, 3)?;
        if unsafe { IsWindowEnabled(control(settings, SWAP_LANGUAGES)?) } != 0 {
            return Err("Language swap must be disabled for an automatic source.".into());
        }
        let saved = saved_settings_bytes(owned)?;
        // Both are drafts: swapping English -> Japanese should produce
        // Japanese -> English, while the saved Auto -> Japanese stays intact.
        select_combo(settings, SOURCE_LANGUAGE, 1)?;
        select_combo(settings, TARGET_LANGUAGE, 3)?;
        if unsafe { IsWindowEnabled(control(settings, SWAP_LANGUAGES)?) } == 0 {
            return Err("Language swap did not enable for two explicit languages.".into());
        }
        click(settings, SWAP_LANGUAGES)?;
        require_combo(settings, SOURCE_LANGUAGE, 4)?;
        require_combo(settings, TARGET_LANGUAGE, 0)?;
        require_settings_bytes(owned, &saved, "swapping unsaved language preferences")?;
        send(settings, WM_CLOSE, 0, 0)?;
        let main = owned.wait_for_window(BACKGROUND_TITLE, false)?;
        if unsafe { PostMessageW(main, WM_APP + 4, 0, 0) } == 0 {
            return Err(
                "Cannot reopen the owned settings after discarding language drafts.".into(),
            );
        }
        let settings = owned.wait_for_settings()?;
        show_preferences(settings)?;
        require_combo(settings, SOURCE_LANGUAGE, 0)?;
        require_combo(settings, TARGET_LANGUAGE, 3)?;
        require_settings_bytes(owned, &saved, "discarding swapped language drafts")?;

        require_text(settings, PAUSE_SELECTION, "暂停划词")?;
        let running = control_label(settings, PAUSE_SELECTION)?;
        click(settings, PAUSE_SELECTION)?;
        if control_label(settings, PAUSE_SELECTION)? == running {
            return Err("Pausing automatic selection did not expose a resume action.".into());
        }
        require_settings_bytes(owned, &saved, "temporarily pausing automatic selection")?;
        click(settings, PAUSE_SELECTION)?;
        require_text(settings, PAUSE_SELECTION, &running)?;
        require_settings_bytes(owned, &saved, "resuming automatic selection")?;
        click(settings, PAUSE_SELECTION)?;
        if control_label(settings, PAUSE_SELECTION)? == running {
            return Err("A second pause did not enter the temporary paused state.".into());
        }
        let settings = owned.restart()?;
        show_preferences(settings)?;
        require_text(settings, PAUSE_SELECTION, &running)?;
        require_combo(settings, SOURCE_LANGUAGE, 0)?;
        require_combo(settings, TARGET_LANGUAGE, 3)?;
        require_settings_bytes(owned, &saved, "restarting after a session-only pause")?;
        show_service(settings)?;
        println!("Language swap remained a discardable draft, Auto source disabled swap, and session pause/resume never persisted across restart.");
        Ok(settings)
    }

    fn control_label(parent: HWND, id: i32) -> Result<String, String> {
        if id == KEY {
            return Err("Refusing to expose the API key through a generic label helper.".into());
        }
        let mut buffer = [0_u16; 256];
        let length = send(
            control(parent, id)?,
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
        )?;
        if length >= buffer.len() - 1 {
            return Err("The test action label exceeded its bounded buffer.".into());
        }
        Ok(String::from_utf16_lossy(&buffer[..length]))
    }

    fn saved_settings_bytes(owned: &OwnedApplication) -> Result<Vec<u8>, String> {
        fs::read(owned.directory.join("LingoLeaf").join("settings.json"))
            .map_err(|_| "Cannot read the test application's private settings snapshot.".into())
    }

    fn require_settings_bytes(
        owned: &OwnedApplication,
        expected: &[u8],
        action: &str,
    ) -> Result<(), String> {
        if saved_settings_bytes(owned)? != expected {
            return Err(format!("Settings changed on disk while {action}."));
        }
        Ok(())
    }

    fn check_languages(owned: &mut OwnedApplication, settings: HWND) -> Result<HWND, String> {
        show_preferences(settings)?;
        require_combo(settings, SOURCE_LANGUAGE, 1)?;
        require_combo(settings, TARGET_LANGUAGE, 1)?;
        select_combo(settings, SOURCE_LANGUAGE, 0)?;
        select_combo(settings, TARGET_LANGUAGE, 3)?;
        save_settings(owned, settings)?;
        let saved = saved_settings(owned)?;
        if saved["source_language"] != "auto" || saved["target_language"] != "japanese" {
            return Err("Saving language preferences did not persist Auto -> Japanese.".into());
        }
        // A provider preset changes API fields while preserving the selected
        // language pair, including preferences in the hidden pane.
        show_service(settings)?;
        select_provider(settings, 0)?;
        select_provider(settings, 1)?;
        save_settings(owned, settings)?;
        let preset_saved = saved_settings(owned)?;
        if preset_saved["source_language"] != "auto"
            || preset_saved["target_language"] != "japanese"
        {
            return Err("Switching API presets changed the language preferences.".into());
        }
        let settings = owned.restart()?;
        show_preferences(settings)?;
        require_combo(settings, SOURCE_LANGUAGE, 0)?;
        require_combo(settings, TARGET_LANGUAGE, 3)?;
        show_service(settings)?;
        println!("Legacy English -> Chinese defaults, language saves, API preset preservation and restart restoration passed.");
        Ok(settings)
    }

    fn show_service(settings: HWND) -> Result<(), String> {
        click(settings, SERVICE_TAB)?;
        for id in [
            PROVIDER,
            KEY,
            KEY_VISIBILITY,
            210,
            212,
            213,
            106,
            UPDATE,
            UPDATE_STATUS,
        ] {
            require_visible(settings, id)?;
        }
        for id in [
            SOURCE_LANGUAGE,
            TARGET_LANGUAGE,
            SWAP_LANGUAGES,
            PAUSE_SELECTION,
            SELECTION_MODE,
            COMPATIBILITY_CAPTURE,
            APPEARANCE,
            281,
            282,
            283,
            284,
        ] {
            require_hidden(settings, id)?;
        }
        Ok(())
    }

    fn show_preferences(settings: HWND) -> Result<(), String> {
        click(settings, PREFERENCES_TAB)?;
        for id in [
            SOURCE_LANGUAGE,
            TARGET_LANGUAGE,
            SWAP_LANGUAGES,
            PAUSE_SELECTION,
            SELECTION_MODE,
            COMPATIBILITY_CAPTURE,
            UPDATE,
            UPDATE_STATUS,
        ] {
            require_visible(settings, id)?;
        }
        for id in [
            PROVIDER,
            KEY,
            KEY_VISIBILITY,
            210,
            212,
            213,
            106,
            APPEARANCE,
            281,
            282,
            283,
            284,
        ] {
            require_hidden(settings, id)?;
        }
        Ok(())
    }

    fn show_appearance_page(settings: HWND) -> Result<(), String> {
        click(settings, APPEARANCE_TAB)?;
        for id in [APPEARANCE, 281, 282, 283, 284, UPDATE, UPDATE_STATUS] {
            require_visible(settings, id)?;
        }
        for id in [
            PROVIDER,
            KEY,
            KEY_VISIBILITY,
            210,
            212,
            213,
            106,
            SOURCE_LANGUAGE,
            TARGET_LANGUAGE,
            SWAP_LANGUAGES,
            PAUSE_SELECTION,
            SELECTION_MODE,
            COMPATIBILITY_CAPTURE,
        ] {
            require_hidden(settings, id)?;
        }
        Ok(())
    }

    fn background_color(settings: HWND) -> Result<u32, String> {
        let dc = unsafe { GetDC(settings) };
        if dc.is_null() {
            return Err("Cannot sample settings palette.".into());
        }
        let mut client: RECT = unsafe { zeroed() };
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(settings, &mut client)
        };
        // The rail now has a raised surface; sample the clear main-page gutter.
        let color = unsafe { GetPixel(dc, client.right / 2, 8) };
        unsafe { ReleaseDC(settings, dc) };
        if color == u32::MAX {
            return Err("The settings background pixel is unavailable.".into());
        }
        Ok(color)
    }

    fn colorref(rgb: u32) -> u32 {
        ((rgb >> 16) & 255) | (rgb & 0x00ff00) | ((rgb & 255) << 16)
    }

    fn require_palette(settings: HWND, mode: ThemeMode, preset: ThemePreset) -> Result<(), String> {
        let expected = colorref(Palette::for_theme(mode, preset).background);
        let end = Instant::now() + Duration::from_secs(3);
        while background_color(settings)? != expected {
            if Instant::now() >= end {
                return Err(format!(
                    "The live palette did not update for {preset:?}/{mode:?}."
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    fn check_theme_presets(owned: &mut OwnedApplication, settings: HWND) -> Result<HWND, String> {
        let baseline = saved_settings(owned)?;
        if baseline
            .get("theme_preset")
            .and_then(serde_json::Value::as_str)
            != Some("amber")
        {
            return Err("A legacy profile did not migrate to the warm Amber palette.".into());
        }
        show_service(settings)?;
        // Appearance previews must save only their explicit preference, never
        // the unfinished API form or language drafts on the other pages.
        set_control_text(settings, 210, "http://127.0.0.1:47777/v1")?;
        set_control_text(settings, 212, "discardable-theme-model")?;
        set_control_text(settings, KEY, "fixed-public-theme-draft-not-a-key")?;
        set_control_text(settings, 213, "47")?;
        show_preferences(settings)?;
        select_combo(settings, SOURCE_LANGUAGE, 6)?;
        select_combo(settings, TARGET_LANGUAGE, 0)?;
        show_appearance_page(settings)?;
        let mut observed = Vec::new();
        for (id, preset, name) in PRESET_CARDS {
            click(settings, id)?;
            for (mode, encoded) in [(ThemeMode::Dark, "dark"), (ThemeMode::Light, "light")] {
                let current = saved_settings(owned)?;
                if current
                    .get("theme_mode")
                    .and_then(serde_json::Value::as_str)
                    != Some(encoded)
                {
                    click(settings, THEME)?;
                }
                require_palette(settings, mode, preset)?;
                let mut expected = baseline.clone();
                expected["theme_mode"] = serde_json::Value::String(encoded.into());
                expected["theme_preset"] = serde_json::Value::String(name.into());
                if saved_settings(owned)? != expected {
                    return Err("A theme preview saved unfinished API/language drafts or lost unrelated settings.".into());
                }
                observed.push(background_color(settings)?);
            }
            show_service(settings)?;
            require_text(settings, 210, "http://127.0.0.1:47777/v1")?;
            require_text(settings, 212, "discardable-theme-model")?;
            require_text(settings, KEY, "fixed-public-theme-draft-not-a-key")?;
            require_text(settings, 213, "47")?;
            show_preferences(settings)?;
            require_combo(settings, SOURCE_LANGUAGE, 6)?;
            require_combo(settings, TARGET_LANGUAGE, 0)?;
            show_appearance_page(settings)?;
        }
        observed.sort_unstable();
        observed.dedup();
        if observed.len() != 8 {
            return Err("The four day/night families were not visually distinct.".into());
        }
        click(settings, THEME)?; // Last family is Amber Light: restore baseline Dark.
        require_palette(settings, ThemeMode::Dark, ThemePreset::Amber)?;
        let mut expected = baseline.clone();
        expected["theme_preset"] = serde_json::Value::String("amber".into());
        expected["theme_mode"] = serde_json::Value::String("dark".into());
        if saved_settings(owned)? != expected {
            return Err("Restoring the initial palette changed other preferences.".into());
        }
        send(settings, WM_CLOSE, 0, 0)?;
        let main = owned.wait_for_window(BACKGROUND_TITLE, false)?;
        if unsafe { PostMessageW(main, WM_APP + 4, 0, 0) } == 0 {
            return Err("Cannot reopen the owned settings window.".into());
        }
        let settings = owned.wait_for_settings()?;
        require_text(
            settings,
            210,
            baseline["base_url"]
                .as_str()
                .ok_or("Missing saved base URL.")?,
        )?;
        require_text(
            settings,
            212,
            baseline["model"].as_str().ok_or("Missing saved model.")?,
        )?;
        require_text(settings, KEY, "")?;
        // A real restart additionally verifies the selected family is loaded.
        let settings = owned.restart()?;
        require_palette(settings, ThemeMode::Dark, ThemePreset::Amber)?;
        println!("All eight theme variants painted immediately; theme saves preserved API/language drafts and only persisted appearance, with legacy Amber migration and restart restoration.");
        Ok(settings)
    }

    fn key_input(key: u16) -> Result<(), String> {
        let mut inputs: [INPUT; 2] = unsafe { zeroed() };
        for (index, input) in inputs.iter_mut().enumerate() {
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki.wVk = key;
            input.Anonymous.ki.dwFlags = if index == 0 { 0 } else { KEYEVENTF_KEYUP };
        }
        if unsafe { SendInput(2, inputs.as_ptr(), size_of::<INPUT>() as i32) } != 2 {
            return Err("Cannot inject the owned combo keyboard input.".into());
        }
        Ok(())
    }

    fn check_native_combo_interaction(settings: HWND) -> Result<(), String> {
        let provider = control(settings, PROVIDER)?;
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(provider, &mut rect) } == 0 {
            return Err("Cannot locate the owned provider combo.".into());
        }
        let mut clicks: [INPUT; 2] = unsafe { zeroed() };
        for (index, input) in clicks.iter_mut().enumerate() {
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi.dwFlags = if index == 0 {
                MOUSEEVENTF_LEFTDOWN
            } else {
                MOUSEEVENTF_LEFTUP
            };
        }
        unsafe {
            SetCursorPos(rect.right - 12, (rect.top + rect.bottom) / 2);
        }
        if unsafe { SendInput(2, clicks.as_ptr(), size_of::<INPUT>() as i32) } != 2 {
            return Err("Cannot click the owned combo arrow.".into());
        }
        let end = Instant::now() + Duration::from_secs(2);
        while send(provider, CB_GETDROPPEDSTATE, 0, 0)? == 0 {
            if Instant::now() >= end {
                return Err(
                    "The rounded native combo did not open after a real arrow click.".into(),
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
        key_input(VK_HOME)?;
        key_input(VK_RETURN)?;
        let end = Instant::now() + Duration::from_secs(2);
        while send(provider, CB_GETDROPPEDSTATE, 0, 0)? != 0 {
            if Instant::now() >= end {
                return Err("The native combo did not close with Enter.".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        require_combo(settings, PROVIDER, 0)?;
        require_text(settings, 210, "https://api.deepseek.com/v1")?;
        println!(
            "Rounded provider combo retained a real arrow click and native Home/Enter selection."
        );
        Ok(())
    }

    fn background_lightness(settings: HWND) -> Result<u32, String> {
        let dc = unsafe { GetDC(settings) };
        if dc.is_null() {
            return Err("Cannot sample the visible settings palette.".into());
        }
        let color = unsafe { GetPixel(dc, 8, 8) };
        unsafe { ReleaseDC(settings, dc) };
        if color == u32::MAX {
            return Err("The settings background pixel is unavailable.".into());
        }
        Ok((color & 255) + ((color >> 8) & 255) + ((color >> 16) & 255))
    }

    fn check_theme(owned: &mut OwnedApplication, settings: HWND) -> Result<HWND, String> {
        require_text(settings, THEME, "切换到白天模式")?;
        let dark = background_lightness(settings)?;
        let baseline = saved_settings(owned)?;
        click(settings, THEME)?;
        require_text(settings, THEME, "切换到暗夜模式")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while background_lightness(settings)? < dark + 300 {
            if Instant::now() >= deadline {
                return Err("The day palette was not painted immediately.".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        let mut expected = baseline.clone();
        expected["theme_mode"] = serde_json::Value::String("light".into());
        if saved_settings(owned)? != expected {
            return Err(
                "Changing the theme did not persist it or changed unrelated settings.".into(),
            );
        }
        let settings = owned.restart()?;
        require_text(settings, THEME, "切换到暗夜模式")?;
        if background_lightness(settings)? < dark + 300 {
            return Err("The saved day palette was not restored on automatic startup.".into());
        }
        click(settings, THEME)?;
        require_text(settings, THEME, "切换到白天模式")?;
        if saved_settings(owned)? != baseline {
            return Err("Switching back to night did not preserve the saved settings.".into());
        }
        println!(
            "Immediate day/night painting, persistence and automatic startup restoration passed."
        );
        Ok(settings)
    }

    fn check_icon_appearance(owned: &mut OwnedApplication, settings: HWND) -> Result<(), String> {
        // Save a key-free baseline first. Appearance saves must preserve every
        // unrelated field, including provider, timeout and selection preference.
        show_preferences(settings)?;
        require_hidden(settings, APPEARANCE)?;
        require_visible(settings, SELECTION_MODE)?;
        require_visible(settings, COMPATIBILITY_CAPTURE)?;
        require_combo(settings, SELECTION_MODE, 0)?;
        select_combo(settings, SELECTION_MODE, 1)?;
        save_settings(owned, settings)?;
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
        show_service(settings)?;
        select_provider(settings, 0)?;
        select_provider(settings, 1)?;
        show_preferences(settings)?;
        save_settings(owned, settings)?;
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
        show_preferences(settings)?;
        require_combo(settings, SELECTION_MODE, 1)?;
        // A second click must disable the choice restored from disk. Checking
        // the whole document also protects mode, provider and saved icon values.
        click(settings, COMPATIBILITY_CAPTURE)?;
        save_settings(owned, settings)?;
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
        close_appearance(appearance, settings)?;
        show_preferences(settings)?;
        select_combo(settings, SELECTION_MODE, 0)?;
        save_settings(owned, settings)?;
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
        show_appearance_page(settings)?;
        click(settings, APPEARANCE)?;
        let appearance = owned.wait_for_window(APPEARANCE_TITLE, true)?;
        if unsafe { GetWindow(appearance, GW_OWNER) } != settings
            || unsafe { IsWindowEnabled(settings) } != 0
        {
            return Err("Icon appearance is not modal and owned by settings.".into());
        }
        let mut child: RECT = unsafe { zeroed() };
        let mut parent: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(appearance, &mut child) } == 0
            || unsafe { GetWindowRect(settings, &mut parent) } == 0
            || (i64::from(child.left) + i64::from(child.right)
                - i64::from(parent.left)
                - i64::from(parent.right))
            .abs()
                > 8
            || (i64::from(child.top) + i64::from(child.bottom)
                - i64::from(parent.top)
                - i64::from(parent.bottom))
            .abs()
                > 8
        {
            return Err("Icon appearance was not centered over its settings owner.".into());
        }
        Ok(appearance)
    }

    fn close_appearance(appearance: HWND, settings: HWND) -> Result<(), String> {
        send(appearance, WM_CLOSE, 0, 0)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { IsWindowVisible(appearance) } != 0
            || unsafe { IsWindowEnabled(settings) } == 0
        {
            if Instant::now() >= deadline {
                return Err("Closing icon appearance did not restore its settings owner.".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
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

    fn require_hidden(settings: HWND, id: i32) -> Result<(), String> {
        if unsafe { IsWindowVisible(control(settings, id)?) } != 0 {
            return Err(format!(
                "Control {id} leaked from the inactive settings pane."
            ));
        }
        Ok(())
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
