//! Native Windows message loop. UIA and HTTP never run on this thread.
use super::{icons, mouse, selection, theme};
use crate::{cache::TranslationCache, config, mailbox, translation};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{
        DataExchange::*,
        LibraryLoader::*,
        Memory::*,
        SystemServices::{SS_CENTERIMAGE, SS_ICON, SS_LEFT, SS_REALSIZECONTROL},
        Threading::*,
    },
    UI::{
        Controls::{
            Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW},
            *,
        },
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        Shell::*,
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "LingoLeaf.NativeWindow";
const WM_TRAY: u32 = WM_APP + 1;
const WM_WORK: u32 = WM_APP + 2;
const WM_SELECTION: u32 = WM_APP + 3;
const WM_SETTINGS: u32 = WM_APP + 4;
const HOTKEY: i32 = 1;
const TIMER_SELECTION: usize = 1;
const TIMER_CHIP: usize = 2;
const ID_SETTINGS: u16 = 100;
const ID_CLIPBOARD: u16 = 101;
const ID_EXIT: u16 = 102;
const ID_TRANSLATE: u16 = 103;
const ID_COPY: u16 = 104;
const ID_SAVE: u16 = 105;
const ID_TEST: u16 = 106;
const ID_CANCEL: u16 = 107;
const ID_PROVIDER: u16 = 208;
const ID_AUTO: u16 = 214;
const ID_APPEARANCE: u16 = 108;
const ID_ICON_STYLE: u16 = 230;
const ID_ICON_COLOR: u16 = 231;
const ID_ICON_APPLY: u16 = 232;
const ID_ICON_RESET: u16 = 233;
const APPEARANCE_WIDTH: i32 = 360;
const APPEARANCE_HEIGHT: i32 = 360;
const SETTINGS_WIDTH: i32 = 460;
const SETTINGS_HEIGHT: i32 = 510;
const SETTINGS_CONTENT_HEIGHT: i32 = 468;
const RESULT_WIDTH: i32 = 480;
const RESULT_HEIGHT: i32 = 420;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    // Painting must also work during synchronous callbacks while App is borrowed.
    static THEME: RefCell<Option<theme::Theme>> = const { RefCell::new(None) };
    static FONTS: RefCell<HashMap<(u32, i32, i32), HFONT>> = RefCell::new(HashMap::new());
    static CHIP_WINDOW: Cell<isize> = const { Cell::new(0) };
    static AUTO_CHECKED: Cell<bool> = const { Cell::new(true) };
    static CHIP_ICON: Cell<isize> = const { Cell::new(0) };
    static DRAFT_ICON_COLOR: Cell<u32> = const { Cell::new(0xD6A575) };
}

#[derive(Clone, Copy, PartialEq)]
enum CaptureKind {
    Auto,
    Hotkey,
    Clipboard,
}

struct CaptureJob {
    generation: u64,
    kind: CaptureKind,
    foreground: isize,
    point: POINT,
}
struct TranslationJob {
    generation: u64,
    settings: config::Settings,
    text: String,
    test: bool,
}
enum Event {
    Captured {
        job: CaptureJob,
        result: Result<selection::CapturedText, String>,
    },
    Translated {
        generation: u64,
        result: Result<String, String>,
        test: bool,
        cached: bool,
    },
    Progress {
        generation: u64,
        text: String,
    },
}

struct SettingsUi {
    provider: HWND,
    base: HWND,
    key: HWND,
    model: HWND,
    timeout: HWND,
    auto: HWND,
    status: HWND,
}
struct ResultUi {
    original: HWND,
    translated: HWND,
    status: HWND,
    copy: HWND,
}
struct AppearanceUi {
    style: HWND,
    preview: HWND,
    color: HWND,
    status: HWND,
}

struct App {
    main: HWND,
    settings_window: HWND,
    result_window: HWND,
    chip: HWND,
    appearance_window: HWND,
    appearance_ui: AppearanceUi,
    icons: icons::IconSet,
    preview_icons: Option<icons::IconSet>,
    settings_ui: SettingsUi,
    result_ui: ResultUi,
    settings: config::Settings,
    events: mpsc::Receiver<Event>,
    capture: mpsc::Sender<CaptureJob>,
    network: mailbox::Sender<TranslationJob>,
    generation: u64,
    capture_generation: u64,
    test_generation: u64,
    latest_translation: Arc<AtomicU64>,
    latest_test: Arc<AtomicU64>,
    capture_busy: bool,
    pending: Option<CaptureJob>,
    auto_point: POINT,
    chip_text: String,
    chip_foreground: isize,
    translated: String,
    taskbar_created: u32,
    tray: NOTIFYICONDATAW,
    settings_scroll: i32,
    settings_epoch: u64,
    active_epoch: u64,
    active_text: String,
    active_started: Option<Instant>,
    streaming: bool,
    cache: Arc<Mutex<TranslationCache>>,
}

