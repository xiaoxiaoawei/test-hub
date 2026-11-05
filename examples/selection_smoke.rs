//! Controller regression using a local SSE service and an owned native EDIT window.
//! `cargo run --example selection_smoke -- path/to/lingoleaf.exe`
//! Candidate messages exercise the selection controller; this does not replace
//! a separate test of actual mouse hooks or Windows hardware compatibility.

#[cfg(not(windows))]
fn main() {
    eprintln!("selection_smoke requires Windows or a Windows-compatible runtime.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        eprintln!("Selection smoke check failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows {
    use lingoleaf::config::{self, SelectionMode, Settings};
    use std::{
        fs,
        io::{Read, Write},
        mem::{size_of, zeroed},
        net::{TcpListener, TcpStream},
        path::{Path, PathBuf},
        process::{Child, Command},
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex,
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
        UI::{Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
    };

    const CLASS: &str = "LingoLeaf.NativeWindow";
    const BACKGROUND: &str = "轻译 · 后台";
    const SETTINGS: &str = "轻译 · 翻译服务设置";
    const INLINE: &str = "轻译 · 划词译文";
    const EM_GETSEL: u32 = 0xB0;
    const EM_SETSEL: u32 = 0xB1;
    const EM_POSFROMCHAR: u32 = 0xD6;
    const SELECTION: u32 = WM_APP + 3;
    const SOURCE_TEXT: &str = "warm light.\r\nwarm light.";

    pub(super) fn run() -> Result<(), String> {
        let executable = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .ok_or("Pass the application EXE as the first argument.")?
            .canonicalize()
            .map_err(|error| format!("Cannot locate the application EXE: {error}"))?;
        if !unsafe { FindWindowW(wide(CLASS).as_ptr(), null()) }.is_null() {
            return Err("LingoLeaf is already running; close it before this check.".into());
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "lingoleaf-selection-smoke-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).map_err(|error| error.to_string())?;
        // Only this example process and its child inherit the isolated location.
        // Configure it before starting the example's worker threads.
        let _isolation = Isolation {
            old: std::env::var_os("LOCALAPPDATA"),
            directory: directory.clone(),
        };
        std::env::set_var("LOCALAPPDATA", &directory);
        let mock = Mock::start()?;
        config::save(&Settings {
            base_url: format!("http://127.0.0.1:{}/v1", mock.port),
            model: "selection-smoke-model".into(),
            selection_mode: SelectionMode::Direct,
            ..Settings::default()
        })?;
        let source = Source::start()?;
        let mut app = Application::start(executable, &directory)?;
        let background = app.window(BACKGROUND)?;
        // Settings is shown automatically after App state is installed. Do not
        // post a second Open command: it could reopen settings after WM_CLOSE
        // and take foreground focus from the owned source during the gesture.
        let settings = app.window(SETTINGS)?;
        wait_for("the startup settings barrier", || {
            Ok(unsafe { IsWindowVisible(settings) } != 0)
        })?;
        send(settings, WM_CLOSE, 0, 0)?;

        source.gesture(background, false, 0, 4)?;
        let inline = app.window(INLINE)?;
        let first_selection = wait_for("the first streamed selection", || {
            Ok(text(inline, 251)? == "旧")
        });
        if let Err(error) = first_selection {
            return Err(format!(
                "{error}; {}; {}; inline_visible={}, inline_text={:?}, inline_status={:?}",
                source.diagnostics(),
                mock.diagnostics(),
                unsafe { IsWindowVisible(inline) },
                text(inline, 251)?,
                text(inline, 254)?,
            ));
        }
        let first = bounds(inline)?;
        require_visible(inline)?;

        source.gesture(background, false, 5, 10)?;
        require_same_inline(&app, inline)?;
        require_visible(inline)?;
        require_pending(inline)?;
        mock.finish_warm.store(true, Ordering::Release);
        // Warm deliberately sends another fragment after light has been selected.
        // No subsequent snapshot may display that superseded translation.
        wait_for("the second streamed selection", || {
            let translated = text(inline, 251)?;
            if translated.contains('旧') {
                return Err(
                    "A late fragment from the previous selection replaced the new selection."
                        .into(),
                );
            }
            Ok(translated == "新")
        })?;
        if bounds(inline)? == first {
            return Err("The compact window did not follow the new selection position.".into());
        }

        // Select the same word on the next line while its HTTP stream is active.
        let before_move = bounds(inline)?;
        source.gesture(background, false, 18, 23)?;
        require_same_inline(&app, inline)?;
        require_visible(inline)?;
        thread::sleep(Duration::from_millis(200));
        mock.finish_light.store(true, Ordering::Release);
        wait_for("the reused in-flight translation", || {
            let translated = text(inline, 251)?;
            if translated.contains('旧') {
                return Err("A superseded translation became visible again.".into());
            }
            Ok(translated == "新译文" && text(inline, 254)?.contains("完成"))
        })?;
        if bounds(inline)? == before_move {
            return Err(
                "Repeating a word at a different position did not move the compact window.".into(),
            );
        }
        mock.require_requests(&["warm", "light"])?;

        source.gesture(background, false, 5, 10)?;
        wait_for("the cached translation", || {
            Ok(text(inline, 251)? == "新译文" && text(inline, 254)?.contains("缓存"))
        })?;
        require_same_inline(&app, inline)?;
        mock.require_requests(&["warm", "light"])?;

        // Both an empty selection and a password selection must remain local.
        source.gesture(background, false, 4, 4)?;
        thread::sleep(Duration::from_millis(2400));
        mock.require_requests(&["warm", "light"])?;
        source.gesture(background, true, 0, 5)?;
        thread::sleep(Duration::from_millis(2400));
        mock.require_requests(&["warm", "light"])?;
        app.stop()?;
        println!("Selection controller passed: one compact HWND, updated positions, stale-progress isolation, in-flight/cache deduplication, and no requests for empty/password selections.");
        println!("Two local mock POSTs; no real API credential or remote service was used. Mouse-hook behavior requires a separate actual-gesture check.");
        Ok(())
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
        fn start(executable: PathBuf, directory: &Path) -> Result<Self, String> {
            Command::new(executable)
                .env("LOCALAPPDATA", directory)
                .spawn()
                .map(|child| Self {
                    child,
                    stopped: false,
                })
                .map_err(|error| format!("Cannot start the isolated application: {error}"))
        }
        fn window(&self, title: &str) -> Result<HWND, String> {
            let mut found = null_mut();
            wait_for("an owned application window", || {
                let window = unsafe { FindWindowW(wide(CLASS).as_ptr(), wide(title).as_ptr()) };
                if window.is_null() {
                    return Ok(false);
                }
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(window, &mut pid) };
                if pid != self.child.id() {
                    return Err(
                        "Refusing to inspect a window outside the owned test process.".into(),
                    );
                }
                found = window;
                Ok(true)
            })?;
            Ok(found)
        }
        fn stop(&mut self) -> Result<(), String> {
            if self.stopped {
                return Ok(());
            }
            let main = self.window(BACKGROUND)?;
            unsafe { PostMessageW(main, WM_COMMAND, 102, 0) };
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                    self.stopped = true;
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(format!("The test application exited with {status}."))
                    };
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err("The owned application did not exit through its tray Exit command.".into())
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
        password: usize,
        thread: u32,
        worker: Option<JoinHandle<()>>,
    }
    impl Source {
        fn start() -> Result<Self, String> {
            let (sender, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || unsafe {
                let result = create_source();
                if let Ok((window, edit, password)) = result {
                    let _ = sender.send(Ok((
                        window as usize,
                        edit as usize,
                        password as usize,
                        GetCurrentThreadId(),
                    )));
                    let mut message: MSG = zeroed();
                    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                } else {
                    let _ = sender.send(Err(
                        "Cannot create the native selection test window.".to_owned()
                    ));
                }
            });
            let (window, edit, password, thread) =
                receiver
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| "The native selection test window did not start.".to_owned())??;
            Ok(Self {
                window,
                edit,
                password,
                thread,
                worker: Some(worker),
            })
        }
        fn gesture(
            &self,
            app: HWND,
            password: bool,
            start: usize,
            end: usize,
        ) -> Result<(), String> {
            let edit = if password { self.password } else { self.edit } as HWND;
            let mut queue: MSG = unsafe { zeroed() };
            unsafe { PeekMessageW(&mut queue, null_mut(), 0, 0, PM_NOREMOVE) };
            let caller = unsafe { GetCurrentThreadId() };
            if unsafe { AttachThreadInput(caller, self.thread, 1) } == 0 {
                return Err("Cannot attach to the owned source input queue.".into());
            }
            unsafe {
                SetForegroundWindow(self.window as HWND);
                SetFocus(edit);
                AttachThreadInput(caller, self.thread, 0);
            }
            if unsafe { GetForegroundWindow() } != self.window as HWND {
                return Err("Cannot foreground the owned selection source.".into());
            }
            send(edit, EM_SETSEL, start, end as isize)?;
            let mut selected_start = 0_u32;
            let mut selected_end = 0_u32;
            send(
                edit,
                EM_GETSEL,
                &mut selected_start as *mut _ as usize,
                &mut selected_end as *mut _ as isize,
            )?;
            if selected_start as usize != start || selected_end as usize != end {
                return Err("The owned native EDIT did not select the requested range.".into());
            }
            let down = character(edit, start)?;
            let up = character(edit, if end > start { end - 1 } else { end })?;
            // Mirror the documented controller notification wire format. Real
            // mouse input is deliberately tested separately by the host runner.
            let packed_down = u64::from(down.x as u32) | (u64::from(down.y as u32) << 32);
            send(app, SELECTION, packed_down as usize, isize::MIN)?;
            let packed_up = u64::from(up.x as u32) | (u64::from(end > start) << 32);
            send(app, SELECTION, packed_up as usize, up.y as isize)?;
            Ok(())
        }
        fn diagnostics(&self) -> String {
            let foreground = unsafe { GetForegroundWindow() };
            let mut info: GUITHREADINFO = unsafe { zeroed() };
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            unsafe { GetGUIThreadInfo(self.thread, &mut info) };
            let points = [0, 3].map(|offset| {
                character(self.edit as HWND, offset)
                    .map(|point| {
                        let hit = unsafe { WindowFromPoint(point) };
                        let root = unsafe { GetAncestor(hit, GA_ROOT) };
                        format!("({},{}) hit={hit:?} root={root:?}", point.x, point.y)
                    })
                    .unwrap_or_else(|error| error)
            });
            format!(
                "source={:?}, edit={:?}, foreground={foreground:?}, focus={:?}, down={}, up={}",
                self.window as HWND, self.edit as HWND, info.hwndFocus, points[0], points[1]
            )
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
    unsafe fn create_source() -> Result<(HWND, HWND, HWND), ()> {
        let name = wide("LingoLeaf.SelectionSmokeSource");
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(source_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as _,
            lpszClassName: name.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return Err(());
        }
        let window = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("LingoLeaf selection smoke source").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            60,
            60,
            730,
            360,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if window.is_null() {
            return Err(());
        }
        let style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER;
        let edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide(SOURCE_TEXT).as_ptr(),
            style | ES_MULTILINE as u32,
            24,
            30,
            650,
            150,
            window,
            101_usize as HMENU,
            instance,
            null(),
        );
        let password = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide("fixed-public-password-placeholder").as_ptr(),
            style | ES_PASSWORD as u32,
            24,
            220,
            500,
            30,
            window,
            102_usize as HMENU,
            instance,
            null(),
        );
        if edit.is_null() || password.is_null() {
            DestroyWindow(window);
            return Err(());
        }
        let font = GetStockObject(DEFAULT_GUI_FONT);
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        SendMessageW(password, WM_SETFONT, font as usize, 1);
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);
        Ok((window, edit, password))
    }
    unsafe extern "system" fn source_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_DESTROY {
            PostQuitMessage(0);
            0
        } else {
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }
    fn character(edit: HWND, offset: usize) -> Result<POINT, String> {
        let packed = send(edit, EM_POSFROMCHAR, offset, 0)? as isize;
        if packed == -1 {
            return Err("Cannot locate the owned native selection on screen.".into());
        }
        let mut point = POINT {
            x: (packed as u16 as i16) as i32 + 1,
            y: ((packed as u32 >> 16) as u16 as i16) as i32 + 7,
        };
        if unsafe { ClientToScreen(edit, &mut point) } == 0 {
            return Err("Cannot map the selection to the screen.".into());
        }
        Ok(point)
    }

    struct Mock {
        port: u16,
        requests: Arc<Mutex<Vec<String>>>,
        error: Arc<Mutex<Option<String>>>,
        finish_warm: Arc<AtomicBool>,
        finish_light: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }
    impl Mock {
        fn start() -> Result<Self, String> {
            let listener =
                TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
            let port = listener
                .local_addr()
                .map_err(|error| error.to_string())?
                .port();
            listener
                .set_nonblocking(true)
                .map_err(|error| error.to_string())?;
            let requests = Arc::new(Mutex::new(Vec::new()));
            let error = Arc::new(Mutex::new(None));
            let finish_warm = Arc::new(AtomicBool::new(false));
            let finish_light = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let state = (
                requests.clone(),
                error.clone(),
                finish_warm.clone(),
                finish_light.clone(),
                stop.clone(),
            );
            let worker = thread::spawn(move || {
                let (requests, error, finish_warm, finish_light, stop) = state;
                let mut handlers = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let state = (
                                requests.clone(),
                                error.clone(),
                                finish_warm.clone(),
                                finish_light.clone(),
                                stop.clone(),
                            );
                            handlers.push(thread::spawn(move || {
                                if let Err(problem) =
                                    serve(stream, &state.0, &state.2, &state.3, &state.4)
                                {
                                    *state.1.lock().unwrap_or_else(|poison| poison.into_inner()) =
                                        Some(problem);
                                }
                            }));
                        }
                        Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
                for handler in handlers {
                    let _ = handler.join();
                }
            });
            Ok(Self {
                port,
                requests,
                error,
                finish_warm,
                finish_light,
                stop,
                worker: Some(worker),
            })
        }
        fn require_requests(&self, expected: &[&str]) -> Result<(), String> {
            if self
                .error
                .lock()
                .map_err(|_| "Mock error lock failed.")?
                .is_some()
            {
                return Err("The local mock received an invalid request or could not complete its response.".into());
            }
            let requests = self
                .requests
                .lock()
                .map_err(|_| "Mock request lock failed.")?;
            if requests.iter().map(String::as_str).collect::<Vec<_>>() != expected {
                return Err(format!(
                    "Unexpected local POST count: expected {}, received {}.",
                    expected.len(),
                    requests.len()
                ));
            }
            Ok(())
        }
        fn diagnostics(&self) -> String {
            let requests = self
                .requests
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let error = self
                .error
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            format!("local_POSTs={requests:?}, mock_error={error:?}")
        }
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            self.finish_warm.store(true, Ordering::Release);
            self.finish_light.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    fn serve(
        mut stream: TcpStream,
        requests: &Mutex<Vec<String>>,
        warm: &AtomicBool,
        light: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| "Cannot set the mock read timeout.")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| "Cannot set the mock write timeout.")?;
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        let (header_end, length) = loop {
            let received = stream
                .read(&mut chunk)
                .map_err(|_| "Cannot read the local mock request.")?;
            if received == 0 || bytes.len() + received > 16_384 {
                return Err("Invalid local mock request size.".into());
            }
            bytes.extend_from_slice(&chunk[..received]);
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let header = std::str::from_utf8(&bytes[..index])
                    .map_err(|_| "Invalid local request headers.")?;
                if !header.starts_with("POST /v1/chat/completions ")
                    || header.to_ascii_lowercase().contains("authorization:")
                {
                    return Err("Unexpected local request headers.".into());
                }
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>())
                    })
                    .ok_or("Missing local request length.")?
                    .map_err(|_| "Invalid local request length.")?;
                break (index + 4, length);
            }
        };
        while bytes.len() < header_end + length {
            let received = stream
                .read(&mut chunk)
                .map_err(|_| "Cannot read the local request body.")?;
            if received == 0 || bytes.len() + received > 16_384 {
                return Err("Incomplete local request body.".into());
            }
            bytes.extend_from_slice(&chunk[..received]);
        }
        let body: serde_json::Value =
            serde_json::from_slice(&bytes[header_end..header_end + length])
                .map_err(|_| "Invalid local request JSON.")?;
        let source = body
            .pointer("/messages/1/content")
            .and_then(serde_json::Value::as_str)
            .ok_or("Missing local selection text.")?;
        if body["model"] != "selection-smoke-model"
            || body["stream"] != true
            || !matches!(source, "warm" | "light")
        {
            return Err("Unexpected local selection request.".into());
        }
        requests
            .lock()
            .map_err(|_| "Mock request lock failed.")?
            .push(source.to_owned());
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").map_err(|_| "Cannot write local response headers.")?;
        let (first, last, gate) = if source == "warm" {
            ("旧", "译文", warm)
        } else {
            ("新", "译文", light)
        };
        let first = format!(
            "data: {}\n\n",
            serde_json::json!({"choices":[{"delta":{"content":first}}]})
        );
        // A comment supplies the next HTTP chunk header on Wine/WinHTTP too.
        if write_chunk(&mut stream, first.as_bytes())
            .and_then(|_| write_chunk(&mut stream, b": heartbeat\n\n"))
            .is_err()
        {
            return Ok(());
        }
        let started = Instant::now();
        while !gate.load(Ordering::Acquire)
            && !stop.load(Ordering::Acquire)
            && started.elapsed() < Duration::from_secs(12)
        {
            thread::sleep(Duration::from_millis(10));
        }
        let tail = format!("data: {}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n", serde_json::json!({"choices":[{"delta":{"content":last}}]}));
        // Disconnects are expected when the controller cancels an old generation.
        let _ =
            write_chunk(&mut stream, tail.as_bytes()).and_then(|_| stream.write_all(b"0\r\n\r\n"));
        Ok(())
    }
    fn write_chunk(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
        write!(stream, "{:x}\r\n", bytes.len())?;
        stream.write_all(bytes)?;
        stream.write_all(b"\r\n")?;
        stream.flush()
    }

    fn require_pending(inline: HWND) -> Result<(), String> {
        if text(inline, 251)? != "正在读取选区…" || text(inline, 254)? != "正在读取新选区"
        {
            return Err(
                "A new selection did not replace the previous text with pending feedback.".into(),
            );
        }
        Ok(())
    }
    fn require_same_inline(app: &Application, expected: HWND) -> Result<(), String> {
        if app.window(INLINE)? != expected {
            return Err("A new compact window replaced the existing HWND.".into());
        }
        Ok(())
    }
    fn require_visible(window: HWND) -> Result<(), String> {
        if unsafe { IsWindowVisible(window) } == 0 {
            return Err("The compact window disappeared during reselection.".into());
        }
        Ok(())
    }
    fn bounds(window: HWND) -> Result<(i32, i32, i32, i32), String> {
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(window, &mut rect) } == 0 {
            return Err("Cannot inspect the compact window position.".into());
        }
        Ok((rect.left, rect.top, rect.right, rect.bottom))
    }
    fn text(parent: HWND, id: i32) -> Result<String, String> {
        let child = unsafe { GetDlgItem(parent, id) };
        if child.is_null() {
            return Err("The compact window is missing a required control.".into());
        }
        let mut buffer = vec![0_u16; 4096];
        let count = send(
            child,
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
        )?;
        if count >= buffer.len() {
            return Err("The compact text did not fit the bounded test buffer.".into());
        }
        Ok(String::from_utf16_lossy(&buffer[..count]))
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
                3000,
                &mut result,
            )
        } == 0
        {
            return Err("An owned test window did not respond within three seconds.".into());
        }
        Ok(result)
    }
    fn wait_for(
        label: &str,
        mut condition: impl FnMut() -> Result<bool, String>,
    ) -> Result<(), String> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if condition()? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(format!("Timed out waiting for {label}."))
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
