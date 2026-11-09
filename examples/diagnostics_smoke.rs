//! Native diagnostics UI regression using an owned EDIT window, real mouse
//! input, an isolated settings directory and a loopback request counter.
//! `cargo run --example diagnostics_smoke -- path/to/lingoleaf.exe`
//! Identify Wine runs as compatibility testing, not native Windows UIA proof.

#[cfg(not(windows))]
fn main() {
    eprintln!("diagnostics_smoke requires Windows or a Windows-compatible runtime.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        eprintln!("Diagnostics smoke check failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows {
    use lingoleaf::config::{self, IconShape, IconStyle, SelectionMode, Settings};
    use std::{
        fs,
        io::Write,
        mem::{size_of, zeroed},
        net::TcpListener,
        path::{Path, PathBuf},
        process::{Child, Command},
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc, Arc,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            ClientToScreen, GetStockObject, UpdateWindow, COLOR_WINDOW, DEFAULT_GUI_FONT,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{AttachThreadInput, GetCurrentThreadId},
        },
        UI::{
            HiDpi::{
                GetDpiForWindow, SetProcessDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Input::KeyboardAndMouse::{
                SendInput, SetFocus, INPUT, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            },
            WindowsAndMessaging::*,
        },
    };

    const CLASS: &str = "LingoLeaf.NativeWindow";
    const BACKGROUND: &str = "轻译 · 后台";
    const DIAGNOSTICS: &str = "轻译 · 划词诊断";
    const SOURCE_TITLE: &str = "diagnostics-title-public-fixture-79";
    const SOURCE_TEXT: &str = "diagnostics-selected-public-fixture-61 warm light calm";
    const CLIPBOARD_SEED: &str = "diagnostics-clipboard-public-fixture-53";
    const MODEL: &str = "diagnostics-model-public-fixture-47";
    const KEY: &str = "diagnostics-fake-key-public-fixture-41";
    const DRAFT: &str = "diagnostics-manual-draft-public-fixture-37";
    const REPORT: i32 = 321;

    pub(super) fn run() -> Result<(), String> {
        checkpoint("Bootstrap");
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let executable = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .ok_or("Pass the application EXE as the first argument.")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(BACKGROUND).as_ptr()) }.is_null() {
            return Err("LingoLeaf is already running; close it before this check.".into());
        }
        let directory = std::env::temp_dir().join(format!(
            "lingoleaf-diagnostics-smoke-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        ));
        fs::create_dir(&directory).map_err(|error| error.to_string())?;
        // Set once before worker threads; only this harness and child inherit it.
        let _isolation = Isolation {
            old: std::env::var_os("LOCALAPPDATA"),
            directory: directory.clone(),
        };
        std::env::set_var("LOCALAPPDATA", &directory);
        let mock = Mock::start()?;
        let base = format!(
            "http://127.0.0.1:{}/v1/diagnostics-base-public-fixture-43",
            mock.port
        );
        config::save(&Settings {
            base_url: base.clone(),
            model: MODEL.into(),
            api_key: KEY.into(),
            selection_button: false,
            compatibility_capture: true,
            selection_mode: SelectionMode::Click,
            icon_size: 48,
            icon_style: IconStyle::Letter,
            icon_shape: IconShape::Square,
            icon_color: 0xC05A96,
            ..Settings::default()
        })?;
        checkpoint("Source.Create");
        let source = Source::start()?;
        checkpoint("Source.ClipboardSeed");
        source.clipboard_seed()?;
        checkpoint("App.Launch");
        let mut app = Application::start(&executable, &directory)?;
        let mut phase = "App.StartWindows";
        let result = (|| -> Result<(), String> {
            checkpoint(phase);
            let background = app.window(BACKGROUND)?;
            let settings = app.window("轻译 · 设置")?;
            wait_for("startup settings", || {
                Ok(unsafe { IsWindowVisible(settings) } != 0)
            })?;
            window_facts("startup settings", settings);
            send(settings, WM_CLOSE, 0, 0)?;
            phase = "Diagnostics.InitialOpen";
            checkpoint(phase);
            let diagnostic = app.window(DIAGNOSTICS)?;
            if unsafe { IsWindowVisible(diagnostic) } != 0 {
                return Err("The diagnostic window opened without an explicit command.".into());
            }
            send(background, WM_COMMAND, 320, 0)?;
            visible(diagnostic)?;
            window_facts("diagnostic window", diagnostic);
            let initial = report(diagnostic)?;
            states(&initial, false, false, false)?;
            privacy(&initial, &base)?;
            if clipboard()? != CLIPBOARD_SEED {
                return Err("Opening diagnostics changed the clipboard.".into());
            }
            phase = "Diagnostics.InitialRefreshCopy";
            checkpoint(phase);
            click(diagnostic, 322)?;
            states(&report(diagnostic)?, false, false, false)?;
            if clipboard()? != CLIPBOARD_SEED {
                return Err("Refreshing diagnostics changed the clipboard.".into());
            }
            copy_report(diagnostic, &base)?;
            phase = "DisplayTest.Disabled";
            checkpoint(phase);
            check_display(&app, diagnostic, &mock)?;
            states(&report(diagnostic)?, false, false, false)?;
            phase = "Resume.Disabled";
            checkpoint(phase);
            // This is the sole action allowed to change disabled automatic state.
            click(diagnostic, 325)?;
            wait_for("first explicit resume hook reinstall", || {
                let value = report(diagnostic)?;
                Ok(value.contains("自动划词：是")
                    && value.contains("监听开关：是")
                    && counter(&value, "HookInstallSuccess")?
                        > counter(&initial, "HookInstallSuccess")?)
            })?;
            if !config::load()?.selection_button {
                return Err("Resume did not persist the automatic switch.".into());
            }

            phase = "Manual.PauseDraft";
            checkpoint(phase);
            send(background, WM_COMMAND, 264, 0)?;
            send(background, WM_COMMAND, 315, 0)?;
            let detail = app.window("轻译 · 翻译与提问")?;
            let draft = wide(DRAFT);
            send(
                unsafe { GetDlgItem(detail, 222) },
                WM_SETTEXT,
                0,
                draft.as_ptr() as isize,
            )?;
            send(background, WM_COMMAND, 320, 0)?;
            phase = "Diagnostics.PausedRefreshCopy";
            checkpoint(phase);
            let paused = report(diagnostic)?;
            states(&paused, true, true, true)?;
            click(diagnostic, 322)?;
            copy_report(diagnostic, &base)?;
            states(&report(diagnostic)?, true, true, true)?;
            phase = "DisplayTest.Paused";
            checkpoint(phase);
            check_display(&app, diagnostic, &mock)?;
            states(&report(diagnostic)?, true, true, true)?;
            if text(detail, 222)? != DRAFT {
                return Err("Diagnostics replaced the manual draft.".into());
            }
            phase = "Resume.PausedManual";
            checkpoint(phase);
            click(diagnostic, 325)?;
            wait_for(
                "explicit resume clearing pause/manual and reinstalling hook",
                || {
                    let value = report(diagnostic)?;
                    Ok(value.contains("本次暂停：否")
                        && value.contains("手动编辑保护：否")
                        && value.contains("监听开关：是")
                        && counter(&value, "HookInstallSuccess")?
                            > counter(&paused, "HookInstallSuccess")?)
                },
            )?;
            phase = "Manual.VerifyPreservedDraft";
            checkpoint(phase);
            send(background, WM_COMMAND, 315, 0)?;
            if text(detail, 222)? != DRAFT {
                return Err("Resume lost the preserved manual draft.".into());
            }
            phase = "Diagnostics.Close";
            checkpoint(phase);
            send(detail, WM_CLOSE, 0, 0)?;
            click(diagnostic, 326)?;
            if unsafe { IsWindowVisible(diagnostic) } != 0 {
                return Err("The Close button did not hide diagnostics.".into());
            }
            let before = report(diagnostic)?;
            phase = "Source.RealDrag";
            checkpoint(phase);
            source.drag()?;
            phase = "Source.CaptureRender";
            checkpoint(phase);
            wait_for("real native drag capture and popup rendering", || {
                let value = report(diagnostic)?;
                Ok(
                    counter(&value, "CaptureQueued")? > counter(&before, "CaptureQueued")?
                        && counter(&value, "CaptureSuccess")? > counter(&before, "CaptureSuccess")?
                        && counter(&value, "RenderSuccess")? > counter(&before, "RenderSuccess")?,
                )
            })?;
            let actual_chip = app.window("轻译")?;
            visible(actual_chip)?;
            require_chip_size(actual_chip, 48)?;
            require_topmost_hit(actual_chip, "captured leaf before diagnostics")?;
            phase = "Diagnostics.FinalPrivacyCopy";
            checkpoint(phase);
            send(background, WM_COMMAND, 320, 0)?;
            require_topmost_hit(actual_chip, "captured leaf after diagnostics")?;
            let after = report(diagnostic)?;
            privacy(&after, &base)?;
            for name in ["HookDown", "HookUp", "ExternalDown", "NotifyPosted"] {
                if counter(&after, name)? <= counter(&before, name)? {
                    return Err(format!("Real native EDIT drag did not increment {name}."));
                }
            }
            if counter(&after, "AppDownAccepted")? <= counter(&before, "AppDownAccepted")?
                || counter(&after, "AppUpAccepted")? <= counter(&before, "AppUpAccepted")?
            {
                return Err("Real hook notifications were not accepted by the controller.".into());
            }
            if counter(&after, "CaptureQueued")? <= counter(&before, "CaptureQueued")? {
                return Err("Real accepted EDIT drag did not reach capture.".into());
            }
            if counter(&after, "HookRebind")? == 0 {
                return Err("Explicit Resume did not record a hook rebind.".into());
            }
            copy_report(diagnostic, &base)?;
            mock.require_zero()?;
            println!("Native diagnostics UI passed: explicit open/refresh/copy/close, fixed 32px display fixture and leaf click without capture or HTTP, disabled/pause/manual state preservation, explicit Resume enabling automatic mode, preserving draft and reinstalling the hook.");
            println!("Observed real SendInput EDIT drag through hook, controller, successful native capture and popup rendering with restored custom appearance; no private selection candidate messages were fabricated. This does not establish another application's UIA support. The actual stage counters follow.");
            println!("{after}");
            phase = "App.Shutdown";
            checkpoint(phase);
            app.stop()?;
            Ok(())
        })();
        if let Err(error) = &result {
            // This runs before Application::drop, so a failed native input
            // assertion retains its fixed report rather than losing the app.
            eprintln!("Diagnostics failed during phase {phase}: {error}");
            source.facts("source at failure");
            app.failure_report(&base);
        }
        result
    }

    fn check_display(app: &Application, diagnostic: HWND, mock: &Mock) -> Result<(), String> {
        checkpoint("DisplayTest.Show");
        let before = report(diagnostic)?;
        click(diagnostic, 324)?;
        let chip = app.window("轻译")?;
        visible(chip)?;
        checkpoint("DisplayTest.Visible");
        window_facts("display test leaf", chip);
        require_chip_size(chip, 32)?;
        require_topmost_hit(chip, "display test leaf")?;
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(chip, &mut rect) } == 0 {
            return Err("Cannot locate the display-test leaf.".into());
        }
        // Genuine input activates the displayed leaf. It must never translate.
        if unsafe { SetCursorPos((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2) } == 0
        {
            return Err("Cannot point at the display-test leaf.".into());
        }
        checkpoint("DisplayTest.RealClick");
        let mut point: POINT = unsafe { zeroed() };
        unsafe { GetCursorPos(&mut point) };
        let hit = unsafe { WindowFromPoint(point) };
        println!(
            "Display click facts: cursor={},{} hit={hit:?} expected_leaf={chip:?}",
            point.x, point.y
        );
        mouse(MOUSEEVENTF_LEFTDOWN)?;
        mouse(MOUSEEVENTF_LEFTUP)?;
        window_facts("display test leaf after click", chip);
        wait_for("the test leaf click to dismiss it", || {
            Ok(unsafe { IsWindowVisible(chip) } == 0)
        })?;
        checkpoint("DisplayTest.Dismissed");
        thread::sleep(Duration::from_millis(250));
        let after = report(diagnostic)?;
        for name in [
            "CaptureQueued",
            "CaptureSuccess",
            "CaptureEmpty",
            "CaptureFailed",
        ] {
            if counter(&after, name)? != counter(&before, name)? {
                return Err(format!("Display test unexpectedly reached {name}."));
            }
        }
        if !after.contains("SelfTest: Succeeded") {
            return Err("The display test did not record its result.".into());
        }
        let settings = config::load()?;
        if settings.icon_size != 48
            || settings.icon_style != IconStyle::Letter
            || settings.icon_shape != IconShape::Square
            || settings.icon_color != 0xC05A96
        {
            return Err("The display test changed the saved custom appearance.".into());
        }
        mock.require_zero()
    }

    fn require_chip_size(chip: HWND, logical_size: u32) -> Result<(), String> {
        let scale = unsafe { GetDpiForWindow(chip) }.clamp(96, 384);
        let expected_size = (logical_size * scale + 48) as i32 / 96;
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(chip, &mut rect) } == 0
            || rect.right - rect.left != expected_size
            || rect.bottom - rect.top != expected_size
        {
            return Err(format!(
                "The popup size does not match {logical_size} logical pixels: {},{}.",
                rect.right - rect.left,
                rect.bottom - rect.top
            ));
        }
        Ok(())
    }

    fn require_topmost_hit(chip: HWND, label: &str) -> Result<(), String> {
        let exstyle = unsafe { GetWindowLongPtrW(chip, GWL_EXSTYLE) };
        let topmost = exstyle & WS_EX_TOPMOST as isize != 0;
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(chip, &mut rect) } == 0 {
            return Err("Cannot inspect the leaf's native bounds.".into());
        }
        let center = POINT {
            x: (rect.left + rect.right) / 2,
            y: (rect.top + rect.bottom) / 2,
        };
        let hit = unsafe { WindowFromPoint(center) };
        println!("Leaf z-order facts {label}: exstyle={exstyle:#x} topmost={topmost} center={},{} hit={hit:?} expected_leaf={chip:?}", center.x, center.y);
        if !topmost {
            return Err(format!("The {label} lost WS_EX_TOPMOST."));
        }
        if hit != chip {
            return Err(format!(
                "The {label} center is occluded by another native window."
            ));
        }
        Ok(())
    }

    fn states(report: &str, auto: bool, paused: bool, manual: bool) -> Result<(), String> {
        for (label, enabled) in [
            ("自动划词", auto),
            ("本次暂停", paused),
            ("手动编辑保护", manual),
        ] {
            let expected = format!("{label}：{}", if enabled { "是" } else { "否" });
            if !report.contains(&expected) {
                return Err(format!("Diagnostic state missing {expected}."));
            }
        }
        Ok(())
    }
    fn privacy(report: &str, base: &str) -> Result<(), String> {
        for value in [
            base,
            MODEL,
            KEY,
            SOURCE_TITLE,
            SOURCE_TEXT,
            CLIPBOARD_SEED,
            DRAFT,
            "public-fixture",
        ] {
            if report.contains(value) {
                return Err("The diagnostic report exposed a private fixture value.".into());
            }
        }
        Ok(())
    }
    fn copy_report(window: HWND, base: &str) -> Result<(), String> {
        click(window, 323)?;
        let copied = clipboard()?;
        privacy(&copied, base)?;
        if !copied.contains("selection diagnostics") || !copied.contains("HookDown:") {
            return Err("Explicit Copy did not put the diagnostic report on the clipboard.".into());
        }
        Ok(())
    }
    fn clipboard() -> Result<String, String> {
        Ok(lingoleaf::platform::selection::read_clipboard_text()?.text)
    }
    fn counter(report: &str, name: &str) -> Result<u64, String> {
        report
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
            .ok_or_else(|| format!("Missing diagnostic counter {name}."))?
            .trim()
            .parse()
            .map_err(|_| format!("Invalid diagnostic counter {name}."))
    }
    fn report(window: HWND) -> Result<String, String> {
        click(window, 322)?;
        text(window, REPORT)
    }
    fn click(parent: HWND, id: i32) -> Result<(), String> {
        let control = unsafe { GetDlgItem(parent, id) };
        if control.is_null() {
            return Err(format!("Missing native control {id}."));
        }
        send(control, BM_CLICK, 0, 0).map(|_| ())
    }
    fn text(parent: HWND, id: i32) -> Result<String, String> {
        let control = unsafe { GetDlgItem(parent, id) };
        let mut buffer = vec![0_u16; 65536];
        let len = send(
            control,
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
        )?;
        if len >= buffer.len() {
            return Err("Report exceeded the bounded test buffer.".into());
        }
        Ok(String::from_utf16_lossy(&buffer[..len]))
    }
    fn visible(window: HWND) -> Result<(), String> {
        wait_for("owned visible window", || {
            Ok(unsafe { IsWindowVisible(window) } != 0)
        })
    }
    fn send(window: HWND, message: u32, wparam: usize, lparam: isize) -> Result<usize, String> {
        let mut result = 0;
        if window.is_null()
            || unsafe {
                SendMessageTimeoutW(
                    window,
                    message,
                    wparam,
                    lparam,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    3000,
                    &mut result,
                )
            } == 0
        {
            return Err("An owned native control did not respond within three seconds.".into());
        }
        Ok(result)
    }
    fn mouse(flags: u32) -> Result<(), String> {
        let mut input: INPUT = unsafe { zeroed() };
        input.r#type = INPUT_MOUSE;
        input.Anonymous.mi.dwFlags = flags;
        let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
        if sent != 1 {
            return Err(format!(
                "Cannot inject the owned mouse input: sent={sent} system_error={}",
                unsafe { windows_sys::Win32::Foundation::GetLastError() }
            ));
        }
        thread::sleep(Duration::from_millis(30));
        Ok(())
    }
    fn wait_for(
        label: &str,
        mut check: impl FnMut() -> Result<bool, String>,
    ) -> Result<(), String> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if check()? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Err(format!("Timed out waiting for {label}."))
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn checkpoint(phase: &str) {
        println!("Diagnostics phase: {phase}");
    }

    fn window_facts(label: &str, window: HWND) {
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(window, &mut pid) };
        let foreground = unsafe { GetForegroundWindow() };
        let mut foreground_pid = 0;
        unsafe { GetWindowThreadProcessId(foreground, &mut foreground_pid) };
        let mut rect: RECT = unsafe { zeroed() };
        let rect_ok = unsafe { GetWindowRect(window, &mut rect) };
        let mut info: GUITHREADINFO = unsafe { zeroed() };
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        let focus_ok = unsafe { GetGUIThreadInfo(thread, &mut info) };
        let exstyle = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
        let topmost = exstyle & WS_EX_TOPMOST as isize != 0;
        println!("Native facts {label}: hwnd={window:?} pid={pid} thread={thread} visible={} enabled={} foreground={foreground:?} foreground_pid={foreground_pid} rect_ok={rect_ok} rect={},{},{},{} dpi={} exstyle={exstyle:#x} topmost={topmost} focus_ok={focus_ok} focus={:?}", unsafe { IsWindowVisible(window) }, unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(window) }, rect.left, rect.top, rect.right, rect.bottom, unsafe { GetDpiForWindow(window) }, info.hwndFocus);
    }

    struct Isolation {
        old: Option<std::ffi::OsString>,
        directory: PathBuf,
    }
    impl Drop for Isolation {
        fn drop(&mut self) {
            if let Some(old) = &self.old {
                std::env::set_var("LOCALAPPDATA", old);
            } else {
                std::env::remove_var("LOCALAPPDATA");
            }
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
    struct Application {
        child: Child,
        stopped: bool,
    }
    impl Application {
        fn start(executable: &Path, directory: &Path) -> Result<Self, String> {
            Command::new(executable)
                .env("LOCALAPPDATA", directory)
                .spawn()
                .map(|child| Self {
                    child,
                    stopped: false,
                })
                .map_err(|e| e.to_string())
        }
        fn window(&self, title: &str) -> Result<HWND, String> {
            let mut window = null_mut();
            wait_for("owned application window", || {
                window = unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(title).as_ptr()) };
                if window.is_null() {
                    return Ok(false);
                }
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(window, &mut pid) };
                if pid != self.child.id() {
                    return Err("Refusing to inspect an unowned application window.".into());
                }
                Ok(true)
            })?;
            Ok(window)
        }
        fn failure_report(&mut self, base: &str) {
            eprintln!(
                "Owned application status before cleanup: {:?}",
                self.child.try_wait()
            );
            for (label, title) in [
                ("background", BACKGROUND),
                ("settings", "轻译 · 设置"),
                ("diagnostics", DIAGNOSTICS),
                ("leaf", "轻译"),
            ] {
                let window = unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(title).as_ptr()) };
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(window, &mut pid) };
                if window.is_null() || pid != self.child.id() {
                    eprintln!("Owned window {label}: unavailable");
                    continue;
                }
                window_facts(label, window);
                if title == DIAGNOSTICS {
                    match report(window) {
                        Ok(value) if privacy(&value, base).is_ok() => {
                            eprintln!("Diagnostic report before cleanup:\n{value}")
                        }
                        Ok(_) => {
                            eprintln!("Diagnostic report withheld: privacy fixture check failed.")
                        }
                        Err(error) => {
                            eprintln!("Cannot read diagnostic report before cleanup: {error}")
                        }
                    }
                }
            }
        }
        fn stop(&mut self) -> Result<(), String> {
            if self.stopped {
                return Ok(());
            }
            let main = self.window(BACKGROUND)?;
            send(main, WM_COMMAND, 102, 0)?;
            wait_for("owned application exit", || {
                if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                    self.stopped = true;
                    if status.success() {
                        Ok(true)
                    } else {
                        Err(format!("Owned application exited with {status}."))
                    }
                } else {
                    Ok(false)
                }
            })
        }
    }
    impl Drop for Application {
        fn drop(&mut self) {
            if !self.stopped {
                let _ = self.stop();
            }
            if !self.stopped {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    struct Source {
        window: usize,
        edit: usize,
        thread: u32,
        worker: Option<JoinHandle<()>>,
    }
    impl Source {
        fn facts(&self, label: &str) {
            window_facts(label, self.window as HWND);
            window_facts("owned EDIT", self.edit as HWND);
        }
        fn start() -> Result<Self, String> {
            let (sender, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || unsafe {
                let result = create_source();
                match result {
                    Ok((window, edit)) => {
                        let _ =
                            sender.send(Ok((window as usize, edit as usize, GetCurrentThreadId())));
                        let mut message: MSG = zeroed();
                        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                    }
                }
            });
            let (window, edit, thread) = receiver
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| "Owned source did not start.")??;
            Ok(Self {
                window,
                edit,
                thread,
                worker: Some(worker),
            })
        }
        fn focus(&self) -> Result<(), String> {
            let mut queue: MSG = unsafe { zeroed() };
            unsafe { PeekMessageW(&mut queue, null_mut(), 0, 0, PM_NOREMOVE) };
            let caller = unsafe { GetCurrentThreadId() };
            wait_for("owned native source focus", || {
                unsafe {
                    AttachThreadInput(caller, self.thread, 1);
                    ShowWindow(self.window as HWND, SW_RESTORE);
                    SetForegroundWindow(self.window as HWND);
                    SetFocus(self.edit as HWND);
                    AttachThreadInput(caller, self.thread, 0);
                }
                let mut info: GUITHREADINFO = unsafe { zeroed() };
                info.cbSize = size_of::<GUITHREADINFO>() as u32;
                Ok(unsafe { GetForegroundWindow() } == self.window as HWND
                    && unsafe { GetGUIThreadInfo(self.thread, &mut info) } != 0
                    && info.hwndFocus == self.edit as HWND)
            })
        }
        fn clipboard_seed(&self) -> Result<(), String> {
            let value = wide(CLIPBOARD_SEED);
            send(self.edit as HWND, WM_SETTEXT, 0, value.as_ptr() as isize)?;
            send(self.edit as HWND, 0xB1, 0, -1)?;
            send(self.edit as HWND, WM_COPY, 0, 0)?;
            wait_for(
                "owned clipboard seed",
                || Ok(clipboard()? == CLIPBOARD_SEED),
            )?;
            let value = wide(SOURCE_TEXT);
            send(self.edit as HWND, WM_SETTEXT, 0, value.as_ptr() as isize)?;
            Ok(())
        }
        fn drag(&self) -> Result<(), String> {
            checkpoint("Source.FocusStart");
            self.facts("source before focus");
            self.focus()?;
            checkpoint("Source.Focused");
            self.facts("source after focus");
            let point = |offset: usize| -> Result<POINT, String> {
                let value = send(self.edit as HWND, 0xD6, offset, 0)?;
                let mut point = POINT {
                    x: (value as u16 as i16) as i32 + 1,
                    y: ((value >> 16) as u16 as i16) as i32 + 8,
                };
                if unsafe { ClientToScreen(self.edit as HWND, &mut point) } == 0 {
                    return Err("Cannot map owned EDIT characters.".into());
                }
                Ok(point)
            };
            let start = point(0)?;
            let end = point(38)?;
            let root = unsafe { GetAncestor(WindowFromPoint(start), GA_ROOT) };
            println!(
                "Native drag facts: start={},{} end={},{} hit_root={root:?} expected_root={:?}",
                start.x, start.y, end.x, end.y, self.window as HWND
            );
            if root != self.window as HWND {
                return Err("The real gesture source is occluded.".into());
            }
            unsafe { SetCursorPos(start.x, start.y) };
            mouse(MOUSEEVENTF_LEFTDOWN)?;
            for step in 1..=12 {
                unsafe { SetCursorPos(start.x + (end.x - start.x) * step / 12, start.y) };
                thread::sleep(Duration::from_millis(25));
            }
            mouse(MOUSEEVENTF_LEFTUP)?;
            checkpoint("Source.RealInputReleased");
            self.facts("source after drag");
            let mut first = 0u32;
            let mut last = 0u32;
            send(
                self.edit as HWND,
                0xB0,
                &mut first as *mut _ as usize,
                &mut last as *mut _ as isize,
            )?;
            if first == last {
                return Err("Real SendInput drag did not select native EDIT text.".into());
            }
            println!(
                "Native drag selected range length={}",
                last.saturating_sub(first)
            );
            Ok(())
        }
    }
    impl Drop for Source {
        fn drop(&mut self) {
            unsafe { PostMessageW(self.window as HWND, WM_CLOSE, 0, 0) };
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    unsafe fn create_source() -> Result<(HWND, HWND), String> {
        let class = wide("LingoLeaf.DiagnosticsSmokeSource");
        let instance = GetModuleHandleW(null());
        let definition = WNDCLASSW {
            lpfnWndProc: Some(source_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as _,
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&definition) == 0 {
            return Err("Cannot register owned source class.".into());
        }
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            wide(SOURCE_TITLE).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            60,
            60,
            760,
            280,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        let edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide(SOURCE_TEXT).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_BORDER | ES_MULTILINE as u32,
            24,
            30,
            690,
            150,
            window,
            101usize as HMENU,
            instance,
            null(),
        );
        if window.is_null() || edit.is_null() {
            return Err("Cannot create owned native EDIT source.".into());
        }
        SendMessageW(
            edit,
            WM_SETFONT,
            GetStockObject(DEFAULT_GUI_FONT) as usize,
            1,
        );
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);
        Ok((window, edit))
    }
    unsafe extern "system" fn source_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_DESTROY {
            PostQuitMessage(0);
            0
        } else {
            DefWindowProcW(window, message, wparam, lparam)
        }
    }

    struct Mock {
        port: u16,
        count: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }
    impl Mock {
        fn start() -> Result<Self, String> {
            let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            let count = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let state = (count.clone(), stop.clone());
            let worker = thread::spawn(move || {
                while !state.1.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            state.0.fetch_add(1, Ordering::Relaxed);
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
                            let _=stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            });
            Ok(Self {
                port,
                count,
                stop,
                worker: Some(worker),
            })
        }
        fn require_zero(&self) -> Result<(), String> {
            if self.count.load(Ordering::Relaxed) == 0 {
                Ok(())
            } else {
                Err("Diagnostics sent an unexpected HTTP request to the loopback fixture.".into())
            }
        }
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