pub fn run() {
    if let Err(message) = unsafe { run_inner() } {
        unsafe {
            MessageBoxW(
                null_mut(),
                wide(&message).as_ptr(),
                wide("轻译 LingoLeaf").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

unsafe fn run_inner() -> Result<(), String> {
    let mutex = CreateMutexW(null(), 0, wide("Local\\LingoLeaf.App").as_ptr());
    if mutex.is_null() {
        return Err("无法创建应用实例。".into());
    }
    let _mutex = Handle(mutex);
    if GetLastError() == ERROR_ALREADY_EXISTS {
        let existing = FindWindowW(wide(CLASS).as_ptr(), wide("轻译 · 后台").as_ptr());
        if !existing.is_null() {
            PostMessageW(existing, WM_SETTINGS, 0, 0);
        }
        return Ok(());
    }
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let instance = GetModuleHandleW(null());
    // Win32 integer resource ID, not an address that will be dereferenced.
    let mut icon = LoadIconW(instance, std::ptr::without_provenance(1));
    if icon.is_null() {
        icon = LoadIconW(null_mut(), IDI_APPLICATION);
    }
    let palette = theme::Theme::new()?;
    let background = palette.background;
    THEME.with(|cell| *cell.borrow_mut() = Some(palette));
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: icon,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: background,
        ..zeroed()
    };
    // Keep the class-name buffer alive until RegisterClassW has copied it.
    let class_name = wide(CLASS);
    let class = WNDCLASSW {
        lpszClassName: class_name.as_ptr(),
        ..class
    };
    if RegisterClassW(&class) == 0 {
        return Err("无法注册应用窗口。".into());
    }
    let main = create_window("轻译 · 后台", 0, WS_OVERLAPPED, 0, 0, null_mut());
    if main.is_null() {
        return Err("无法创建后台窗口。".into());
    }
    let (settings, load_error) = match config::load() {
        Ok(settings) => (settings, None),
        Err(error) => (config::Settings::default(), Some(error)),
    };
    let runtime_icons =
        icons::IconSet::new(settings.icon_style, settings.icon_color, GetDpiForSystem())?;
    icon = runtime_icons.small();
    let settings_window = create_window(
        "轻译 · 翻译服务设置",
        0,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VSCROLL,
        SETTINGS_WIDTH,
        SETTINGS_HEIGHT,
        null_mut(),
    );
    let result_window = create_window(
        "轻译 · 英文 → 简体中文",
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        WS_OVERLAPPEDWINDOW,
        RESULT_WIDTH,
        RESULT_HEIGHT,
        main,
    );
    let chip = create_window(
        "轻译",
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        WS_POPUP | WS_BORDER,
        52,
        36,
        main,
    );
    let appearance_window = create_window(
        "轻译 · 图标外观",
        WS_EX_TOOLWINDOW,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        APPEARANCE_WIDTH,
        APPEARANCE_HEIGHT,
        main,
    );
    if settings_window.is_null()
        || result_window.is_null()
        || chip.is_null()
        || appearance_window.is_null()
    {
        return Err("无法创建翻译窗口。".into());
    }
    CHIP_WINDOW.set(chip as isize);
    theme::apply_window(settings_window);
    theme::apply_window(result_window);
    theme::apply_window(appearance_window);
    for window in [main, settings_window, result_window, appearance_window] {
        set_window_icons(window, &runtime_icons);
    }
    CHIP_ICON.set(runtime_icons.large() as isize);
    let settings_ui = make_settings(settings_window);
    let result_ui = make_result(result_window);
    let appearance_ui = make_appearance(appearance_window);
    let (events_tx, events) = mpsc::channel();
    let (capture_tx, capture_rx) = mpsc::channel::<CaptureJob>();
    let (network_tx, network_rx) = mailbox::channel::<TranslationJob>();
    let destination = main as isize;
    let capture_events = events_tx.clone();
    thread::Builder::new()
        .name("selection".into())
        .spawn(move || {
            for job in capture_rx {
                let result = if job.kind == CaptureKind::Clipboard {
                    selection::read_clipboard_text()
                } else if GetForegroundWindow() as isize != job.foreground {
                    Err("前台窗口已改变，请重新选择文字。".into())
                } else {
                    selection::capture_selection(job.kind == CaptureKind::Hotkey)
                };
                if capture_events
                    .send(Event::Captured { job, result })
                    .is_err()
                {
                    break;
                }
                PostMessageW(destination as HWND, WM_WORK, 0, 0);
            }
        })
        .map_err(|e| format!("无法启动取词线程：{e}"))?;
    let latest_translation = Arc::new(AtomicU64::new(0));
    let latest_test = Arc::new(AtomicU64::new(0));
    let worker_translation = latest_translation.clone();
    let worker_test = latest_test.clone();
    let shared_cache = Arc::new(Mutex::new(TranslationCache::new()));
    let worker_cache = shared_cache.clone();
    thread::Builder::new()
        .name("translation".into())
        .spawn(move || {
            let mut client = translation::TranslationClient::new();
            while let Some(job) = network_rx.recv() {
                let latest = if job.test {
                    &worker_test
                } else {
                    &worker_translation
                };
                let cancelled = || latest.load(Ordering::Acquire) != job.generation;
                if cancelled() {
                    continue;
                }
                let cached = if job.test {
                    None
                } else {
                    worker_cache
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get(&job.settings, &job.text)
                };
                let was_cached = cached.is_some();
                let mut last_progress = Instant::now()
                    .checked_sub(Duration::from_millis(50))
                    .unwrap_or_else(Instant::now);
                let result = if let Some(text) = cached {
                    Ok(text)
                } else {
                    client.translate_streaming(&job.settings, &job.text, cancelled, |text| {
                        if !job.test
                            && !cancelled()
                            && last_progress.elapsed() >= Duration::from_millis(40)
                        {
                            let _ = events_tx.send(Event::Progress {
                                generation: job.generation,
                                text: text.to_owned(),
                            });
                            PostMessageW(destination as HWND, WM_WORK, 0, 0);
                            last_progress = Instant::now();
                        }
                    })
                };
                if cancelled() {
                    continue;
                }
                if !job.test && !was_cached {
                    if let Ok(text) = &result {
                        worker_cache
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(&job.settings, &job.text, text);
                    }
                }
                if events_tx
                    .send(Event::Translated {
                        generation: job.generation,
                        result,
                        test: job.test,
                        cached: was_cached,
                    })
                    .is_err()
                {
                    break;
                }
                PostMessageW(destination as HWND, WM_WORK, 0, 0);
            }
        })
        .map_err(|e| format!("无法启动翻译线程：{e}"))?;
    let mut tray: NOTIFYICONDATAW = zeroed();
    tray.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    tray.hWnd = main;
    tray.uID = 1;
    tray.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    tray.uCallbackMessage = WM_TRAY;
    tray.hIcon = icon;
    copy_wide(&mut tray.szTip, "轻译 · Ctrl+Alt+T 翻译选区");
    if Shell_NotifyIconW(NIM_ADD, &tray) == 0 {
        return Err("无法创建托盘图标，请重启资源管理器后重试。".into());
    }
    let hotkey_ok = RegisterHotKey(
        main,
        HOTKEY,
        MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
        b'T' as u32,
    ) != 0;
    mouse::set_enabled(settings.selection_button);
    let hook = mouse::install(main as isize, WM_SELECTION);
    let hook_error = hook.as_ref().err().cloned();
    let first_run = !config::has_saved_settings();
    let app = App {
        main,
        settings_window,
        result_window,
        chip,
        appearance_window,
        appearance_ui,
        icons: runtime_icons,
        preview_icons: None,
        settings_ui,
        result_ui,
        settings,
        events,
        capture: capture_tx,
        network: network_tx,
        generation: 0,
        capture_generation: 0,
        test_generation: 0,
        latest_translation,
        latest_test,
        capture_busy: false,
        pending: None,
        auto_point: POINT { x: 0, y: 0 },
        chip_text: String::new(),
        chip_foreground: 0,
        translated: String::new(),
        taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        tray,
        settings_scroll: 0,
        settings_epoch: 0,
        active_epoch: 0,
        active_text: String::new(),
        active_started: None,
        streaming: false,
        cache: shared_cache,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(|app| {
        layout_settings(app);
        layout_result(app);
        layout_appearance(app);
        layout_chip(app);
        fill_settings(app);
        let mut problems = Vec::new();
        if let Some(error) = load_error {
            problems.push(error);
        }
        if !hotkey_ok {
            problems.push("Ctrl+Alt+T 已被其他应用占用；划词按钮与翻译剪贴板仍可用。".into());
        }
        if let Some(error) = hook_error {
            problems.push(error);
        }
        if first_run || !problems.is_empty() {
            show_settings(app);
            if !problems.is_empty() {
                set_text(app.settings_ui.status, &problems.join("\r\n"));
            }
        }
    });
    let mut message: MSG = zeroed();
    loop {
        let status = GetMessageW(&mut message, null_mut(), 0, 0);
        if status <= 0 {
            break;
        }
        if message.message == WM_KEYDOWN && message.wParam == VK_ESCAPE as usize {
            let root = GetAncestor(message.hwnd, GA_ROOT);
            if root == result_window || root == settings_window || root == appearance_window {
                ShowWindow(root, SW_HIDE);
                continue;
            }
        }
        if IsDialogMessageW(settings_window, &message) == 0
            && IsDialogMessageW(result_window, &message) == 0
            && IsDialogMessageW(appearance_window, &message) == 0
        {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        } else if matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN)
            && matches!(
                message.wParam as u16,
                VK_TAB
                    | VK_UP
                    | VK_DOWN
                    | VK_LEFT
                    | VK_RIGHT
                    | VK_HOME
                    | VK_END
                    | VK_PRIOR
                    | VK_NEXT
            )
        {
            with_app(|app| ensure_settings_focus_visible(app));
        }
    }
    drop(hook);
    UnregisterHotKey(main, HOTKEY);
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().take() {
            Shell_NotifyIconW(NIM_DELETE, &app.tray);
            DestroyWindow(app.chip);
            DestroyWindow(app.result_window);
            DestroyWindow(app.settings_window);
            DestroyWindow(app.appearance_window);
            DestroyWindow(app.main);
        }
    });
    FONTS.with(|cache| {
        for font in cache.borrow_mut().drain().map(|(_, font)| font) {
            DeleteObject(font);
        }
    });
    THEME.with(|cell| drop(cell.borrow_mut().take()));
    CHIP_WINDOW.set(0);
    CHIP_ICON.set(0);
    Ok(())
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        if let Ok(mut state) = cell.try_borrow_mut() {
            if let Some(app) = state.as_mut() {
                f(app);
            }
        }
    });
}

fn with_theme<R>(paint: impl FnOnce(&theme::Theme) -> R) -> Option<R> {
    THEME.with(|cell| cell.borrow().as_ref().map(paint))
}

// These synchronous paint messages are intentionally outside App's mutable
// borrow. Native controls can request colors while SetWindowText/layout runs.
unsafe fn paint_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match message {
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            let control = lparam as HWND;
            let id = GetDlgCtrlID(control);
            let surface = message == WM_CTLCOLOREDIT
                || message == WM_CTLCOLORLISTBOX
                || matches!(id, 210..=213 | 222 | 224)
                || id == ID_PROVIDER as i32;
            with_theme(|palette| {
                let dc = wparam as HDC;
                SetBkColor(
                    dc,
                    if surface {
                        theme::SURFACE
                    } else {
                        theme::BACKGROUND
                    },
                );
                SetBkMode(dc, OPAQUE as i32);
                SetTextColor(
                    dc,
                    if surface || id == 200 || id == 220 {
                        theme::TEXT
                    } else {
                        theme::MUTED
                    },
                );
                if surface {
                    palette.surface as isize
                } else {
                    palette.background as isize
                }
            })
        }
        WM_MEASUREITEM if lparam != 0 => {
            let item = &mut *(lparam as *mut MEASUREITEMSTRUCT);
            if item.CtlType == ODT_COMBOBOX {
                item.itemHeight = scaled(26, dpi(hwnd)) as u32;
                Some(1)
            } else {
                None
            }
        }
        WM_DRAWITEM if lparam != 0 => {
            let item = &*(lparam as *const DRAWITEMSTRUCT);
            if item.CtlType != ODT_BUTTON && item.CtlType != ODT_COMBOBOX {
                return None;
            }
            with_theme(|palette| {
                let face = SendMessageW(item.hwndItem, WM_GETFONT, 0, 0) as HFONT;
                if item.CtlType == ODT_COMBOBOX {
                    palette.draw_combo(item, face);
                } else if item.CtlID == ID_AUTO as u32 {
                    palette.draw_checkbox(item, face, AUTO_CHECKED.get());
                } else {
                    palette.draw_button(
                        item,
                        face,
                        item.CtlID == ID_SAVE as u32
                            || item.CtlID == ID_COPY as u32
                            || item.CtlID == ID_ICON_APPLY as u32,
                    );
                }
                1
            })
        }
        WM_ERASEBKGND => with_theme(|palette| {
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            FillRect(wparam as HDC, &rect, palette.background);
            1
        }),
        WM_PAINT if hwnd as isize == CHIP_WINDOW.get() && CHIP_WINDOW.get() != 0 => {
            with_theme(|palette| {
                let mut paint: PAINTSTRUCT = zeroed();
                let dc = BeginPaint(hwnd, &mut paint);
                let mut rect: RECT = zeroed();
                GetClientRect(hwnd, &mut rect);
                FillRect(dc, &rect, palette.background);
                let size = (rect.bottom - scaled(4, dpi(hwnd)))
                    .min(rect.right - scaled(4, dpi(hwnd)))
                    .max(1);
                DrawIconEx(
                    dc,
                    (rect.right - size) / 2,
                    (rect.bottom - size) / 2,
                    CHIP_ICON.get() as HICON,
                    size,
                    size,
                    0,
                    null_mut(),
                    DI_NORMAL,
                );
                EndPaint(hwnd, &paint);
                0
            })
        }
        _ => None,
    }
}

fn provider_index(base_url: &str) -> usize {
    match config::parse_endpoint(base_url)
        .ok()
        .map(|endpoint| endpoint.host)
        .as_deref()
    {
        Some("api.deepseek.com") => 0,
        Some("api.openai.com") => 1,
        _ => 2,
    }
}

unsafe fn change_provider(app: &mut App) {
    let choice = SendMessageW(app.settings_ui.provider, CB_GETCURSEL, 0, 0);
    let preset = match choice {
        0 => config::Settings::deepseek(),
        1 => config::Settings::default(),
        _ => return,
    };
    set_text(app.settings_ui.base, &preset.base_url);
    set_text(app.settings_ui.model, &preset.model);
    set_text(app.settings_ui.timeout, &preset.timeout_seconds.to_string());
    set_text(app.settings_ui.key, "");
    // A result from testing the previous provider must not describe this draft.
    app.test_generation += 1;
    app.latest_test
        .store(app.test_generation, Ordering::Release);
    EnableWindow(GetDlgItem(app.settings_window, ID_TEST as i32), 1);
    set_text(
        app.settings_ui.status,
        "已填入服务地址与模型，请填写对应的 API 密钥。",
    );
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_COMMAND
        && (wparam & 0xffff) as u16 == ID_ICON_COLOR
        && (wparam >> 16) as u16 == BN_CLICKED as u16
    {
        if let Some(color) = pick_icon_color(hwnd, DRAFT_ICON_COLOR.get()) {
            DRAFT_ICON_COLOR.set(color);
            with_app(|app| refresh_icon_preview(app));
        }
        return 0;
    }
    if let Some(result) = paint_message(hwnd, message, wparam, lparam) {
        return result;
    }
    let mut handled = false;
    let mut value = 0;
    with_app(|app| {
        if message == app.taskbar_created && app.taskbar_created != 0 {
            Shell_NotifyIconW(NIM_ADD, &app.tray);
            handled = true;
            return;
        }
        match message {
            WM_SETTINGS => {
                show_settings(app);
                handled = true;
            }
            WM_TRAY => {
                if lparam as u32 == WM_RBUTTONUP || lparam as u32 == WM_CONTEXTMENU {
                    let id = tray_menu(app);
                    if id != 0 {
                        command(app, id);
                    }
                } else if lparam as u32 == WM_LBUTTONDBLCLK {
                    show_settings(app);
                }
                handled = true;
            }
            WM_HOTKEY if wparam == HOTKEY as usize => {
                request_capture(app, CaptureKind::Hotkey, cursor());
                handled = true;
            }
            WM_SELECTION => {
                if lparam == isize::MIN {
                    ShowWindow(app.chip, SW_HIDE);
                    KillTimer(app.main, TIMER_SELECTION);
                    app.capture_generation += 1;
                    app.pending = None;
                    handled = true;
                    return;
                }
                if app.settings.selection_button {
                    app.auto_point = POINT {
                        x: wparam as i32,
                        y: lparam as i32,
                    };
                    ShowWindow(app.chip, SW_HIDE);
                    KillTimer(app.main, TIMER_SELECTION);
                    SetTimer(app.main, TIMER_SELECTION, 150, None);
                }
                handled = true;
            }
            WM_TIMER if wparam == TIMER_SELECTION => {
                KillTimer(app.main, TIMER_SELECTION);
                if app.settings.selection_button {
                    request_capture(app, CaptureKind::Auto, app.auto_point);
                }
                handled = true;
            }
            WM_TIMER if wparam == TIMER_CHIP => {
                KillTimer(app.main, TIMER_CHIP);
                ShowWindow(app.chip, SW_HIDE);
                handled = true;
            }
            WM_WORK => {
                drain_events(app);
                handled = true;
            }
            WM_COMMAND => {
                let id = (wparam & 0xffff) as u16;
                if id == ID_PROVIDER {
                    if (wparam >> 16) as u16 == CBN_SELCHANGE as u16 {
                        change_provider(app);
                    }
                } else if id == ID_ICON_STYLE {
                    if (wparam >> 16) as u16 == CBN_SELCHANGE as u16 {
                        refresh_icon_preview(app);
                    }
                } else if (wparam >> 16) as u16 == BN_CLICKED as u16 {
                    command(app, id);
                }
                handled = true;
            }
            WM_CLOSE => {
                ShowWindow(hwnd, SW_HIDE);
                handled = true;
            }
            WM_SIZE => {
                if hwnd == app.settings_window {
                    layout_settings(app);
                } else if hwnd == app.result_window {
                    layout_result(app);
                } else if hwnd == app.chip {
                    layout_chip(app);
                } else if hwnd == app.appearance_window {
                    layout_appearance(app);
                }
                handled = true;
            }
            WM_VSCROLL if hwnd == app.settings_window => {
                let mut info: SCROLLINFO = zeroed();
                info.cbSize = size_of::<SCROLLINFO>() as u32;
                info.fMask = SIF_ALL;
                GetScrollInfo(hwnd, SB_VERT, &mut info);
                match (wparam & 0xffff) as i32 {
                    SB_LINEUP => app.settings_scroll -= 25,
                    SB_LINEDOWN => app.settings_scroll += 25,
                    SB_PAGEUP => app.settings_scroll -= info.nPage as i32,
                    SB_PAGEDOWN => app.settings_scroll += info.nPage as i32,
                    SB_THUMBPOSITION | SB_THUMBTRACK => app.settings_scroll = info.nTrackPos,
                    SB_TOP => app.settings_scroll = 0,
                    SB_BOTTOM => app.settings_scroll = SETTINGS_CONTENT_HEIGHT,
                    _ => {}
                }
                layout_settings(app);
                handled = true;
            }
            WM_MOUSEWHEEL if hwnd == app.settings_window => {
                app.settings_scroll -= ((wparam >> 16) as u16 as i16 as i32 / 120) * 60;
                layout_settings(app);
                handled = true;
            }
            WM_GETMINMAXINFO if hwnd == app.result_window => {
                let info = &mut *(lparam as *mut MINMAXINFO);
                let dpi = dpi(hwnd);
                info.ptMinTrackSize = POINT {
                    x: scaled(400, dpi),
                    y: scaled(360, dpi),
                };
                handled = true;
            }
            WM_DPICHANGED => {
                let rect = &*(lparam as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if hwnd == app.result_window {
                    layout_result(app);
                } else if hwnd == app.settings_window {
                    layout_settings(app);
                } else if hwnd == app.appearance_window {
                    layout_appearance(app);
                } else {
                    layout_chip(app);
                }
                handled = true;
            }
            WM_MOUSEACTIVATE if hwnd == app.chip => {
                value = MA_NOACTIVATE as isize;
                handled = true;
            }
            WM_LBUTTONUP if hwnd == app.chip => {
                command(app, ID_TRANSLATE);
                handled = true;
            }
            WM_SETCURSOR if hwnd == app.chip => {
                SetCursor(LoadCursorW(null_mut(), IDC_HAND));
                value = 1;
                handled = true;
            }
            _ => {}
        }
    });
    if handled {
        value
    } else {
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

unsafe fn command(app: &mut App, id: u16) {
    match id {
        ID_SETTINGS => show_settings(app),
        ID_APPEARANCE => show_appearance(app),
        ID_ICON_RESET => {
            let defaults = config::Settings::default();
            DRAFT_ICON_COLOR.set(defaults.icon_color);
            SendMessageW(
                app.appearance_ui.style,
                CB_SETCURSEL,
                icon_style_index(defaults.icon_style),
                0,
            );
            refresh_icon_preview(app);
        }
        ID_ICON_APPLY => apply_appearance(app),
        ID_AUTO => {
            AUTO_CHECKED.set(!AUTO_CHECKED.get());
            InvalidateRect(app.settings_ui.auto, null(), 0);
        }
        ID_CLIPBOARD => request_capture(app, CaptureKind::Clipboard, cursor()),
        ID_EXIT => PostQuitMessage(0),
        ID_CANCEL => {
            ShowWindow(app.settings_window, SW_HIDE);
        }
        ID_COPY => match write_clipboard(app.result_window, &app.translated) {
            Ok(()) => set_text(app.result_ui.status, "译文已复制"),
            Err(error) => set_text(app.result_ui.status, &error),
        },
        ID_TRANSLATE => {
            ShowWindow(app.chip, SW_HIDE);
            if GetForegroundWindow() as isize != app.chip_foreground {
                return;
            }
            let text = app.chip_text.clone();
            if !text.is_empty() {
                start_translation(app, text, "划词选区", cursor());
            }
        }
        ID_SAVE => {
            match read_settings(app).and_then(|settings| {
                config::save(&settings)?;
                Ok(settings)
            }) {
                Ok(settings) => {
                    if settings.base_url != app.settings.base_url
                        || settings.model != app.settings.model
                        || settings.api_key != app.settings.api_key
                    {
                        app.settings_epoch += 1;
                        app.generation += 1;
                        app.latest_translation
                            .store(app.generation, Ordering::Release);
                        app.streaming = false;
                    }
                    app.test_generation += 1;
                    app.latest_test
                        .store(app.test_generation, Ordering::Release);
                    EnableWindow(GetDlgItem(app.settings_window, ID_TEST as i32), 1);
                    mouse::set_enabled(settings.selection_button);
                    if !settings.selection_button {
                        ShowWindow(app.chip, SW_HIDE);
                        KillTimer(app.main, TIMER_SELECTION);
                        app.capture_generation += 1;
                        app.pending = None;
                    }
                    app.settings = settings;
                    set_text(
                        app.settings_ui.status,
                        "已保存 · 选中英文后点击“译”，或按 Ctrl+Alt+T。",
                    );
                }
                Err(error) => set_text(app.settings_ui.status, &error),
            }
        }
        ID_TEST => match read_settings(app) {
            Ok(settings) => {
                app.test_generation += 1;
                app.latest_test
                    .store(app.test_generation, Ordering::Release);
                set_text(app.settings_ui.status, "正在测试连接…");
                EnableWindow(GetDlgItem(app.settings_window, ID_TEST as i32), 0);
                if app
                    .network
                    .send(
                        mailbox::Lane::Test,
                        TranslationJob {
                            generation: app.test_generation,
                            settings,
                            text: "Hello, world.".into(),
                            test: true,
                        },
                    )
                    .is_err()
                {
                    EnableWindow(GetDlgItem(app.settings_window, ID_TEST as i32), 1);
                    set_text(app.settings_ui.status, "翻译线程已停止，请重启应用。");
                }
            }
            Err(error) => set_text(app.settings_ui.status, &error),
        },
        _ => {}
    }
}

unsafe fn request_capture(app: &mut App, kind: CaptureKind, point: POINT) {
    app.capture_generation += 1;
    let foreground = GetForegroundWindow();
    if kind != CaptureKind::Clipboard && is_our_window(foreground) {
        return;
    }
    let job = CaptureJob {
        generation: app.capture_generation,
        kind,
        foreground: foreground as isize,
        point,
    };
    if app.capture_busy {
        app.pending = Some(job);
        return;
    }
    app.capture_busy = true;
    if app.capture.send(job).is_err() {
        app.capture_busy = false;
    }
}

unsafe fn drain_events(app: &mut App) {
    while let Ok(event) = app.events.try_recv() {
        match event {
            Event::Captured { job, result } => {
                app.capture_busy = false;
                if job.generation == app.capture_generation
                    && (job.kind == CaptureKind::Clipboard
                        || GetForegroundWindow() as isize == job.foreground)
                {
                    match result {
                        Ok(captured) if !captured.text.trim().is_empty() => {
                            if captured.text.chars().count() > translation::MAX_TEXT_CHARS {
                                if job.kind != CaptureKind::Auto {
                                    show_error(app, "选区过长，请缩小选择范围后重试。", job.point);
                                }
                            } else if job.kind == CaptureKind::Auto && app.settings.selection_button
                            {
                                app.chip_text = captured.text;
                                app.chip_foreground = job.foreground;
                                position_near(app.chip, job.point, 52, 36);
                                layout_chip(app);
                                ShowWindow(app.chip, SW_SHOWNOACTIVATE);
                                SetTimer(app.main, TIMER_CHIP, 7000, None);
                            } else if job.kind != CaptureKind::Auto {
                                start_translation(app, captured.text, captured.source, job.point);
                            }
                        }
                        Ok(_) => {}
                        Err(error) if job.kind != CaptureKind::Auto => {
                            show_error(app, &error, job.point)
                        }
                        Err(_) => {}
                    }
                }
                if let Some(pending) = app.pending.take() {
                    if GetForegroundWindow() as isize == pending.foreground
                        || pending.kind == CaptureKind::Clipboard
                    {
                        app.capture_busy = true;
                        if app.capture.send(pending).is_err() {
                            app.capture_busy = false;
                        }
                    }
                }
            }
            Event::Translated {
                generation,
                result,
                test,
                cached,
            } => {
                if generation
                    != if test {
                        app.test_generation
                    } else {
                        app.generation
                    }
                {
                    continue;
                }
                if test {
                    EnableWindow(GetDlgItem(app.settings_window, ID_TEST as i32), 1);
                    match result {
                        Ok(text) => set_text(
                            app.settings_ui.status,
                            &format!("连接成功：{}", text.chars().take(100).collect::<String>()),
                        ),
                        Err(error) => set_text(app.settings_ui.status, &error),
                    }
                } else {
                    app.streaming = false;
                    match result {
                        Ok(text) => {
                            render_translation(app, &text);
                            let elapsed = app
                                .active_started
                                .take()
                                .map(|start| start.elapsed())
                                .unwrap_or_default();
                            let status = if cached {
                                format!("本地缓存 · {} ms · 未调用 API", elapsed.as_millis())
                            } else {
                                format!("翻译完成 · {:.2} 秒", elapsed.as_secs_f64())
                            };
                            set_text(app.result_ui.status, &status);
                            EnableWindow(app.result_ui.copy, 1);
                        }
                        Err(error) => {
                            if !translation::is_cancelled_error(&error) {
                                set_text(app.result_ui.translated, &error);
                            }
                            set_text(app.result_ui.status, "翻译失败，可在设置中测试接口");
                        }
                    }
                }
            }
            Event::Progress { generation, text } => {
                if generation == app.generation && app.streaming {
                    render_translation(app, &text);
                    set_text(app.result_ui.status, "正在翻译 · 实时显示");
                }
            }
        }
    }
}

unsafe fn start_translation(app: &mut App, text: String, source: &str, point: POINT) {
    if app.streaming && app.active_epoch == app.settings_epoch && app.active_text == text.trim() {
        ShowWindow(app.chip, SW_HIDE);
        ShowWindow(app.result_window, SW_SHOWNOACTIVATE);
        return;
    }
    app.generation += 1;
    app.latest_translation
        .store(app.generation, Ordering::Release);
    ShowWindow(app.chip, SW_HIDE);
    app.translated.clear();
    app.active_text = text.trim().to_owned();
    app.active_epoch = app.settings_epoch;
    app.active_started = Some(Instant::now());
    app.streaming = true;
    EnableWindow(app.result_ui.copy, 0);
    set_text(app.result_ui.original, &text);
    set_text(app.result_ui.translated, "正在翻译…");
    set_text(app.result_ui.status, &format!("{source} · 正在翻译"));
    position_near(app.result_window, point, RESULT_WIDTH, RESULT_HEIGHT);
    layout_result(app);
    ShowWindow(app.result_window, SW_SHOWNOACTIVATE);
    let cached = app
        .cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&app.settings, &text);
    if let Some(cached) = cached {
        app.streaming = false;
        render_translation(app, &cached);
        let elapsed = app
            .active_started
            .take()
            .map(|start| start.elapsed())
            .unwrap_or_default();
        set_text(
            app.result_ui.status,
            &format!("本地缓存 · {} ms · 未调用 API", elapsed.as_millis()),
        );
        EnableWindow(app.result_ui.copy, 1);
        return;
    }
    if app
        .network
        .send(
            mailbox::Lane::Translation,
            TranslationJob {
                generation: app.generation,
                settings: app.settings.clone(),
                text,
                test: false,
            },
        )
        .is_err()
    {
        set_text(app.result_ui.status, "翻译线程已停止，请重启应用。");
    }
}

unsafe fn show_error(app: &mut App, error: &str, point: POINT) {
    app.streaming = false;
    app.generation += 1;
    app.latest_translation
        .store(app.generation, Ordering::Release);
    app.translated.clear();
    EnableWindow(app.result_ui.copy, 0);
    set_text(app.result_ui.original, "未获取到可翻译的选区");
    set_text(app.result_ui.translated, error);
    set_text(
        app.result_ui.status,
        "也可手动复制文字，再右击托盘选择“翻译剪贴板”",
    );
    position_near(app.result_window, point, RESULT_WIDTH, RESULT_HEIGHT);
    layout_result(app);
    ShowWindow(app.result_window, SW_SHOWNOACTIVATE);
}

unsafe fn render_translation(app: &mut App, text: &str) {
    let previous = native_text(&app.translated);
    let current = native_text(text);
    if !previous.is_empty() && current.starts_with(&previous) {
        let suffix = wide(&current[previous.len()..]);
        SendMessageW(
            app.result_ui.translated,
            EM_SETSEL,
            i32::MAX as usize,
            i32::MAX as isize,
        );
        SendMessageW(
            app.result_ui.translated,
            EM_REPLACESEL,
            0,
            suffix.as_ptr() as isize,
        );
        SendMessageW(app.result_ui.translated, EM_SCROLLCARET, 0, 0);
    } else {
        SetWindowTextW(app.result_ui.translated, wide(&current).as_ptr());
    }
    app.translated = text.to_owned();
}

unsafe fn show_settings(app: &mut App) {
    fill_settings(app);
    position_near(
        app.settings_window,
        cursor(),
        SETTINGS_WIDTH,
        SETTINGS_HEIGHT,
    );
    layout_settings(app);
    ShowWindow(app.settings_window, SW_SHOW);
    SetForegroundWindow(app.settings_window);
}

unsafe fn set_window_icons(hwnd: HWND, icon: &icons::IconSet) {
    SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon.small() as isize);
    SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon.large() as isize);
}

fn icon_style_index(style: config::IconStyle) -> usize {
    match style {
        config::IconStyle::Leaf => 0,
        config::IconStyle::Translate => 1,
        config::IconStyle::Letter => 2,
    }
}

unsafe fn selected_icon_style(app: &App) -> config::IconStyle {
    match SendMessageW(app.appearance_ui.style, CB_GETCURSEL, 0, 0) {
        0 => config::IconStyle::Leaf,
        2 => config::IconStyle::Letter,
        _ => config::IconStyle::Translate,
    }
}

unsafe fn show_appearance(app: &mut App) {
    SendMessageW(
        app.appearance_ui.style,
        CB_SETCURSEL,
        icon_style_index(app.settings.icon_style),
        0,
    );
    DRAFT_ICON_COLOR.set(app.settings.icon_color);
    refresh_icon_preview(app);
    position_near(
        app.appearance_window,
        cursor(),
        APPEARANCE_WIDTH,
        APPEARANCE_HEIGHT,
    );
    layout_appearance(app);
    ShowWindow(app.appearance_window, SW_SHOW);
    SetForegroundWindow(app.appearance_window);
}

unsafe fn refresh_icon_preview(app: &mut App) {
    match icons::IconSet::new(
        selected_icon_style(app),
        DRAFT_ICON_COLOR.get(),
        dpi(app.appearance_window) * 2,
    ) {
        Ok(preview) => {
            SendMessageW(
                app.appearance_ui.preview,
                STM_SETICON,
                preview.large() as usize,
                0,
            );
            app.preview_icons = Some(preview);
            set_text(
                app.appearance_ui.color,
                &format!("#{:06X}  选择颜色…", DRAFT_ICON_COLOR.get()),
            );
            set_text(app.appearance_ui.status, "预览已更新，点击应用保存。");
        }
        Err(error) => set_text(app.appearance_ui.status, &error),
    }
}

unsafe fn apply_appearance(app: &mut App) {
    let mut settings = app.settings.clone();
    settings.icon_style = selected_icon_style(app);
    settings.icon_color = DRAFT_ICON_COLOR.get();
    let next =
        match icons::IconSet::new(settings.icon_style, settings.icon_color, GetDpiForSystem()) {
            Ok(icons) => icons,
            Err(error) => {
                set_text(app.appearance_ui.status, &error);
                return;
            }
        };
    if let Err(error) = config::save(&settings) {
        set_text(app.appearance_ui.status, &error);
        return;
    }
    for window in [
        app.main,
        app.settings_window,
        app.result_window,
        app.appearance_window,
    ] {
        set_window_icons(window, &next);
    }
    app.tray.hIcon = next.small();
    let notified = Shell_NotifyIconW(NIM_MODIFY, &app.tray) != 0;
    CHIP_ICON.set(next.large() as isize);
    // All native users have been updated before the previous owned HICON drops.
    app.icons = next;
    app.settings = settings;
    InvalidateRect(app.chip, null(), 0);
    set_text(
        app.appearance_ui.status,
        if notified {
            "已应用并保存，重启后继续使用。"
        } else {
            "已保存，托盘将在资源管理器刷新后更新。"
        },
    );
}

unsafe fn pick_icon_color(owner: HWND, rgb: u32) -> Option<u32> {
    let mut custom = [0u32; 16];
    let mut picker: CHOOSECOLORW = zeroed();
    picker.lStructSize = size_of::<CHOOSECOLORW>() as u32;
    picker.hwndOwner = owner;
    picker.rgbResult = ((rgb >> 16) & 255) | (rgb & 0xFF00) | ((rgb & 255) << 16);
    picker.lpCustColors = custom.as_mut_ptr();
    picker.Flags = CC_FULLOPEN | CC_RGBINIT;
    if ChooseColorW(&mut picker) == 0 {
        return None;
    }
    let bgr = picker.rgbResult;
    Some(((bgr & 255) << 16) | (bgr & 0xFF00) | ((bgr >> 16) & 255))
}

unsafe fn fill_settings(app: &App) {
    set_text(app.settings_ui.base, &app.settings.base_url);
    set_text(app.settings_ui.key, &app.settings.api_key);
    set_text(app.settings_ui.model, &app.settings.model);
    set_text(
        app.settings_ui.timeout,
        &app.settings.timeout_seconds.to_string(),
    );
    AUTO_CHECKED.set(app.settings.selection_button);
    InvalidateRect(app.settings_ui.auto, null(), 0);
    let provider = provider_index(&app.settings.base_url);
    SendMessageW(app.settings_ui.provider, CB_SETCURSEL, provider, 0);
}

unsafe fn read_settings(app: &App) -> Result<config::Settings, String> {
    let settings = config::Settings {
        base_url: text(app.settings_ui.base).trim().into(),
        api_key: text(app.settings_ui.key).trim().into(),
        model: text(app.settings_ui.model).trim().into(),
        timeout_seconds: text(app.settings_ui.timeout)
            .trim()
            .parse()
            .map_err(|_| "超时秒数必须是整数。".to_string())?,
        selection_button: AUTO_CHECKED.get(),
        ..app.settings.clone()
    };
    settings.validate()?;
    Ok(settings)
}

unsafe fn tray_menu(app: &App) -> u16 {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return 0;
    }
    AppendMenuW(
        menu,
        MF_STRING,
        ID_CLIPBOARD as usize,
        wide("翻译剪贴板").as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_SETTINGS as usize,
        wide("翻译服务设置…").as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_APPEARANCE as usize,
        wide("图标外观…").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_EXIT as usize, wide("退出轻译").as_ptr());
    let point = cursor();
    SetForegroundWindow(app.main);
    let chosen = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
        point.x,
        point.y,
        0,
        app.main,
        null(),
    );
    PostMessageW(app.main, WM_NULL, 0, 0);
    DestroyMenu(menu);
    chosen as u16
}

unsafe fn create_window(
    title: &str,
    ex: u32,
    style: u32,
    width: i32,
    height: i32,
    parent: HWND,
) -> HWND {
    let scale = GetDpiForSystem().max(96);
    CreateWindowExW(
        ex,
        wide(CLASS).as_ptr(),
        wide(title).as_ptr(),
        style | WS_CLIPCHILDREN,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        scaled(width, scale),
        scaled(height, scale),
        parent,
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    )
}

unsafe fn child(parent: HWND, class: &str, label: &str, style: u32, id: u16) -> HWND {
    let style = if class == "BUTTON" {
        (style & !0x0f) | BS_OWNERDRAW as u32
    } else {
        style
    };
    let hwnd = CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(label).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | style,
        0,
        0,
        1,
        1,
        parent,
        id as usize as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    if class == "EDIT" {
        SendMessageW(hwnd, EM_SETLIMITTEXT, 32768, 0);
        SendMessageW(
            hwnd,
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            10 | (10 << 16),
        );
        theme::style_edit(hwnd);
    } else if class == "COMBOBOX" {
        theme::style_edit(hwnd);
    }
    hwnd
}

unsafe fn make_settings(parent: HWND) -> SettingsUi {
    child(parent, "STATIC", "轻译", SS_LEFT, 200);
    child(
        parent,
        "STATIC",
        "英文 → 中文    ·    Ctrl + Alt + T",
        SS_LEFT,
        201,
    );
    child(parent, "STATIC", "服务", SS_LEFT, 202);
    let provider = child(
        parent,
        "COMBOBOX",
        "",
        (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32 | WS_VSCROLL | WS_TABSTOP,
        ID_PROVIDER,
    );
    for label in ["DeepSeek", "OpenAI", "自定义兼容服务"] {
        SendMessageW(provider, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
    }
    child(parent, "STATIC", "API 密钥", SS_LEFT, 204);
    let key = child(
        parent,
        "EDIT",
        "",
        (ES_AUTOHSCROLL | ES_PASSWORD) as u32 | WS_TABSTOP,
        211,
    );
    child(parent, "STATIC", "服务地址", SS_LEFT, 203);
    let base = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 210);
    child(parent, "STATIC", "模型", SS_LEFT, 205);
    let model = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 212);
    child(parent, "STATIC", "超时 / 秒", SS_LEFT, 206);
    let timeout = child(
        parent,
        "EDIT",
        "",
        (ES_NUMBER | ES_AUTOHSCROLL) as u32 | WS_TABSTOP,
        213,
    );
    let auto = child(parent, "BUTTON", "划词后显示翻译按钮", WS_TABSTOP, ID_AUTO);
    let status = child(
        parent,
        "STATIC",
        "选择服务，填写密钥后测试连接。",
        SS_LEFT,
        215,
    );
    child(parent, "BUTTON", "测试连接", WS_TABSTOP, ID_TEST);
    child(parent, "BUTTON", "图标", WS_TABSTOP, ID_APPEARANCE);
    child(parent, "BUTTON", "保存", WS_TABSTOP, ID_SAVE);
    SettingsUi {
        provider,
        base,
        key,
        model,
        timeout,
        auto,
        status,
    }
}

unsafe fn make_appearance(parent: HWND) -> AppearanceUi {
    child(parent, "STATIC", "图标外观", SS_LEFT, 240);
    child(parent, "STATIC", "托盘、窗口与划词按钮", SS_LEFT, 241);
    child(parent, "STATIC", "样式", SS_LEFT, 242);
    let style = child(
        parent,
        "COMBOBOX",
        "",
        (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32 | WS_VSCROLL | WS_TABSTOP,
        ID_ICON_STYLE,
    );
    for label in ["叶片", "译字", "字母 A"] {
        SendMessageW(style, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
    }
    let preview = child(
        parent,
        "STATIC",
        "",
        SS_ICON | SS_CENTERIMAGE | SS_REALSIZECONTROL,
        234,
    );
    child(parent, "STATIC", "颜色", SS_LEFT, 243);
    let color = child(parent, "BUTTON", "选择颜色…", WS_TABSTOP, ID_ICON_COLOR);
    let status = child(parent, "STATIC", "选择样式与颜色后应用。", SS_LEFT, 235);
    child(parent, "BUTTON", "恢复默认", WS_TABSTOP, ID_ICON_RESET);
    child(parent, "BUTTON", "应用", WS_TABSTOP, ID_ICON_APPLY);
    AppearanceUi {
        style,
        preview,
        color,
        status,
    }
}

unsafe fn layout_appearance(app: &mut App) {
    let hwnd = app.appearance_window;
    let scale = dpi(hwnd);
    let face = font(scale, 14, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / scale as i32;
    let positions = [
        (240, 22, 16, width - 44, 28),
        (241, 22, 49, width - 44, 22),
        (242, 22, 86, 194, 22),
        (ID_ICON_STYLE, 22, 112, width - 134, 150),
        (234, width - 93, 102, 64, 64),
        (243, 22, 158, width - 44, 22),
        (ID_ICON_COLOR, 22, 184, width - 44, 32),
        (235, 22, 230, width - 44, 36),
        (ID_ICON_RESET, 22, 275, 112, 32),
        (ID_ICON_APPLY, width - 126, 275, 104, 32),
    ];
    for (id, x, y, w, h) in positions {
        let face = if id == 240 {
            font(scale, 21, FW_SEMIBOLD as i32)
        } else {
            face
        };
        place(GetDlgItem(hwnd, id as i32), x, y, w, h, scale, face);
    }
    redraw_layout(hwnd);
}

unsafe fn make_result(parent: HWND) -> ResultUi {
    child(parent, "STATIC", "中文译文", SS_LEFT, 220);
    child(parent, "STATIC", "原文", SS_LEFT, 221);
    let original = child(
        parent,
        "EDIT",
        "",
        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL | WS_TABSTOP,
        222,
    );
    child(parent, "STATIC", "译文", SS_LEFT, 223);
    let translated = child(
        parent,
        "EDIT",
        "",
        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL | WS_TABSTOP,
        224,
    );
    let status = child(parent, "STATIC", "", SS_LEFT, 225);
    let copy = child(
        parent,
        "BUTTON",
        "复制译文",
        BS_PUSHBUTTON as u32 | WS_TABSTOP,
        ID_COPY,
    );
    child(
        parent,
        "BUTTON",
        "设置",
        BS_PUSHBUTTON as u32 | WS_TABSTOP,
        ID_SETTINGS,
    );
    ResultUi {
        original,
        translated,
        status,
        copy,
    }
}

unsafe fn layout_settings(app: &mut App) {
    let hwnd = app.settings_window;
    let dpi = dpi(hwnd);
    let face = font(dpi, 14, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let height = rect.bottom * 96 / dpi as i32;
    app.settings_scroll = app
        .settings_scroll
        .clamp(0, (SETTINGS_CONTENT_HEIGHT - height).max(0));
    let scroll = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: SETTINGS_CONTENT_HEIGHT - 1,
        nPage: height.max(1) as u32,
        nPos: app.settings_scroll,
        nTrackPos: 0,
    };
    // Scrollbar visibility only changes when the available client height changes.
    SetScrollInfo(hwnd, SB_VERT, &scroll, 0);
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / dpi as i32;
    let margin = 24;
    let inner = width - margin * 2;
    let positions = [
        (200, margin, 16, inner, 30),
        (201, margin, 50, inner, 21),
        (202, margin, 88, 52, 22),
        (ID_PROVIDER, margin + 65, 80, inner - 65, 180),
        (204, margin, 124, inner, 21),
        (211, margin, 150, inner, 30),
        (203, margin, 192, inner, 21),
        (210, margin, 218, inner, 30),
        (205, margin, 260, inner - 106, 21),
        (212, margin, 286, inner - 106, 30),
        (206, width - margin - 88, 260, 88, 21),
        (213, width - margin - 88, 286, 88, 30),
        (ID_AUTO, margin, 330, inner, 28),
        (215, margin, 370, inner, 32),
        (ID_TEST, margin, 418, 112, 34),
        (ID_APPEARANCE, margin + 128, 418, 88, 34),
        (ID_SAVE, width - margin - 104, 418, 104, 34),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = if id == 200 {
            font(dpi, 23, FW_SEMIBOLD as i32)
        } else {
            face
        };
        place(
            GetDlgItem(hwnd, id as i32),
            x,
            y - app.settings_scroll,
            w,
            h,
            dpi,
            control_font,
        );
    }
    redraw_layout(hwnd);
}

unsafe fn layout_result(app: &mut App) {
    let hwnd = app.result_window;
    let dpi = dpi(hwnd);
    let face = font(dpi, 14, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / dpi as i32;
    let height = rect.bottom * 96 / dpi as i32;
    let inner = width - 40;
    let positions = [
        (220, 20, 15, inner, 27),
        (221, 20, 52, inner, 20),
        (222, 20, 76, inner, 54),
        (223, 20, 143, inner, 20),
        (224, 20, 167, inner, (height - 251).max(50)),
        (225, 20, height - 72, inner, 24),
        (ID_SETTINGS, 20, height - 41, 76, 30),
        (ID_COPY, width - 132, height - 41, 112, 30),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = if id == 220 {
            font(dpi, 19, FW_SEMIBOLD as i32)
        } else {
            face
        };
        place(GetDlgItem(hwnd, id as i32), x, y, w, h, dpi, control_font);
    }
    redraw_layout(hwnd);
}

unsafe fn layout_chip(app: &mut App) {
    InvalidateRect(app.chip, null(), 0);
}

unsafe fn redraw_layout(hwnd: HWND) {
    // One deferred paint for the complete layout; never force per-child redraws.
    RedrawWindow(
        hwnd,
        null(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
    );
}

unsafe fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, dpi: u32, face: HFONT) {
    MoveWindow(
        hwnd,
        scaled(x, dpi),
        scaled(y, dpi),
        scaled(w, dpi),
        scaled(h, dpi),
        0,
    );
    if SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT != face {
        SendMessageW(hwnd, WM_SETFONT, face as usize, 0);
    }
    if matches!(GetDlgCtrlID(hwnd), 210..=213) {
        let inset = scaled(10, dpi) as isize;
        SendMessageW(
            hwnd,
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            inset | (inset << 16),
        );
    }
}

unsafe fn font(dpi: u32, size: i32, weight: i32) -> HFONT {
    FONTS.with(|cache| {
        *cache
            .borrow_mut()
            .entry((dpi, size, weight))
            .or_insert_with(|| {
                CreateFontW(
                    -scaled(size, dpi),
                    0,
                    0,
                    0,
                    weight,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    CLEARTYPE_QUALITY as u32,
                    DEFAULT_PITCH as u32,
                    wide("Segoe UI").as_ptr(),
                )
            })
    })
}

unsafe fn position_near(hwnd: HWND, point: POINT, width: i32, height: i32) {
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    let w = scaled(width, dpi_x).min(info.rcWork.right - info.rcWork.left);
    let h = scaled(height, dpi_x).min(info.rcWork.bottom - info.rcWork.top);
    let x = (point.x + scaled(14, dpi_x))
        .min(info.rcWork.right - w)
        .max(info.rcWork.left);
    let y = (point.y + scaled(18, dpi_x))
        .min(info.rcWork.bottom - h)
        .max(info.rcWork.top);
    let insert_after = if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST as isize != 0 {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    SetWindowPos(hwnd, insert_after, x, y, w, h, SWP_NOACTIVATE);
}

unsafe fn ensure_settings_focus_visible(app: &mut App) {
    let focus = GetFocus();
    // The top-level window includes a caption above its client area. Treating
    // its negative client-relative top as a clipped child creates a paint loop.
    if focus.is_null() || focus == app.settings_window || IsChild(app.settings_window, focus) == 0 {
        return;
    }
    let mut bounds: RECT = zeroed();
    GetWindowRect(focus, &mut bounds);
    let mut client: RECT = zeroed();
    GetClientRect(app.settings_window, &mut client);
    let mut top = POINT {
        x: bounds.left,
        y: bounds.top,
    };
    let mut bottom = POINT {
        x: bounds.right,
        y: bounds.bottom,
    };
    ScreenToClient(app.settings_window, &mut top);
    ScreenToClient(app.settings_window, &mut bottom);
    let scale = dpi(app.settings_window) as i32;
    let mut next = app.settings_scroll;
    if top.y < 0 {
        next += top.y * 96 / scale - 8;
    } else if bottom.y > client.bottom {
        next += (bottom.y - client.bottom) * 96 / scale + 8;
    } else {
        return;
    }
    let height = client.bottom * 96 / scale;
    next = next.clamp(0, (SETTINGS_CONTENT_HEIGHT - height).max(0));
    if next != app.settings_scroll {
        app.settings_scroll = next;
        layout_settings(app);
    }
}

unsafe fn is_our_window(hwnd: HWND) -> bool {
    let mut process = 0;
    GetWindowThreadProcessId(hwnd, &mut process);
    process == GetCurrentProcessId()
}
unsafe fn cursor() -> POINT {
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    point
}
unsafe fn dpi(hwnd: HWND) -> u32 {
    GetDpiForWindow(hwnd).max(96)
}
fn scaled(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn copy_wide<const N: usize>(buffer: &mut [u16; N], value: &str) {
    for (slot, item) in buffer.iter_mut().take(N - 1).zip(value.encode_utf16()) {
        *slot = item;
    }
}
unsafe fn set_text(hwnd: HWND, value: &str) {
    SetWindowTextW(hwnd, wide(&native_text(value)).as_ptr());
}

fn native_text(value: &str) -> String {
    value.replace('\n', "\r\n").replace("\r\r\n", "\r\n")
}
unsafe fn text(hwnd: HWND) -> String {
    let length = GetWindowTextLengthW(hwnd).clamp(0, 32768) as usize;
    let mut buffer = vec![0; length + 1];
    let actual = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..actual.max(0) as usize])
}

unsafe fn write_clipboard(owner: HWND, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("没有可复制的译文。".into());
    }
    let data = wide(value);
    let memory = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
    if memory.is_null() {
        return Err("无法分配剪贴板内存。".into());
    }
    let destination = GlobalLock(memory) as *mut u16;
    if destination.is_null() {
        GlobalFree(memory);
        return Err("无法写入剪贴板。".into());
    }
    std::ptr::copy_nonoverlapping(data.as_ptr(), destination, data.len());
    GlobalUnlock(memory);
    if OpenClipboard(owner) == 0 {
        GlobalFree(memory);
        return Err("剪贴板正忙，请稍后重试。".into());
    }
    let result = if EmptyClipboard() != 0 && !SetClipboardData(13, memory).is_null() {
        Ok(())
    } else {
        GlobalFree(memory);
        Err("复制失败，请直接选中译文复制。".into())
    };
    CloseClipboard();
    result
}
