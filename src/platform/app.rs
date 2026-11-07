//! Native Windows message loop. UIA and HTTP never run on this thread.
use super::{
    clipboard_copy, icons, mouse, picker, popup, rich_text, selection, selection_events, speech,
    surfaces, symbols, theme, update_install,
};
use crate::{cache::TranslationCache, config, mailbox, translation, update};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
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

#[path = "app/conversation_ui.rs"]
mod conversation_ui;
#[path = "app/layout_ui.rs"]
mod layout_ui;
#[path = "app/manual_ui.rs"]
mod manual_ui;
#[path = "app/speech_ui.rs"]
mod speech_ui;
use layout_ui::*;

#[path = "app/update_ui.rs"]
mod update_ui;
use update_ui::{check_updates, download_available_update, enqueue_notice, handle_notice};

const CLASS: &str = "LingoLeaf.NativeWindow";
const WM_TRAY: u32 = WM_APP + 1;
const WM_WORK: u32 = WM_APP + 2;
const WM_SELECTION: u32 = WM_APP + 3;
const WM_SETTINGS: u32 = WM_APP + 4;
const WM_ACCESSIBILITY: u32 = WM_APP + 5;
const WM_NOTICE: u32 = WM_APP + 6;
const HOTKEY: i32 = 1;
const HOTKEY_MANUAL: i32 = 2;
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
const ID_COMPATIBILITY: u16 = 217;
const ID_APPEARANCE: u16 = 108;
const ID_THEME: u16 = 109;
const ID_UPDATE: u16 = 110;
const ID_ICON_STYLE: u16 = 230;
const ID_ICON_COLOR: u16 = 231;
const ID_ICON_APPLY: u16 = 232;
const ID_ICON_RESET: u16 = 233;
const ID_ICON_SHAPE: u16 = 236;
const ID_ICON_SIZE: u16 = 237;
const ID_SCREEN_PICK: u16 = 238;
const ID_SELECTION_MODE: u16 = 216;
const ID_INLINE_EXPAND: u16 = 252;
const ID_INLINE_CLOSE: u16 = 253;
const ID_QUESTION: u16 = 260;
const ID_SEND: u16 = 261;
const ID_RETRY: u16 = 262;
const ID_PIN: u16 = 263;
const ID_PAUSE: u16 = 264;
const ID_SWAP_LANGUAGES: u16 = 265;
const ID_COPY_CONVERSATION: u16 = 266;
const ID_KEY_VISIBILITY: u16 = 267;
const ID_COPY_SELECTION: u16 = 268;
const ID_SERVICE_TAB: u16 = 270;
const ID_PREFERENCES_TAB: u16 = 271;
const ID_SOURCE_LANGUAGE: u16 = 272;
const ID_TARGET_LANGUAGE: u16 = 273;
const ID_APPEARANCE_TAB: u16 = 280;
const ID_PRESET_OCEAN: u16 = 281;
const ID_PRESET_AURORA: u16 = 282;
const ID_PRESET_NEBULA: u16 = 283;
const ID_PRESET_AMBER: u16 = 284;
const ID_SPEAK: u16 = 285;
const ID_POLISH: u16 = 286;
const ID_COPY_MARKDOWN: u16 = 287;
const ID_MANUAL_HOTKEY: u16 = 301;
const ID_TRANSLATE_INPUT: u16 = 302;
const ID_SPEECH_SETTINGS: u16 = 303;
const ID_SPEECH_APPLY: u16 = 308;
const ID_SPEECH_CLOSE: u16 = 309;
const ID_MANUAL_WINDOW: u16 = 315;
const SPEECH_WIDTH: i32 = 480;
const SPEECH_HEIGHT: i32 = 424;
const APPEARANCE_WIDTH: i32 = 440;
const APPEARANCE_HEIGHT: i32 = 380;
const INLINE_WIDTH: i32 = 460;
const INLINE_HEIGHT: i32 = 356;
const SETTINGS_WIDTH: i32 = 760;
const SETTINGS_HEIGHT: i32 = 680;
const SETTINGS_CONTENT_HEIGHT: i32 = 600;
const RESULT_WIDTH: i32 = 600;
const RESULT_HEIGHT: i32 = 620;

#[derive(Clone, Copy, PartialEq)]
enum SettingsPage {
    Service,
    Preferences,
    Appearance,
}

thread_local! {
    static TRANSLATION_STREAMING: Cell<bool> = const { Cell::new(false) };
    static SPEECH_BUSY: Cell<bool> = const { Cell::new(false) };
    static CONVERSATION_BUSY: Cell<bool> = const { Cell::new(false) };
    static WINDOW_PINNED: Cell<bool> = const { Cell::new(false) };
    static KEY_VISIBLE: Cell<bool> = const { Cell::new(false) };
    static SETTINGS_PAGE: Cell<SettingsPage> = const { Cell::new(SettingsPage::Service) };
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    // Painting must also work during synchronous callbacks while App is borrowed.
    static THEME: RefCell<Option<theme::Theme>> = const { RefCell::new(None) };
    static FONTS: RefCell<HashMap<(u32, i32, i32), HFONT>> = RefCell::new(HashMap::new());
    static CURRENT_PRESET: Cell<config::ThemePreset> = const { Cell::new(config::ThemePreset::Amber) };
    static SETTINGS_WINDOW: Cell<isize> = const { Cell::new(0) };
    static APPEARANCE_WINDOW: Cell<isize> = const { Cell::new(0) };
    static CHIP_WINDOW: Cell<isize> = const { Cell::new(0) };
    static AUTO_CHECKED: Cell<bool> = const { Cell::new(true) };
    static COMPATIBILITY_CHECKED: Cell<bool> = const { Cell::new(false) };
    static CURRENT_THEME: Cell<config::ThemeMode> = const { Cell::new(config::ThemeMode::Dark) };
    static NOTICES: RefCell<VecDeque<Notice>> = const { RefCell::new(VecDeque::new()) };
    static NOTICE_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static DRAFT_ICON_COLOR: Cell<u32> = const { Cell::new(0x19A97C) };
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
    origin: Option<POINT>,
    compatibility: bool,
    copy_attempted: bool,
    compatibility_checked: bool,
    copy_retryable: bool,
    clipboard_sequence: u32,
}
enum CaptureWork {
    Warm(isize),
    Capture(CaptureJob),
}
struct TranslationJob {
    generation: u64,
    settings: config::Settings,
    text: String,
    test: bool,
    force_refresh: bool,
}
enum Event {
    Conversation(conversation_ui::Event),
    Speech(speech::Event),
    Warmed,
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
    PickedColor(Result<Option<u32>, String>),
    UpdateChecked {
        manual: bool,
        result: Result<update::CheckResult, String>,
    },
    UpdateProgress {
        received: u64,
        total: u64,
    },
    UpdateDownloaded(Result<update::VerifiedDownload, String>),
    UpdateLaunched(Result<(), String>),
}

enum Notice {
    Saved,
    Information(String),
    Error(String),
    UpdateAvailable,
    RestartUpdate,
}

struct SettingsUi {
    source: HWND,
    target: HWND,
    provider: HWND,
    base: HWND,
    key: HWND,
    model: HWND,
    timeout: HWND,
    auto: HWND,
    compatibility: HWND,
    mode: HWND,
    status: HWND,
    update_status: HWND,
}
struct ResultUi {
    action: HWND,
    pin: HWND,
    question: HWND,
    send: HWND,
    original: HWND,
    translated: HWND,
    status: HWND,
    copy: HWND,
}
struct AppearanceUi {
    style: HWND,
    shape: HWND,
    size: HWND,
    preview: HWND,
    color: HWND,
    status: HWND,
}
struct InlineUi {
    action: HWND,
    pin: HWND,
    question: HWND,
    send: HWND,
    translated: HWND,
    status: HWND,
    copy: HWND,
}

type WindowIconKey = (u32, config::IconStyle, u32, config::IconShape, u32);

struct App {
    main: HWND,
    settings_window: HWND,
    result_window: HWND,
    chip: HWND,
    inline_window: HWND,
    inline_ui: InlineUi,
    inline_active: bool,
    inline_positioned: bool,
    result_positioned: bool,
    manual_mode: bool,
    manual_draft: String,
    manual_hotkey_registered: bool,
    window_pinned: bool,
    selection_paused: bool,
    appearance_window: HWND,
    appearance_ui: AppearanceUi,
    speech_window: HWND,
    speech_ui: SpeechSettingsUi,
    speech_restore_view: Option<bool>,
    speech_settings_was_visible: bool,
    icons: icons::IconSet,
    preview_icons: Option<icons::IconSet>,
    window_icons: HashMap<isize, (WindowIconKey, icons::IconSet)>,
    settings_ui: SettingsUi,
    result_ui: ResultUi,
    settings: config::Settings,
    events: mpsc::Receiver<Event>,
    ui_events: mpsc::Sender<Event>,
    capture: mpsc::Sender<CaptureWork>,
    network: mailbox::Sender<TranslationJob>,
    generation: u64,
    capture_generation: u64,
    latest_capture: Arc<AtomicU64>,
    test_generation: u64,
    latest_translation: Arc<AtomicU64>,
    latest_test: Arc<AtomicU64>,
    capture_busy: bool,
    warm_busy: bool,
    warmed_foreground: isize,
    pending: Option<CaptureJob>,
    auto_point: POINT,
    auto_foreground: isize,
    auto_source_root: isize,
    auto_origin: Option<POINT>,
    auto_pressed: bool,
    auto_strong: bool,
    auto_started: Option<Instant>,
    auto_attempt: usize,
    auto_text: Option<String>,
    pending_icon_click: bool,
    compatibility_attempted: bool,
    chip_deadline: Option<Instant>,
    chip_pixels: Option<icons::IconPixels>,
    chip_dpi: u32,
    picking_color: bool,
    chip_text: String,
    chip_foreground: isize,
    conversation: conversation_ui::State,
    speech: speech::Speech,
    speech_token: u64,
    speaking: bool,
    translation_ready: bool,
    translated: String,
    rendered_text: String,
    display_pending: bool,
    taskbar_created: u32,
    tray: NOTIFYICONDATAW,
    settings_scroll: i32,
    settings_epoch: u64,
    active_epoch: u64,
    active_text: String,
    active_started: Option<Instant>,
    streaming: bool,
    cache: Arc<Mutex<TranslationCache>>,
    update_busy: bool,
    available_update: Option<update::Release>,
    downloaded_update: Option<update::VerifiedDownload>,
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
            let mut process = 0;
            GetWindowThreadProcessId(existing, &mut process);
            AllowSetForegroundWindow(process);
            PostMessageW(existing, WM_SETTINGS, 0, 0);
        }
        return Ok(());
    }
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let (settings, load_error) = match config::load() {
        Ok(settings) => (settings, None),
        Err(error) => (config::Settings::default(), Some(error)),
    };
    CURRENT_THEME.set(settings.theme_mode);
    CURRENT_PRESET.set(settings.theme_preset);
    rich_text::load()?;
    let instance = GetModuleHandleW(null());
    // Win32 integer resource ID, not an address that will be dereferenced.
    let mut icon = LoadIconW(instance, std::ptr::without_provenance(1));
    if icon.is_null() {
        icon = LoadIconW(null_mut(), IDI_APPLICATION);
    }
    let palette = theme::Theme::new(settings.theme_mode, settings.theme_preset)?;
    surfaces::set_palette(&palette);
    THEME.with(|cell| *cell.borrow_mut() = Some(palette));
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: icon,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        // Brushes belong to the replaceable Theme, not the window class.
        hbrBackground: null_mut(),
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
    let runtime_icons = icons::IconSet::new(
        settings.icon_style,
        settings.icon_color,
        settings.icon_shape,
        settings.icon_size,
        GetDpiForSystem(),
    )?;
    icon = runtime_icons.small();
    let settings_window = create_window(
        "轻译 · 设置",
        0,
        WS_OVERLAPPED
            | WS_CAPTION
            | WS_SYSMENU
            | WS_MINIMIZEBOX
            | WS_MAXIMIZEBOX
            | WS_THICKFRAME
            | WS_VSCROLL,
        SETTINGS_WIDTH,
        SETTINGS_HEIGHT,
        null_mut(),
    );
    let result_window = create_window(
        "轻译 · 翻译与提问",
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        WS_OVERLAPPEDWINDOW,
        RESULT_WIDTH,
        RESULT_HEIGHT,
        main,
    );
    let chip = create_window(
        "轻译",
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        WS_POPUP,
        settings.icon_size as i32,
        settings.icon_size as i32,
        main,
    );
    let inline_window = create_window(
        "轻译 · 划词译文",
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        WS_POPUP,
        INLINE_WIDTH,
        INLINE_HEIGHT,
        main,
    );
    let appearance_window = create_window(
        "轻译 · 划词图标",
        WS_EX_TOOLWINDOW,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        APPEARANCE_WIDTH,
        APPEARANCE_HEIGHT,
        settings_window,
    );
    let speech_window = create_window(
        "轻译 · 朗读接口",
        WS_EX_TOOLWINDOW,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        SPEECH_WIDTH,
        SPEECH_HEIGHT,
        settings_window,
    );
    if settings_window.is_null()
        || result_window.is_null()
        || chip.is_null()
        || appearance_window.is_null()
        || inline_window.is_null()
        || speech_window.is_null()
    {
        return Err("无法创建翻译窗口。".into());
    }
    CHIP_WINDOW.set(chip as isize);
    SETTINGS_WINDOW.set(settings_window as isize);
    APPEARANCE_WINDOW.set(appearance_window as isize);
    theme::apply_window(settings_window, settings.theme_mode, settings.theme_preset);
    theme::apply_window(result_window, settings.theme_mode, settings.theme_preset);
    theme::apply_window(
        appearance_window,
        settings.theme_mode,
        settings.theme_preset,
    );
    for window in [main, settings_window, result_window, appearance_window] {
        set_window_icons(window, &runtime_icons);
    }
    popup::disable_shadow(chip);
    let settings_ui = make_settings(settings_window);
    let result_ui = make_result(result_window);
    let inline_ui = make_inline(inline_window);
    let appearance_ui = make_appearance(appearance_window);
    let speech_ui = make_speech_settings(speech_window);
    let (events_tx, events) = mpsc::channel();
    let ui_events = events_tx.clone();
    let (capture_tx, capture_rx) = mpsc::channel::<CaptureWork>();
    let (network_tx, network_rx) = mailbox::channel::<TranslationJob>();
    let destination = main as isize;
    let capture_events = events_tx.clone();
    let latest_capture = Arc::new(AtomicU64::new(0));
    let worker_capture = latest_capture.clone();
    thread::Builder::new()
        .name("selection".into())
        .spawn(move || {
            for work in capture_rx {
                let mut job = match work {
                    CaptureWork::Warm(foreground) => {
                        selection::warm_selection_provider(foreground);
                        if capture_events.send(Event::Warmed).is_err() {
                            break;
                        }
                        PostMessageW(destination as HWND, WM_WORK, 0, 0);
                        continue;
                    }
                    CaptureWork::Capture(job) => job,
                };
                let cancelled = || worker_capture.load(Ordering::Acquire) != job.generation;
                let result = if cancelled() {
                    Err("选区已改变，已取消读取。".into())
                } else if job.kind == CaptureKind::Clipboard {
                    selection::read_clipboard_text()
                } else if GetForegroundWindow() as isize != job.foreground {
                    Err("前台窗口已改变，请重新选择文字。".into())
                } else if job.kind == CaptureKind::Auto {
                    let result = selection::capture_selection_for_gesture(job.origin, job.point);
                    if job.compatibility
                        && !cancelled()
                        && result.as_ref().err().is_some_and(|error| {
                            !selection::is_no_selection_error(error)
                                && !selection::is_protected_error(error)
                        })
                        && selection::can_copy_gesture(job.origin, job.point)
                    {
                        job.compatibility_checked = true;
                        let outcome = clipboard_copy::capture_checked(
                            job.foreground,
                            job.clipboard_sequence,
                            cancelled,
                        );
                        job.copy_attempted = outcome.injected;
                        job.copy_retryable = outcome.retryable;
                        outcome.result.map(|text| selection::CapturedText {
                            text,
                            source: "兼容取词",
                        })
                    } else {
                        result
                    }
                } else {
                    selection::capture_selection(true)
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
                let cached = if job.test || job.force_refresh {
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
    let manual_hotkey_ok = manual_ui::register(main, settings.manual_hotkey);
    mouse::set_enabled(settings.selection_button);
    let hook = mouse::install(main as isize, WM_SELECTION);
    let accessibility = selection_events::install(main as isize, WM_ACCESSIBILITY);
    let hook_error = hook.as_ref().err().cloned();
    let conversation = conversation_ui::State::new(main, ui_events.clone())?;
    let speech_sender = ui_events.clone();
    let speech_notify_window = main as isize;
    let speech = speech::Speech::new(move |event| {
        let _ = speech_sender.send(Event::Speech(event));
        unsafe {
            PostMessageW(speech_notify_window as HWND, WM_WORK, 0, 0);
        }
    })?;
    let app = App {
        main,
        settings_window,
        result_window,
        chip,
        inline_window,
        inline_ui,
        inline_active: false,
        inline_positioned: false,
        result_positioned: false,
        manual_mode: false,
        manual_draft: String::new(),
        manual_hotkey_registered: manual_hotkey_ok,
        window_pinned: false,
        selection_paused: false,
        appearance_window,
        appearance_ui,
        speech_window,
        speech_ui,
        speech_restore_view: None,
        speech_settings_was_visible: false,
        icons: runtime_icons,
        preview_icons: None,
        window_icons: HashMap::new(),
        settings_ui,
        result_ui,
        settings,
        events,
        ui_events,
        capture: capture_tx,
        network: network_tx,
        generation: 0,
        capture_generation: 0,
        latest_capture,
        test_generation: 0,
        latest_translation,
        latest_test,
        capture_busy: false,
        warm_busy: false,
        warmed_foreground: 0,
        pending: None,
        auto_point: POINT { x: 0, y: 0 },
        auto_foreground: 0,
        auto_source_root: 0,
        auto_origin: None,
        auto_pressed: false,
        auto_strong: false,
        auto_started: None,
        auto_attempt: 0,
        auto_text: None,
        pending_icon_click: false,
        compatibility_attempted: false,
        chip_deadline: None,
        chip_pixels: None,
        chip_dpi: 0,
        picking_color: false,
        chip_text: String::new(),
        chip_foreground: 0,
        conversation,
        speech,
        speech_token: 0,
        speaking: false,
        translation_ready: false,
        translated: String::new(),
        rendered_text: String::new(),
        display_pending: false,
        taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        tray,
        settings_scroll: 0,
        settings_epoch: 0,
        active_epoch: 0,
        active_text: String::new(),
        active_started: None,
        streaming: false,
        cache: shared_cache,
        update_busy: false,
        available_update: None,
        downloaded_update: None,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(|app| {
        warm_foreground(app, GetForegroundWindow() as isize);
        layout_settings(app);
        layout_result(app);
        layout_appearance(app);
        layout_inline(app);
        layout_chip(app);
        layout_speech_settings(app.speech_window);
        theme::apply_window(app.speech_window, CURRENT_THEME.get(), CURRENT_PRESET.get());
        for window in [
            app.main,
            app.settings_window,
            app.result_window,
            app.appearance_window,
        ] {
            refresh_window_icons(app, window);
        }
        fill_settings(app);
        refresh_translation_actions(app);
        let mut problems = Vec::new();
        if let Some(error) = load_error {
            problems.push(error);
        }
        if !hotkey_ok {
            problems.push("Ctrl+Alt+T 已被其他应用占用；划词按钮与翻译剪贴板仍可用。".into());
        }
        if !manual_hotkey_ok {
            problems.push("手动翻译快捷键已被占用，可在偏好设置中更改。".into());
        }
        if let Some(error) = hook_error {
            problems.push(error);
        }
        show_settings(app);
        if !problems.is_empty() {
            set_text(app.settings_ui.status, &problems.join("\r\n"));
        }
        check_updates(app, false);
    });
    update_install::notify_startup_ready();
    let mut message: MSG = zeroed();
    loop {
        let status = GetMessageW(&mut message, null_mut(), 0, 0);
        if status <= 0 {
            break;
        }
        if message.message == WM_KEYDOWN
            && message.wParam == VK_RETURN as usize
            && message.hwnd == GetDlgItem(result_window, 222)
        {
            let mut manual = false;
            with_app(|app| {
                manual = app.manual_mode;
                if manual && GetKeyState(VK_CONTROL as i32) < 0 {
                    manual_ui::submit(app, false);
                }
            });
            if manual {
                if GetKeyState(VK_CONTROL as i32) >= 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                continue;
            }
        }
        if message.message == WM_KEYDOWN
            && message.wParam == VK_RETURN as usize
            && GetDlgCtrlID(message.hwnd) == ID_QUESTION as i32
        {
            let root = GetAncestor(message.hwnd, GA_ROOT);
            if root == inline_window || root == result_window {
                with_app(|app| {
                    if !conversation_ui::is_busy(&app.conversation) {
                        conversation_ui::submit(app, root);
                    }
                });
                continue;
            }
        }
        if message.message == WM_KEYDOWN && message.wParam == VK_ESCAPE as usize {
            let root = GetAncestor(message.hwnd, GA_ROOT);
            if root == result_window
                || root == settings_window
                || root == appearance_window
                || root == inline_window
                || root == speech_window
            {
                if root == inline_window {
                    with_app(|app| close_inline(app));
                } else if root == result_window {
                    with_app(|app| close_translation(app));
                } else if root == speech_window {
                    with_app(|app| speech_ui::close(app));
                } else if root == appearance_window {
                    with_app(|app| close_appearance(app));
                } else {
                    if root == settings_window {
                        with_app(|app| mask_api_key(app));
                    }
                    ShowWindow(root, SW_HIDE);
                }
                continue;
            }
        }
        if IsDialogMessageW(speech_window, &message) == 0
            && IsDialogMessageW(settings_window, &message) == 0
            && IsDialogMessageW(result_window, &message) == 0
            && IsDialogMessageW(appearance_window, &message) == 0
            && IsDialogMessageW(inline_window, &message) == 0
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
    drop(accessibility);
    drop(hook);
    UnregisterHotKey(main, HOTKEY);
    UnregisterHotKey(main, HOTKEY_MANUAL);
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().take() {
            Shell_NotifyIconW(NIM_DELETE, &app.tray);
            DestroyWindow(app.chip);
            DestroyWindow(app.inline_window);
            DestroyWindow(app.result_window);
            DestroyWindow(app.settings_window);
            DestroyWindow(app.appearance_window);
            DestroyWindow(app.speech_window);
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
    SETTINGS_WINDOW.set(0);
    APPEARANCE_WINDOW.set(0);

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
                || matches!(id, 210..=213 | 222 | 224 | 237 | 251 | 260 | 272 | 273 | 301 | 304..=307)
                || id == ID_PROVIDER as i32;
            with_theme(|palette| {
                let dc = wparam as HDC;
                SetBkColor(
                    dc,
                    if surface {
                        palette.surface_color
                    } else if surfaces::is_card_control(control) {
                        palette.card_color
                    } else {
                        palette.background_color
                    },
                );
                SetBkMode(dc, OPAQUE as i32);
                SetTextColor(
                    dc,
                    if surface || matches!(id, 200 | 220 | 250 | 290 | 294 | 298 | 299) {
                        palette.text_color
                    } else {
                        palette.muted_color
                    },
                );
                if surface {
                    palette.surface as isize
                } else if surfaces::is_card_control(control) {
                    palette.card as isize
                } else {
                    palette.background as isize
                }
            })
        }
        WM_MEASUREITEM if lparam != 0 => {
            let item = &mut *(lparam as *mut MEASUREITEMSTRUCT);
            if item.CtlType == ODT_COMBOBOX {
                item.itemHeight = scaled(32, dpi(hwnd)) as u32;
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
                } else if let Some(preset) = preset_for_control(item.CtlID as u16) {
                    palette.draw_preset(item, face, preset, preset == CURRENT_PRESET.get());
                } else if matches!(
                    item.CtlID as u16,
                    ID_SERVICE_TAB | ID_PREFERENCES_TAB | ID_APPEARANCE_TAB
                ) {
                    let selected = matches!(
                        (item.CtlID as u16, SETTINGS_PAGE.get()),
                        (ID_SERVICE_TAB, SettingsPage::Service)
                            | (ID_PREFERENCES_TAB, SettingsPage::Preferences)
                            | (ID_APPEARANCE_TAB, SettingsPage::Appearance)
                    );
                    palette.draw_navigation(item, face, selected);
                } else if item.CtlID == ID_AUTO as u32 {
                    palette.draw_checkbox(item, face, AUTO_CHECKED.get());
                } else if item.CtlID == ID_COMPATIBILITY as u32 {
                    palette.draw_checkbox(item, face, COMPATIBILITY_CHECKED.get());
                } else if let Some(symbol) = button_symbol(item.CtlID) {
                    symbols::draw_button(item, face, palette, symbol);
                } else {
                    palette.draw_button(
                        item,
                        face,
                        matches!(
                            item.CtlID as u16,
                            ID_SAVE | ID_SPEECH_APPLY | ID_TRANSLATE_INPUT
                        ) || (item.CtlID == ID_SERVICE_TAB as u32
                            && SETTINGS_PAGE.get() == SettingsPage::Service)
                            || (item.CtlID == ID_PREFERENCES_TAB as u32
                                && SETTINGS_PAGE.get() == SettingsPage::Preferences)
                            || (item.CtlID == ID_APPEARANCE_TAB as u32
                                && SETTINGS_PAGE.get() == SettingsPage::Appearance)
                            || item.CtlID == ID_ICON_APPLY as u32,
                    );
                }
                1
            })
        }
        WM_ERASEBKGND if hwnd as isize == CHIP_WINDOW.get() && CHIP_WINDOW.get() != 0 => Some(1),
        WM_ERASEBKGND => with_theme(|palette| {
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            FillRect(wparam as HDC, &rect, palette.background);
            paint_panels(hwnd, wparam as HDC, palette, &rect);
            1
        }),
        WM_PAINT if hwnd as isize == CHIP_WINDOW.get() && CHIP_WINDOW.get() != 0 => {
            let mut paint: PAINTSTRUCT = zeroed();
            BeginPaint(hwnd, &mut paint);
            EndPaint(hwnd, &paint);
            Some(0)
        }
        _ => None,
    }
}

fn preset_for_control(id: u16) -> Option<config::ThemePreset> {
    match id {
        ID_PRESET_OCEAN => Some(config::ThemePreset::Ocean),
        ID_PRESET_AURORA => Some(config::ThemePreset::Aurora),
        ID_PRESET_NEBULA => Some(config::ThemePreset::Nebula),
        ID_PRESET_AMBER => Some(config::ThemePreset::Amber),
        _ => None,
    }
}

unsafe fn paint_panels(hwnd: HWND, dc: HDC, palette: &theme::Theme, client: &RECT) {
    let scale = dpi(hwnd);
    let width = client.right * 96 / scale as i32;
    let (panels, scroll) = if hwnd as isize == SETTINGS_WINDOW.get() {
        let (rail, _, _) = settings_geometry(width);
        let sidebar = RECT {
            left: 0,
            top: 0,
            right: scaled(rail, scale),
            bottom: client.bottom,
        };
        FillRect(dc, &sidebar, palette.card);
        let mut info: SCROLLINFO = zeroed();
        info.cbSize = size_of::<SCROLLINFO>() as u32;
        info.fMask = SIF_POS;
        GetScrollInfo(hwnd, SB_VERT, &mut info);
        let panels = settings_panels(width, SETTINGS_PAGE.get())
            .into_iter()
            .map(|rect| (rect.left, rect.top, rect.right, rect.bottom))
            .collect::<Vec<_>>();
        (panels, info.nPos)
    } else if hwnd as isize == APPEARANCE_WINDOW.get() {
        (vec![(20, 84, width - 20, 244)], 0)
    } else {
        return;
    };
    for (left, top, right, bottom) in panels {
        palette.draw_card(
            dc,
            RECT {
                left: scaled(left, scale),
                top: scaled(top - scroll, scale),
                right: scaled(right, scale),
                bottom: scaled(bottom - scroll, scale),
            },
            scaled(12, scale),
        );
    }
}

fn button_symbol(id: u32) -> Option<symbols::Symbol> {
    use symbols::Symbol;
    Some(match id as u16 {
        ID_THEME => {
            if CURRENT_THEME.get() == config::ThemeMode::Dark {
                Symbol::Sun
            } else {
                Symbol::Moon
            }
        }
        ID_SPEAK => {
            if SPEECH_BUSY.get() {
                Symbol::SpeakStop
            } else {
                Symbol::Speak
            }
        }
        ID_POLISH => Symbol::Polish,
        ID_COPY => Symbol::Copy,
        ID_TEST => Symbol::Connection,
        ID_SEND => {
            if CONVERSATION_BUSY.get() {
                Symbol::Stop
            } else {
                Symbol::Send
            }
        }
        ID_RETRY => {
            if TRANSLATION_STREAMING.get() {
                Symbol::Stop
            } else {
                Symbol::Retry
            }
        }
        ID_PIN => {
            if WINDOW_PINNED.get() {
                Symbol::PinActive
            } else {
                Symbol::Pin
            }
        }
        ID_SWAP_LANGUAGES => Symbol::Swap,
        ID_KEY_VISIBILITY => {
            if KEY_VISIBLE.get() {
                Symbol::EyeClosed
            } else {
                Symbol::Eye
            }
        }
        ID_INLINE_EXPAND => Symbol::Expand,
        ID_INLINE_CLOSE => Symbol::Close,
        ID_SETTINGS => Symbol::Settings,
        _ => return None,
    })
}

unsafe fn selected_language(
    control: HWND,
    languages: &[config::Language],
) -> Result<config::Language, String> {
    let index = SendMessageW(control, CB_GETCURSEL, 0, 0);
    if index < 0 {
        return Err("请选择语言。".into());
    }
    languages
        .get(index as usize)
        .copied()
        .ok_or_else(|| "请选择有效语言。".into())
}

unsafe fn update_language_labels(app: &App) {
    let pair = format!(
        "{} → {}",
        app.settings.source_language.label(),
        app.settings.target_language.label()
    );
    set_text(GetDlgItem(app.inline_window, 250), &pair);
    set_text(GetDlgItem(app.result_window, 220), &pair);
}

unsafe fn update_appearance_summary(app: &App) {
    let style = match app.settings.icon_style {
        config::IconStyle::Leaf => "叶片",
        config::IconStyle::Translate => "译字",
        config::IconStyle::Letter => "字母 A",
    };
    let shape = match app.settings.icon_shape {
        config::IconShape::Rounded => "圆角",
        config::IconShape::Square => "方形",
        config::IconShape::Circle => "圆形",
        config::IconShape::Leaf => "叶形",
    };
    set_text(
        GetDlgItem(app.settings_window, 276),
        &format!("{style} · {shape} · {} px", app.settings.icon_size),
    );
}

unsafe fn close_appearance(app: &mut App) {
    if app.picking_color {
        return;
    }
    ShowWindow(app.appearance_window, SW_HIDE);
    EnableWindow(app.settings_window, 1);
    SetForegroundWindow(app.settings_window);
    SetFocus(GetDlgItem(app.settings_window, ID_APPEARANCE as i32));
}

unsafe fn position_over_owner(hwnd: HWND, owner: HWND, width: i32, height: i32) {
    let mut owner_rect: RECT = zeroed();
    GetWindowRect(owner, &mut owner_rect);
    let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        return;
    }
    let scale = dpi(owner);
    let work = info.rcWork;
    let w = scaled(width, scale).min(work.right - work.left);
    let h = scaled(height, scale).min(work.bottom - work.top);
    let x = (owner_rect.left + (owner_rect.right - owner_rect.left - w) / 2)
        .clamp(work.left, work.right - w);
    let y = (owner_rect.top + (owner_rect.bottom - owner_rect.top - h) / 2)
        .clamp(work.top, work.bottom - h);
    SetWindowPos(hwnd, HWND_TOP, x, y, w, h, SWP_NOACTIVATE);
}

fn automatic_enabled(app: &App) -> bool {
    app.settings.selection_button && !app.selection_paused && !app.manual_mode
}

unsafe fn update_pause_ui(app: &App) {
    let control = GetDlgItem(app.settings_window, ID_PAUSE as i32);
    set_text(
        control,
        if app.selection_paused {
            "恢复划词"
        } else {
            "暂停划词"
        },
    );
    EnableWindow(control, app.settings.selection_button as i32);
    update_language_labels(app);
}

unsafe fn toggle_selection_pause(app: &mut App) {
    if !app.settings.selection_button {
        return;
    }
    app.selection_paused = !app.selection_paused;
    mouse::set_enabled(automatic_enabled(app));
    cancel_automatic(app);
    update_pause_ui(app);
    set_text(
        app.settings_ui.status,
        if app.selection_paused {
            "本次已暂停 · 快捷键与剪贴板翻译仍可用"
        } else {
            "自动划词已恢复"
        },
    );
    copy_wide(
        &mut app.tray.szTip,
        if app.selection_paused {
            "轻译 · 划词已暂停 · Ctrl+Alt+T"
        } else {
            "轻译 · Ctrl+Alt+T 翻译选区"
        },
    );
    Shell_NotifyIconW(NIM_MODIFY, &app.tray);
}

unsafe fn update_swap_button(app: &App) {
    let enabled = selected_language(app.settings_ui.source, config::SOURCE_LANGUAGES)
        .is_ok_and(|language| language != config::Language::Auto);
    EnableWindow(
        GetDlgItem(app.settings_window, ID_SWAP_LANGUAGES as i32),
        enabled as i32,
    );
}

unsafe fn swap_languages(app: &App) {
    let Ok(source) = selected_language(app.settings_ui.source, config::SOURCE_LANGUAGES) else {
        return;
    };
    let Ok(target) = selected_language(app.settings_ui.target, config::TARGET_LANGUAGES) else {
        return;
    };
    let Some(new_target) = config::TARGET_LANGUAGES
        .iter()
        .position(|language| *language == source)
    else {
        return;
    };
    let Some(new_source) = config::SOURCE_LANGUAGES
        .iter()
        .position(|language| *language == target)
    else {
        return;
    };
    SendMessageW(app.settings_ui.source, CB_SETCURSEL, new_source, 0);
    SendMessageW(app.settings_ui.target, CB_SETCURSEL, new_target, 0);
    update_swap_button(app);
    set_text(app.settings_ui.status, "");
}

unsafe fn set_api_key_visibility(app: &App, visible: bool) {
    KEY_VISIBLE.set(visible);
    SendMessageW(
        app.settings_ui.key,
        EM_SETPASSWORDCHAR,
        if visible { 0 } else { 0x25cf },
        0,
    );
    InvalidateRect(app.settings_ui.key, null(), 0);
    let button = GetDlgItem(app.settings_window, ID_KEY_VISIBILITY as i32);
    set_text(
        button,
        if visible {
            "隐藏 API 密钥"
        } else {
            "显示 API 密钥"
        },
    );
    InvalidateRect(button, null(), 0);
}

unsafe fn mask_api_key(app: &App) {
    set_api_key_visibility(app, false);
}

unsafe fn refresh_translation_actions(app: &App) {
    refresh_speech_actions(app);
    let changed = TRANSLATION_STREAMING.replace(app.streaming) != app.streaming;
    let pin_changed = WINDOW_PINNED.replace(app.window_pinned) != app.window_pinned;
    for button in [app.inline_ui.action, app.result_ui.action] {
        EnableWindow(
            button,
            (!app.display_pending && !app.active_text.is_empty()) as i32,
        );
        if changed || text(button).is_empty() {
            set_text(
                button,
                if app.streaming {
                    "停止翻译"
                } else {
                    "重新翻译"
                },
            );
            InvalidateRect(button, null(), 0);
        }
    }
    if pin_changed {
        for button in [app.inline_ui.pin, app.result_ui.pin] {
            set_text(
                button,
                if app.window_pinned {
                    "取消保持显示"
                } else {
                    "保持窗口显示"
                },
            );
            InvalidateRect(button, null(), 0);
        }
    }
}

unsafe fn refresh_speech_actions(app: &App) {
    let changed = SPEECH_BUSY.replace(app.speaking) != app.speaking;
    let ready = app.translation_ready
        && !app.display_pending
        && !app.streaming
        && !app.translated.trim().is_empty();
    for window in [app.inline_window, app.result_window] {
        let button = GetDlgItem(window, ID_SPEAK as i32);
        EnableWindow(button, (ready || app.speaking) as i32);
        if changed || text(button).is_empty() {
            set_text(
                button,
                if app.speaking {
                    "停止朗读"
                } else {
                    "朗读译文"
                },
            );
            InvalidateRect(button, null(), 0);
        }
    }
}

unsafe fn stop_speech(app: &mut App) {
    if app.speaking {
        app.speech_token = app.speech.stop();
        app.speaking = false;
        refresh_speech_actions(app);
    }
}

unsafe fn toggle_speech(app: &mut App) {
    if app.speaking {
        stop_speech(app);
        translation_status(app, "已停止朗读");
    } else if app.translation_ready && !app.display_pending && !app.streaming {
        if app.settings.speech_settings().validate().is_err() {
            speech_ui::show(app, true);
            return;
        }
        let spoken = crate::markdown::plain_text(&app.translated);
        if !spoken.trim().is_empty() {
            app.speech_token = app.speech.start(spoken, app.settings.speech_settings());
            app.speaking = true;
            refresh_speech_actions(app);
        }
    }
}

unsafe fn handle_speech(app: &mut App, event: speech::Event) {
    if event.token != app.speech_token {
        return;
    }
    let status = match event.result {
        Ok(speech::Outcome::Started) => {
            app.speaking = true;
            "正在朗读译文".to_owned()
        }
        Ok(speech::Outcome::Finished) => {
            app.speaking = false;
            "朗读完成".to_owned()
        }
        Ok(speech::Outcome::Stopped) => {
            app.speaking = false;
            "已停止朗读".to_owned()
        }
        Err(error) => {
            app.speaking = false;
            error
        }
    };
    refresh_speech_actions(app);
    if !app.streaming && !conversation_ui::is_busy(&app.conversation) {
        translation_status(app, &status);
    }
}

unsafe fn stop_translation(app: &mut App) {
    app.generation += 1;
    app.latest_translation
        .store(app.generation, Ordering::Release);
    app.streaming = false;
    app.active_started = None;
    app.display_pending = false;
    conversation_ui::reset(app);
    translation_copy_enabled(app, false);
    if app.translated.is_empty() {
        app.rendered_text.clear();
        for editor in [app.result_ui.translated, app.inline_ui.translated] {
            set_text(editor, "翻译已停止，可点击重新翻译再试。");
        }
    } else {
        let partial = app.translated.clone();
        render_translation(app, &partial);
        EnableWindow(app.result_ui.copy, 1);
        EnableWindow(app.inline_ui.copy, 1);
    }
    translation_status(app, "已停止 · 可重新翻译");
    refresh_translation_actions(app);
}

unsafe fn is_translation_copy_target(app: &App, window: HWND) -> bool {
    window == app.inline_ui.translated
        || window == app.result_ui.translated
        || window == app.inline_ui.copy
        || window == app.result_ui.copy
}

unsafe fn copy_menu(app: &mut App, target: HWND, position: LPARAM) {
    let editor = if GetAncestor(target, GA_ROOT) == app.inline_window {
        app.inline_ui.translated
    } else {
        app.result_ui.translated
    };
    let mut start = 0u32;
    let mut end = 0u32;
    SendMessageW(
        editor,
        EM_GETSEL,
        &mut start as *mut u32 as usize,
        &mut end as *mut u32 as isize,
    );
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    AppendMenuW(
        menu,
        MF_STRING | if start != end { 0 } else { MF_GRAYED },
        ID_COPY_SELECTION as usize,
        wide("复制选中文字").as_ptr(),
    );
    let available = !app.display_pending && !app.translated.is_empty();
    let flags = MF_STRING | if available { 0 } else { MF_GRAYED };
    AppendMenuW(menu, flags, ID_COPY as usize, wide("复制译文").as_ptr());
    AppendMenuW(
        menu,
        flags
            | if conversation_ui::has_content(&app.conversation) {
                0
            } else {
                MF_GRAYED
            },
        ID_COPY_CONVERSATION as usize,
        wide("复制译文与问答").as_ptr(),
    );
    AppendMenuW(
        menu,
        flags,
        ID_COPY_MARKDOWN as usize,
        wide("复制 Markdown 原文").as_ptr(),
    );
    let point = if position == -1 {
        cursor()
    } else {
        POINT {
            x: (position as u16 as i16) as i32,
            y: ((position >> 16) as u16 as i16) as i32,
        }
    };
    let chosen = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
        point.x,
        point.y,
        0,
        GetAncestor(editor, GA_ROOT),
        null(),
    );
    DestroyMenu(menu);
    // Native menus pump messages while App is borrowed; drain any queued events
    // whose original WM_WORK was deferred by that nested loop.
    PostMessageW(app.main, WM_WORK, 0, 0);
    if chosen == ID_COPY_SELECTION as i32 {
        SendMessageW(editor, WM_COPY, 0, 0);
    } else if chosen != 0 {
        command(app, chosen as u16);
    }
}

/// Preserve selection and reading position while a stream grows. Only follow
/// new content when the user was already at the bottom without a text selection.
unsafe fn render_editor(editor: HWND, previous: &str, current: &str, fresh: bool) {
    rich_text::render(
        editor,
        previous,
        current,
        fresh,
        CURRENT_THEME.get(),
        CURRENT_PRESET.get(),
    );
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
    mask_api_key(app);
    // A result from testing the previous provider must not describe this draft.
    app.test_generation += 1;
    app.latest_test
        .store(app.test_generation, Ordering::Release);
    EnableWindow(GetDlgItem(app.settings_window, ID_TEST as i32), 1);
    set_text(app.settings_ui.status, "");
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NOTICE {
        handle_notice(hwnd);
        return 0;
    }
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
                check_updates(app, false);
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
            WM_HOTKEY if wparam == HOTKEY_MANUAL as usize => {
                manual_ui::open(app);
                handled = true;
            }
            WM_HOTKEY if wparam == HOTKEY as usize => {
                request_capture(app, CaptureKind::Hotkey, cursor());
                handled = true;
            }
            WM_SELECTION => {
                if lparam == isize::MIN {
                    begin_automatic_press(app, mouse::press_point(wparam));
                    handled = true;
                    return;
                }
                if automatic_enabled(app) && !app.picking_color {
                    let foreground = GetForegroundWindow();
                    if !app.auto_pressed {
                        handled = true;
                        return;
                    }
                    if app.auto_source_root == 0
                        || GetAncestor(foreground, GA_ROOT) as isize != app.auto_source_root
                    {
                        cancel_automatic(app);
                        handled = true;
                        return;
                    }
                    app.auto_point = POINT {
                        x: wparam as i32,
                        y: lparam as i32,
                    };
                    app.auto_pressed = false;
                    app.auto_strong = mouse::is_selection_gesture(wparam);
                    app.auto_foreground = foreground as isize;
                    app.auto_started = Some(Instant::now());
                    app.auto_attempt = 0;
                    // A gesture is only a probe candidate. Blank drags and
                    // delayed providers must never create a speculative popup.
                    SetTimer(app.main, TIMER_SELECTION, 60, None);
                }
                handled = true;
            }
            WM_ACCESSIBILITY => {
                let window = lparam as HWND;
                let foreground = GetForegroundWindow();
                if wparam as u32 == selection_events::FOREGROUND {
                    if window == foreground {
                        if (app.auto_pressed || app.auto_started.is_some())
                            && !is_our_window(foreground)
                            && GetAncestor(foreground, GA_ROOT) as isize != app.auto_source_root
                        {
                            cancel_automatic(app);
                        }
                        warm_foreground(app, foreground as isize);
                    }
                } else if wparam as u32 == selection_events::SELECTION
                    && automatic_enabled(app)
                    && !app.picking_color
                    && !app.auto_pressed
                    && app
                        .auto_started
                        .is_some_and(|time| time.elapsed() < Duration::from_secs(5))
                    && foreground as isize == app.auto_foreground
                    && GetAncestor(window, GA_ROOT) == GetAncestor(foreground, GA_ROOT)
                    && !is_our_window(window)
                {
                    // A provider may publish a PDF selection after mouse-up.
                    // Coalesce hints; only the recent physical gesture grants
                    // permission to inspect it, never the WinEvent alone.
                    SetTimer(app.main, TIMER_SELECTION, 20, None);
                }
                handled = true;
            }
            WM_TIMER if wparam == TIMER_SELECTION => {
                KillTimer(app.main, TIMER_SELECTION);
                if automatic_enabled(app)
                    && app
                        .auto_started
                        .is_some_and(|time| time.elapsed() < Duration::from_secs(5))
                    && app.auto_foreground == GetForegroundWindow() as isize
                    && !app.picking_color
                {
                    request_capture(app, CaptureKind::Auto, app.auto_point);
                } else if app.auto_started.is_some() {
                    if !automatic_enabled(app)
                        || app.picking_color
                        || app.auto_foreground != GetForegroundWindow() as isize
                    {
                        cancel_automatic(app);
                    } else {
                        retry_automatic(app);
                    }
                }
                handled = true;
            }
            WM_TIMER if wparam == TIMER_CHIP => {
                let foreground = GetForegroundWindow();
                if IsWindowVisible(app.chip) != 0
                    && (!app
                        .chip_deadline
                        .is_some_and(|deadline| Instant::now() < deadline)
                        || foreground as isize != app.chip_foreground)
                {
                    ShowWindow(app.chip, SW_HIDE);
                    app.chip_deadline = None;
                }
                if !app.window_pinned
                    && app.inline_active
                    && IsWindowVisible(app.inline_window) != 0
                    && foreground as isize != app.chip_foreground
                    && GetAncestor(foreground, GA_ROOT) != app.inline_window
                {
                    stop_speech(app);
                    ShowWindow(app.inline_window, SW_HIDE);
                }
                if IsWindowVisible(app.chip) == 0 && IsWindowVisible(app.inline_window) == 0 {
                    KillTimer(app.main, TIMER_CHIP);
                }
                handled = true;
            }
            WM_CONTEXTMENU if is_translation_copy_target(app, wparam as HWND) => {
                copy_menu(app, wparam as HWND, lparam);
                handled = true;
            }
            WM_WORK => {
                drain_events(app);
                handled = true;
            }
            WM_COMMAND => {
                let id = (wparam & 0xffff) as u16;
                if id == ID_SEND && (wparam >> 16) as u16 == BN_CLICKED as u16 {
                    conversation_ui::submit(app, hwnd);
                } else if id == ID_SOURCE_LANGUAGE || id == ID_TARGET_LANGUAGE {
                    if (wparam >> 16) as u16 == CBN_SELCHANGE as u16 {
                        update_swap_button(app);
                        set_text(app.settings_ui.status, "");
                    }
                } else if id == ID_PROVIDER {
                    if (wparam >> 16) as u16 == CBN_SELCHANGE as u16 {
                        change_provider(app);
                    }
                } else if id == ID_SELECTION_MODE {
                    if (wparam >> 16) as u16 == CBN_SELCHANGE as u16 {
                        update_mode_hint(app);
                    }
                } else if id == ID_ICON_STYLE || id == ID_ICON_SHAPE {
                    if (wparam >> 16) as u16 == CBN_SELCHANGE as u16 {
                        refresh_icon_preview(app);
                    }
                } else if id == ID_ICON_SIZE {
                    if (wparam >> 16) as u16 == EN_CHANGE as u16 && selected_icon_size(app).is_ok()
                    {
                        refresh_icon_preview(app);
                    }
                } else if (wparam >> 16) as u16 == BN_CLICKED as u16 {
                    command(app, id);
                }
                handled = true;
            }
            WM_CLOSE => {
                if hwnd == app.inline_window {
                    close_inline(app);
                } else if hwnd == app.result_window {
                    close_translation(app);
                } else if hwnd == app.settings_window {
                    mask_api_key(app);
                    ShowWindow(hwnd, SW_HIDE);
                } else if hwnd == app.speech_window {
                    speech_ui::close(app);
                } else if hwnd == app.appearance_window {
                    close_appearance(app);
                } else {
                    ShowWindow(hwnd, SW_HIDE);
                }
                handled = true;
            }
            WM_SIZE => {
                if hwnd == app.speech_window {
                    layout_speech_settings(hwnd);
                } else if hwnd == app.settings_window {
                    layout_settings(app);
                } else if hwnd == app.result_window {
                    layout_result(app);
                } else if hwnd == app.inline_window {
                    layout_inline(app);
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
            WM_GETMINMAXINFO if hwnd == app.result_window || hwnd == app.settings_window => {
                let info = &mut *(lparam as *mut MINMAXINFO);
                let dpi = dpi(hwnd);
                info.ptMinTrackSize = POINT {
                    x: scaled(
                        if hwnd == app.settings_window {
                            600
                        } else {
                            400
                        },
                        dpi,
                    ),
                    y: scaled(480, dpi),
                };
                handled = true;
            }
            WM_DPICHANGED if hwnd == app.chip => {
                handled = true;
            }
            WM_DPICHANGED => {
                refresh_window_icons(app, hwnd);
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
                    refresh_icon_preview(app);
                } else if hwnd == app.inline_window {
                    layout_inline(app);
                } else if hwnd == app.speech_window {
                    layout_speech_settings(hwnd);
                } else {
                    layout_chip(app);
                }
                handled = true;
            }
            WM_NCHITTEST if hwnd == app.inline_window => {
                let point = POINT {
                    x: (lparam as u16 as i16) as i32,
                    y: ((lparam >> 16) as u16 as i16) as i32,
                };
                let hit = WindowFromPoint(point);
                let mut local = point;
                ScreenToClient(hwnd, &mut local);
                if local.y >= 0
                    && local.y < scaled(40, dpi(hwnd))
                    && (hit == hwnd || GetDlgCtrlID(hit) == 250)
                {
                    value = HTCAPTION as isize;
                    handled = true;
                }
            }
            WM_MOUSEACTIVATE if hwnd == app.chip || hwnd == app.inline_window => {
                let control = WindowFromPoint(cursor());
                value = if hwnd == app.inline_window
                    && (control == app.inline_ui.question
                        || control == app.inline_ui.translated
                        || control == app.inline_ui.send
                        || control == app.inline_ui.action
                        || control == app.inline_ui.pin)
                {
                    MA_ACTIVATE
                } else {
                    MA_NOACTIVATE
                } as isize;
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
        ID_SERVICE_TAB | ID_PREFERENCES_TAB | ID_APPEARANCE_TAB => {
            SETTINGS_PAGE.set(match id {
                ID_SERVICE_TAB => SettingsPage::Service,
                ID_PREFERENCES_TAB => SettingsPage::Preferences,
                _ => SettingsPage::Appearance,
            });
            if id != ID_SERVICE_TAB {
                mask_api_key(app);
            }
            app.settings_scroll = 0;
            layout_settings(app);
        }
        ID_PRESET_OCEAN | ID_PRESET_AURORA | ID_PRESET_NEBULA | ID_PRESET_AMBER => {
            if let Some(preset) = preset_for_control(id) {
                apply_visual_theme(app, app.settings.theme_mode, preset);
            }
        }
        ID_RETRY => {
            if app.streaming {
                stop_translation(app);
            } else if app.manual_mode {
                manual_ui::submit(app, true);
            } else if !app.active_text.is_empty() {
                let text = app.active_text.clone();
                let inline = app.inline_active;
                start_translation_request(app, text, "重新翻译", app.auto_point, inline, true);
            }
        }
        ID_PIN => {
            app.window_pinned = !app.window_pinned;
            // Settings and pause can cancel the auto-hide timer while keeping
            // a pinned view. Restoring normal behavior must restore its timer.
            if app.inline_active && IsWindowVisible(app.inline_window) != 0 {
                SetTimer(app.main, TIMER_CHIP, 200, None);
            }
            refresh_translation_actions(app);
            translation_status(
                app,
                if app.window_pinned {
                    "已保持显示 · 新划词仍会更新译文"
                } else {
                    "已取消保持显示"
                },
            );
        }
        ID_PAUSE => toggle_selection_pause(app),
        ID_SWAP_LANGUAGES => swap_languages(app),
        ID_KEY_VISIBILITY => {
            let visible = !KEY_VISIBLE.get();
            set_api_key_visibility(app, visible);
        }
        ID_MANUAL_WINDOW => manual_ui::open(app),
        ID_TRANSLATE_INPUT => manual_ui::submit(app, false),
        ID_SPEECH_SETTINGS => speech_ui::show(app, false),
        ID_SPEECH_APPLY => speech_ui::save(app),
        ID_SPEECH_CLOSE => speech_ui::close(app),
        ID_SPEAK => toggle_speech(app),
        ID_POLISH => conversation_ui::polish(app),
        ID_COPY_MARKDOWN => {
            let value = conversation_ui::display_text(app);
            match write_clipboard(app.result_window, &value) {
                Ok(()) => translation_status(app, "Markdown 原文已复制"),
                Err(error) => translation_status(app, &error),
            }
        }
        ID_COPY_CONVERSATION => {
            let value = crate::markdown::plain_text(&conversation_ui::display_text(app));
            match write_clipboard(app.result_window, &value) {
                Ok(()) => translation_status(app, "译文与问答已复制"),
                Err(error) => translation_status(app, &error),
            }
        }
        ID_SETTINGS => show_settings(app),
        ID_THEME => toggle_theme(app),
        ID_UPDATE => {
            if app.downloaded_update.is_some() {
                enqueue_notice(app, Notice::RestartUpdate);
            } else if app.available_update.is_some() {
                download_available_update(app);
            } else {
                check_updates(app, true);
            }
        }
        ID_APPEARANCE => show_appearance(app),
        ID_INLINE_EXPAND => {
            present_translation(app, false, cursor());
            SetForegroundWindow(app.result_window);
        }
        ID_INLINE_CLOSE => close_inline(app),
        ID_SCREEN_PICK => begin_screen_pick(app),
        ID_ICON_RESET => {
            let defaults = config::Settings::default();
            DRAFT_ICON_COLOR.set(defaults.icon_color);
            SendMessageW(
                app.appearance_ui.shape,
                CB_SETCURSEL,
                shape_index(defaults.icon_shape),
                0,
            );
            set_text(app.appearance_ui.size, &defaults.icon_size.to_string());
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
            update_mode_hint(app);
        }
        ID_COMPATIBILITY => {
            COMPATIBILITY_CHECKED.set(!COMPATIBILITY_CHECKED.get());
            InvalidateRect(app.settings_ui.compatibility, null(), 0);
            update_mode_hint(app);
        }
        ID_CLIPBOARD => request_capture(app, CaptureKind::Clipboard, cursor()),
        ID_EXIT => PostQuitMessage(0),
        ID_CANCEL => {
            ShowWindow(app.settings_window, SW_HIDE);
        }
        ID_COPY => match write_clipboard(
            app.result_window,
            &crate::markdown::plain_text(&app.translated),
        ) {
            Ok(()) => translation_status(app, "译文已复制"),
            Err(error) => translation_status(app, &error),
        },
        ID_TRANSLATE => {
            ShowWindow(app.chip, SW_HIDE);
            if GetForegroundWindow() as isize != app.chip_foreground {
                return;
            }
            let text = app.chip_text.clone();
            app.pending_icon_click = true;
            if crate::selection_text::has_visible_text(&text) {
                start_translation_in(app, text, "划词选区", app.auto_point, true);
            }
        }
        ID_SAVE => {
            match read_settings(app).and_then(|settings| manual_ui::save_settings(app, settings)) {
                Ok(settings) => {
                    if settings.base_url != app.settings.base_url
                        || settings.model != app.settings.model
                        || settings.api_key != app.settings.api_key
                        || settings.source_language != app.settings.source_language
                        || settings.target_language != app.settings.target_language
                    {
                        conversation_ui::reset(app);
                        translation_copy_enabled(app, false);
                        app.settings_epoch += 1;
                        app.generation += 1;
                        app.latest_translation
                            .store(app.generation, Ordering::Release);
                        app.streaming = false;
                        app.active_started = None;
                        translation_status(app, "已停止，请重新翻译");
                    }
                    app.test_generation += 1;
                    app.latest_test
                        .store(app.test_generation, Ordering::Release);
                    EnableWindow(GetDlgItem(app.settings_window, ID_TEST as i32), 1);
                    if !settings.selection_button
                        || settings.selection_mode != app.settings.selection_mode
                        || settings.compatibility_capture != app.settings.compatibility_capture
                    {
                        cancel_automatic(app);
                        if app.inline_active {
                            app.generation += 1;
                            app.latest_translation
                                .store(app.generation, Ordering::Release);
                            app.streaming = false;
                            app.inline_active = false;
                        }
                    }
                    app.settings = settings;
                    if !app.settings.selection_button {
                        app.selection_paused = false;
                        copy_wide(&mut app.tray.szTip, "轻译 · Ctrl+Alt+T 翻译选区");
                        Shell_NotifyIconW(NIM_MODIFY, &app.tray);
                    }
                    mouse::set_enabled(automatic_enabled(app));
                    update_pause_ui(app);
                    update_language_labels(app);
                    conversation_ui::refresh(app);
                    update_mode_hint(app);
                    refresh_translation_actions(app);
                    set_text(app.settings_ui.status, "");
                    enqueue_notice(app, Notice::Saved);
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
                            force_refresh: false,
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

unsafe fn cancel_automatic(app: &mut App) {
    let was_pending = app.display_pending;
    reset_automatic(app);
    if !app.window_pinned {
        ShowWindow(app.inline_window, SW_HIDE);
    } else if was_pending {
        if !app.translated.is_empty() {
            let original = app.translated.clone();
            render_translation(app, &original);
            translation_copy_enabled(app, app.translation_ready);
            translation_status(
                app,
                if app.streaming {
                    "正在翻译…"
                } else {
                    "已保留当前译文"
                },
            );
        } else {
            set_text(app.inline_ui.translated, "未读取到新选区，请重新划选。");
            translation_status(app, "请重新划选");
        }
    }
}

unsafe fn reset_automatic(app: &mut App) {
    KillTimer(app.main, TIMER_SELECTION);
    KillTimer(app.main, TIMER_CHIP);
    ShowWindow(app.chip, SW_HIDE);
    if !app.window_pinned && !app.manual_mode {
        ShowWindow(app.result_window, SW_HIDE);
    }
    app.chip_deadline = None;
    app.chip_text.clear();
    app.capture_generation += 1;
    app.latest_capture
        .store(app.capture_generation, Ordering::Release);
    app.pending = None;
    app.auto_started = None;
    app.auto_attempt = 0;
    app.auto_text = None;
    app.auto_origin = None;
    app.auto_source_root = 0;
    app.auto_pressed = false;
    app.auto_strong = false;
    app.pending_icon_click = false;
    app.compatibility_attempted = false;
    // Keep the network model alive for cache/in-flight reuse. Detach its view
    // immediately so an older stream cannot paint over a new selection.
    app.display_pending = !app.window_pinned && !app.manual_mode;
    conversation_ui::refresh(app);
}

unsafe fn begin_automatic_press(app: &mut App, point: POINT) {
    if !automatic_enabled(app) || app.picking_color {
        return;
    }
    if !app.window_pinned {
        conversation_ui::reset(app);
    }
    reset_automatic(app);
    if !app.window_pinned {
        ShowWindow(app.inline_window, SW_HIDE);
    }
    app.auto_origin = Some(point);
    app.auto_source_root = GetAncestor(WindowFromPoint(point), GA_ROOT) as isize;
    app.auto_pressed = true;
}

unsafe fn warm_foreground(app: &mut App, foreground: isize) {
    if foreground == 0
        || is_our_window(foreground as HWND)
        || foreground == app.warmed_foreground
        || app.warm_busy
        || app.capture_busy
        || app.auto_pressed
        || app
            .auto_started
            .is_some_and(|time| time.elapsed() < Duration::from_secs(5))
    {
        return;
    }
    if app.capture.send(CaptureWork::Warm(foreground)).is_ok() {
        app.warmed_foreground = foreground;
        app.warm_busy = true;
    }
}

unsafe fn retry_automatic(app: &mut App) {
    const DELAYS: [u32; 7] = [80, 140, 240, 400, 650, 900, 1200];
    // A plain click often only moves the caret. Reserve the longer PDF-loading
    // tail for an actual drag, multi-click or Shift selection gesture.
    let attempts = if app.auto_strong { DELAYS.len() } else { 1 };
    if app
        .auto_started
        .is_some_and(|started| started.elapsed() < Duration::from_secs(5))
        && app.auto_attempt < attempts
        && automatic_enabled(app)
        && GetForegroundWindow() as isize == app.auto_foreground
        && !app.picking_color
    {
        let delay = DELAYS[app.auto_attempt];
        app.auto_attempt += 1;
        SetTimer(app.main, TIMER_SELECTION, delay, None);
    } else {
        app.auto_started = None;
        // A late duplicate failure cannot dismiss an already confirmed selection.
        if app.auto_text.is_none() {
            cancel_automatic(app);
        }
    }
}

unsafe fn show_chip(app: &mut App, point: POINT) -> Result<(), String> {
    if !crate::selection_text::has_visible_text(&app.chip_text) {
        return Ok(());
    }
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        return Err("无法定位屏幕。".into());
    }
    let mut scale = 96;
    let mut dpi_y = 96;
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut scale, &mut dpi_y);
    if app.chip_pixels.is_none() || app.chip_dpi != scale {
        app.chip_pixels = Some(icons::render_pixels(
            app.settings.icon_style,
            app.settings.icon_color,
            app.settings.icon_shape,
            app.settings.icon_size,
            scale,
        )?);
        app.chip_dpi = scale;
    }
    let pixels = app.chip_pixels.as_ref().expect("icon pixels were created");
    let x = (point.x + scaled(12, scale))
        .min(info.rcWork.right - pixels.width)
        .max(info.rcWork.left);
    let y = (point.y + scaled(16, scale))
        .min(info.rcWork.bottom - pixels.height)
        .max(info.rcWork.top);
    popup::update(app.chip, x, y, pixels)?;
    app.chip_deadline = Some(Instant::now() + Duration::from_secs(12));
    ShowWindow(app.chip, SW_SHOWNOACTIVATE);
    SetTimer(app.main, TIMER_CHIP, 200, None);
    Ok(())
}

unsafe fn present_translation(app: &mut App, inline: bool, point: POINT) {
    ShowWindow(app.chip, SW_HIDE);
    app.inline_active = inline;
    if inline {
        ShowWindow(app.result_window, SW_HIDE);
        if !app.inline_positioned {
            position_near(app.inline_window, point, INLINE_WIDTH, INLINE_HEIGHT);
            app.inline_positioned = true;
        }
        SetWindowPos(
            app.inline_window,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        layout_inline(app);
        ShowWindow(app.inline_window, SW_SHOWNOACTIVATE);
        SetTimer(app.main, TIMER_CHIP, 200, None);
    } else {
        ShowWindow(app.inline_window, SW_HIDE);
        if !app.result_positioned {
            position_near(app.result_window, point, RESULT_WIDTH, RESULT_HEIGHT);
            app.result_positioned = true;
        }
        SetWindowPos(
            app.result_window,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        layout_result(app);
        ShowWindow(app.result_window, SW_SHOWNOACTIVATE);
    }
}

unsafe fn close_inline(app: &mut App) {
    close_translation(app);
}

unsafe fn close_translation(app: &mut App) {
    ShowWindow(app.result_window, SW_HIDE);
    manual_ui::leave(app);
    app.inline_positioned = false;
    app.result_positioned = false;
    app.window_pinned = false;
    conversation_ui::reset(app);
    ShowWindow(app.inline_window, SW_HIDE);
    app.inline_active = false;
    app.streaming = false;
    app.generation += 1;
    app.latest_translation
        .store(app.generation, Ordering::Release);
    cancel_automatic(app);
    refresh_translation_actions(app);
}

unsafe fn begin_screen_pick(app: &mut App) {
    if app.picking_color {
        return;
    }
    cancel_automatic(app);
    app.picking_color = true;
    EnableWindow(GetDlgItem(app.appearance_window, ID_SCREEN_PICK as i32), 0);
    set_text(
        app.appearance_ui.status,
        "点击屏幕取色；Esc 或右键取消。取色后仍需应用保存。",
    );
    let sender = app.ui_events.clone();
    let destination = app.main as isize;
    let started = thread::Builder::new()
        .name("screen-color".into())
        .spawn(move || {
            let result = picker::pick_screen_color();
            let _ = sender.send(Event::PickedColor(result));
            PostMessageW(destination as HWND, WM_WORK, 0, 0);
        });
    if started.is_err() {
        app.picking_color = false;
        EnableWindow(GetDlgItem(app.appearance_window, ID_SCREEN_PICK as i32), 1);
        set_text(app.appearance_ui.status, "无法启动取色，请重试。");
    }
}

unsafe fn selected_mode(app: &App) -> config::SelectionMode {
    if SendMessageW(app.settings_ui.mode, CB_GETCURSEL, 0, 0) == 1 {
        config::SelectionMode::Direct
    } else {
        config::SelectionMode::Click
    }
}

unsafe fn update_mode_hint(app: &App) {
    EnableWindow(app.settings_ui.mode, AUTO_CHECKED.get() as i32);
    EnableWindow(app.settings_ui.compatibility, AUTO_CHECKED.get() as i32);
    set_text(app.settings_ui.status, "");
}

unsafe fn request_capture(app: &mut App, kind: CaptureKind, point: POINT) {
    if kind != CaptureKind::Auto {
        conversation_ui::reset(app);
        cancel_automatic(app);
    }
    let foreground = GetForegroundWindow();
    if kind != CaptureKind::Clipboard && is_our_window(foreground) {
        return;
    }
    let job = CaptureJob {
        generation: app.capture_generation,
        kind,
        foreground: foreground as isize,
        point,
        origin: if kind == CaptureKind::Auto {
            app.auto_origin
        } else {
            None
        },
        compatibility: kind == CaptureKind::Auto
            && app.auto_strong
            && app.settings.compatibility_capture
            && !app.compatibility_attempted,
        copy_attempted: false,
        compatibility_checked: false,
        copy_retryable: false,
        clipboard_sequence: GetClipboardSequenceNumber(),
    };
    if app.capture_busy {
        app.pending = Some(job);
        return;
    }
    app.capture_busy = true;
    if app.capture.send(CaptureWork::Capture(job)).is_err() {
        app.capture_busy = false;
    }
}

unsafe fn drain_events(app: &mut App) {
    while let Ok(event) = app.events.try_recv() {
        match event {
            Event::Conversation(event) => conversation_ui::handle_event(app, event),
            Event::Speech(event) => handle_speech(app, event),
            Event::Warmed => {
                app.warm_busy = false;
                warm_foreground(app, GetForegroundWindow() as isize);
            }
            Event::Captured { job, result } => {
                app.capture_busy = false;
                if job.generation == app.capture_generation {
                    app.compatibility_attempted |= job.copy_attempted;
                }
                if job.generation == app.capture_generation
                    && (job.kind == CaptureKind::Clipboard
                        || GetForegroundWindow() as isize == job.foreground)
                {
                    match result {
                        Ok(captured) if crate::selection_text::has_visible_text(&captured.text) => {
                            if captured.text.chars().count() > translation::MAX_TEXT_CHARS {
                                if job.kind == CaptureKind::Auto {
                                    cancel_automatic(app);
                                } else {
                                    show_error(app, "选区过长，请缩小选择范围后重试。", job.point);
                                }
                            } else if job.kind == CaptureKind::Auto && automatic_enabled(app) {
                                KillTimer(app.main, TIMER_SELECTION);
                                app.chip_foreground = job.foreground;
                                // Notification/probe duplicates within one gesture
                                // are harmless. A later physical gesture always
                                // performs the normal cache/TTL/request binding.
                                if app.auto_text.as_deref() != Some(captured.text.trim()) {
                                    app.auto_text = Some(captured.text.trim().to_owned());
                                    if app.settings.selection_mode == config::SelectionMode::Direct
                                        || app.pending_icon_click
                                    {
                                        start_translation_in(
                                            app,
                                            captured.text,
                                            captured.source,
                                            job.point,
                                            true,
                                        );
                                    } else {
                                        app.chip_text = captured.text;
                                        if !app.window_pinned {
                                            ShowWindow(app.inline_window, SW_HIDE);
                                        }
                                        let _ = show_chip(app, job.point);
                                    }
                                }
                            } else if job.kind != CaptureKind::Auto {
                                start_translation(app, captured.text, captured.source, job.point);
                            }
                        }
                        Ok(_) => {
                            if job.kind == CaptureKind::Auto && app.auto_text.is_none() {
                                cancel_automatic(app);
                            }
                        }
                        Err(_) if job.kind == CaptureKind::Auto && app.auto_text.is_some() => {}
                        Err(error) if job.kind != CaptureKind::Auto => {
                            show_error(app, &error, job.point)
                        }
                        Err(error) => {
                            if job.compatibility_checked && job.copy_retryable {
                                retry_automatic(app);
                            } else if job.compatibility_checked
                                || selection::is_no_selection_error(&error)
                                || selection::is_protected_error(&error)
                            {
                                cancel_automatic(app);
                            } else {
                                retry_automatic(app);
                            }
                        }
                    }
                }
                if let Some(mut pending) = app.pending.take() {
                    if pending.generation == app.capture_generation
                        && (GetForegroundWindow() as isize == pending.foreground
                            || pending.kind == CaptureKind::Clipboard)
                    {
                        pending.compatibility &= !app.compatibility_attempted;
                        pending.clipboard_sequence = GetClipboardSequenceNumber();
                        app.capture_busy = true;
                        if app.capture.send(CaptureWork::Capture(pending)).is_err() {
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
                            app.translated = text.clone();
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
                            if !app.display_pending {
                                render_translation(app, &text);
                                translation_status(app, &status);
                                translation_copy_enabled(app, true);
                            }
                        }
                        Err(error) => {
                            if !app.display_pending && !translation::is_cancelled_error(&error) {
                                app.rendered_text.clear();
                                set_text(app.result_ui.translated, &error);
                                set_text(app.inline_ui.translated, &error);
                                translation_status(app, "翻译失败，请检查设置");
                            }
                        }
                    }
                }
            }
            Event::Progress { generation, text } => {
                if generation == app.generation && app.streaming {
                    app.translated = text.clone();
                    if !app.display_pending {
                        render_translation(app, &text);
                        translation_status(app, "正在翻译 · 实时显示");
                    }
                }
            }
            Event::PickedColor(result) => {
                app.picking_color = false;
                EnableWindow(GetDlgItem(app.appearance_window, ID_SCREEN_PICK as i32), 1);
                match result {
                    Ok(Some(color)) => {
                        DRAFT_ICON_COLOR.set(color);
                        refresh_icon_preview(app);
                    }
                    Ok(None) => set_text(app.appearance_ui.status, ""),
                    Err(error) => set_text(app.appearance_ui.status, &error),
                }
            }
            Event::UpdateChecked { manual, result } => update_ui::checked(app, manual, result),
            Event::UpdateProgress { received, total } => {
                let percent = received.saturating_mul(100).checked_div(total).unwrap_or(0);
                set_text(
                    app.settings_ui.update_status,
                    &format!("下载 {}%", percent.min(100)),
                );
            }
            Event::UpdateDownloaded(result) => update_ui::downloaded(app, result),
            Event::UpdateLaunched(result) => update_ui::launched(app, result),
        }
    }
}

unsafe fn start_translation(app: &mut App, text: String, source: &str, point: POINT) {
    manual_ui::leave(app);
    start_translation_in(app, text, source, point, false);
}

unsafe fn start_translation_in(
    app: &mut App,
    text: String,
    source: &str,
    point: POINT,
    inline: bool,
) {
    start_translation_request(app, text, source, point, inline, false);
}

unsafe fn start_translation_request(
    app: &mut App,
    text: String,
    source: &str,
    point: POINT,
    inline: bool,
    force_refresh: bool,
) {
    conversation_ui::reset(app);
    app.display_pending = false;
    EnableWindow(GetDlgItem(app.inline_window, ID_INLINE_EXPAND as i32), 1);
    if !force_refresh
        && app.streaming
        && app.active_epoch == app.settings_epoch
        && app.active_text == text.trim()
    {
        if app.translated.is_empty() {
            app.rendered_text.clear();
            set_text(app.result_ui.translated, "正在翻译…");
            set_text(app.inline_ui.translated, "正在翻译…");
        } else {
            let text = app.translated.clone();
            render_translation(app, &text);
        }
        translation_status(app, &format!("{source} · 正在翻译"));
        translation_copy_enabled(app, false);
        present_translation(app, inline, point);
        return;
    }
    app.generation += 1;
    app.latest_translation
        .store(app.generation, Ordering::Release);
    ShowWindow(app.chip, SW_HIDE);
    app.translated.clear();
    app.rendered_text.clear();
    app.active_text = text.trim().to_owned();
    app.active_epoch = app.settings_epoch;
    app.active_started = Some(Instant::now());
    app.streaming = true;
    translation_copy_enabled(app, false);
    set_text(app.result_ui.original, &text);
    set_text(app.result_ui.translated, "正在翻译…");
    set_text(app.inline_ui.translated, "正在翻译…");
    translation_status(app, &format!("{source} · 正在翻译"));
    present_translation(app, inline, point);
    let cached = if force_refresh {
        None
    } else {
        app.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&app.settings, &text)
    };
    if let Some(cached) = cached {
        app.streaming = false;
        render_translation(app, &cached);
        let elapsed = app
            .active_started
            .take()
            .map(|start| start.elapsed())
            .unwrap_or_default();
        translation_status(
            app,
            &format!("本地缓存 · {} ms · 未调用 API", elapsed.as_millis()),
        );
        translation_copy_enabled(app, true);
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
                force_refresh,
            },
        )
        .is_err()
    {
        set_text(app.result_ui.status, "翻译线程已停止，请重启应用。");
    }
}

unsafe fn show_error(app: &mut App, error: &str, point: POINT) {
    if app.manual_mode {
        translation_status(app, error);
        ShowWindow(app.result_window, SW_RESTORE);
        SetForegroundWindow(app.result_window);
        return;
    }
    app.active_text.clear();
    conversation_ui::reset(app);
    app.display_pending = false;
    app.streaming = false;
    app.generation += 1;
    app.latest_translation
        .store(app.generation, Ordering::Release);
    app.translated.clear();
    app.rendered_text.clear();
    translation_copy_enabled(app, false);
    set_text(app.result_ui.original, "未获取到可翻译的选区");
    set_text(app.result_ui.translated, error);
    set_text(
        app.result_ui.status,
        "也可手动复制文字，再右击托盘选择“翻译剪贴板”",
    );
    if !app.result_positioned {
        position_near(app.result_window, point, RESULT_WIDTH, RESULT_HEIGHT);
        app.result_positioned = true;
    }
    layout_result(app);
    ShowWindow(app.result_window, SW_SHOWNOACTIVATE);
}

unsafe fn render_translation(app: &mut App, text: &str) {
    app.translated = text.to_owned();
    let display = conversation_ui::display_text(app);
    let previous = native_text(&app.rendered_text);
    let current = native_text(&display);
    if current != previous {
        for editor in [app.result_ui.translated, app.inline_ui.translated] {
            render_editor(editor, &previous, &current, previous.is_empty());
        }
    }
    app.rendered_text = display;
    conversation_ui::refresh(app);
    refresh_translation_actions(app);
}

unsafe fn translation_status(app: &App, message: &str) {
    set_text(app.result_ui.status, message);
    set_text(app.inline_ui.status, message);
    refresh_translation_actions(app);
}

unsafe fn translation_copy_enabled(app: &mut App, enabled: bool) {
    app.translation_ready = enabled;
    EnableWindow(app.result_ui.copy, enabled as i32);
    EnableWindow(app.inline_ui.copy, enabled as i32);
    conversation_ui::refresh(app);
    refresh_translation_actions(app);
}

unsafe fn show_settings(app: &mut App) {
    cancel_automatic(app);
    if IsWindowVisible(app.speech_window) != 0 {
        SetForegroundWindow(app.speech_window);
        return;
    }
    SetWindowPos(
        app.result_window,
        HWND_NOTOPMOST,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    if IsWindowVisible(app.appearance_window) != 0 {
        SetForegroundWindow(app.appearance_window);
        return;
    }
    if IsWindowVisible(app.settings_window) == 0 {
        SETTINGS_PAGE.set(SettingsPage::Service);
        app.settings_scroll = 0;
        fill_settings(app);
    }
    let mut settings_rect: RECT = zeroed();
    GetWindowRect(app.settings_window, &mut settings_rect);
    let scale = dpi(app.settings_window) as i32;
    position_centered(
        app.settings_window,
        ((settings_rect.right - settings_rect.left) * 96 / scale).max(600),
        ((settings_rect.bottom - settings_rect.top) * 96 / scale).max(480),
    );
    layout_settings(app);
    ShowWindow(app.settings_window, SW_RESTORE);
    SetForegroundWindow(app.settings_window);
}

unsafe fn update_preset_controls(app: &App) {
    for id in [
        ID_PRESET_OCEAN,
        ID_PRESET_AURORA,
        ID_PRESET_NEBULA,
        ID_PRESET_AMBER,
    ] {
        if let Some(preset) = preset_for_control(id) {
            let label = if preset == app.settings.theme_preset {
                format!("{}，已选择", preset.label())
            } else {
                preset.label().to_owned()
            };
            let control = GetDlgItem(app.settings_window, id as i32);
            set_text(control, &label);
            InvalidateRect(control, null(), 0);
        }
    }
}

unsafe fn toggle_theme(app: &mut App) {
    let mode = if app.settings.theme_mode == config::ThemeMode::Dark {
        config::ThemeMode::Light
    } else {
        config::ThemeMode::Dark
    };
    apply_visual_theme(app, mode, app.settings.theme_preset);
}

unsafe fn apply_visual_theme(app: &mut App, mode: config::ThemeMode, preset: config::ThemePreset) {
    let next = match theme::Theme::new(mode, preset) {
        Ok(next) => next,
        Err(error) => {
            enqueue_notice(app, Notice::Error(error));
            return;
        }
    };
    let mut saved = app.settings.clone();
    saved.theme_mode = mode;
    saved.theme_preset = preset;
    if let Err(error) = config::save(&saved) {
        enqueue_notice(app, Notice::Error(error));
        return;
    }
    app.settings = saved;
    CURRENT_THEME.set(mode);
    CURRENT_PRESET.set(preset);
    update_preset_controls(app);
    surfaces::set_palette(&next);
    let previous = THEME.with(|cell| cell.replace(Some(next)));
    for window in [
        app.settings_window,
        app.result_window,
        app.inline_window,
        app.appearance_window,
        app.speech_window,
    ] {
        theme::apply_window(window, mode, preset);
        EnumChildWindows(window, Some(retheme_child), 0);
        redraw_layout(window);
    }
    drop(previous);
    set_text(
        GetDlgItem(app.settings_window, ID_THEME as i32),
        if mode == config::ThemeMode::Dark {
            "切换到白天模式"
        } else {
            "切换到暗夜模式"
        },
    );
    set_text(app.settings_ui.status, "");
}

unsafe extern "system" fn retheme_child(window: HWND, _data: LPARAM) -> BOOL {
    let mut class = [0u16; 64];
    let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
    let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
    if class.eq_ignore_ascii_case(rich_text::CLASS) {
        theme::style_edit(window, CURRENT_THEME.get());
        rich_text::restyle(window, CURRENT_THEME.get(), CURRENT_PRESET.get());
        surfaces::layout_control(window);
    } else if class.eq_ignore_ascii_case("EDIT") {
        theme::style_edit(window, CURRENT_THEME.get());
        surfaces::layout_control(window);
    } else if class.eq_ignore_ascii_case("COMBOBOX") {
        surfaces::install_combo(window);
    }
    1
}

unsafe fn refresh_window_icons(app: &mut App, window: HWND) {
    let key = (
        dpi(window),
        app.settings.icon_style,
        app.settings.icon_color,
        app.settings.icon_shape,
        app.settings.icon_size,
    );
    if app
        .window_icons
        .get(&(window as isize))
        .is_some_and(|(old, _)| *old == key)
    {
        return;
    }
    if let Ok(next) = icons::IconSet::new(key.1, key.2, key.3, key.4, key.0) {
        set_window_icons(window, &next);
        app.window_icons.insert(window as isize, (key, next));
    }
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

fn shape_index(shape: config::IconShape) -> usize {
    match shape {
        config::IconShape::Rounded => 0,
        config::IconShape::Square => 1,
        config::IconShape::Circle => 2,
        config::IconShape::Leaf => 3,
    }
}

unsafe fn selected_icon_shape(app: &App) -> config::IconShape {
    match SendMessageW(app.appearance_ui.shape, CB_GETCURSEL, 0, 0) {
        1 => config::IconShape::Square,
        2 => config::IconShape::Circle,
        3 => config::IconShape::Leaf,
        _ => config::IconShape::Rounded,
    }
}

unsafe fn selected_icon_size(app: &App) -> Result<u32, String> {
    let value: u32 = text(app.appearance_ui.size)
        .trim()
        .parse()
        .map_err(|_| "请输入 16–64 的整数尺寸。".to_string())?;
    if !(16..=64).contains(&value) {
        return Err("图标尺寸范围为 16–64 像素。".into());
    }
    Ok(value)
}

unsafe fn show_appearance(app: &mut App) {
    if IsWindowVisible(app.speech_window) != 0 {
        SetForegroundWindow(app.speech_window);
        return;
    }
    cancel_automatic(app);
    SendMessageW(
        app.appearance_ui.shape,
        CB_SETCURSEL,
        shape_index(app.settings.icon_shape),
        0,
    );
    set_text(app.appearance_ui.size, &app.settings.icon_size.to_string());
    SendMessageW(
        app.appearance_ui.style,
        CB_SETCURSEL,
        icon_style_index(app.settings.icon_style),
        0,
    );
    DRAFT_ICON_COLOR.set(app.settings.icon_color);
    if IsWindowVisible(app.settings_window) == 0 {
        show_settings(app);
    }
    EnableWindow(app.settings_window, 0);
    position_over_owner(
        app.appearance_window,
        app.settings_window,
        APPEARANCE_WIDTH,
        APPEARANCE_HEIGHT,
    );
    refresh_icon_preview(app);
    ShowWindow(app.appearance_window, SW_SHOW);
    SetForegroundWindow(app.appearance_window);
}

unsafe fn refresh_icon_preview(app: &mut App) {
    let size = match selected_icon_size(app) {
        Ok(size) => size,
        Err(error) => {
            set_text(app.appearance_ui.status, &error);
            return;
        }
    };
    match icons::IconSet::new(
        selected_icon_style(app),
        DRAFT_ICON_COLOR.get(),
        selected_icon_shape(app),
        size,
        dpi(app.appearance_window),
    ) {
        Ok(preview) => {
            SendMessageW(
                app.appearance_ui.preview,
                STM_SETICON,
                preview.chip() as usize,
                0,
            );
            app.preview_icons = Some(preview);
            layout_appearance(app);
            set_text(
                app.appearance_ui.color,
                &format!("#{:06X}", DRAFT_ICON_COLOR.get()),
            );
            set_text(app.appearance_ui.status, "");
        }
        Err(error) => set_text(app.appearance_ui.status, &error),
    }
}

unsafe fn apply_appearance(app: &mut App) {
    let mut settings = app.settings.clone();
    settings.icon_style = selected_icon_style(app);
    settings.icon_color = DRAFT_ICON_COLOR.get();
    settings.icon_shape = selected_icon_shape(app);
    settings.icon_size = match selected_icon_size(app) {
        Ok(size) => size,
        Err(error) => {
            set_text(app.appearance_ui.status, &error);
            return;
        }
    };
    let next = match icons::IconSet::new(
        settings.icon_style,
        settings.icon_color,
        settings.icon_shape,
        settings.icon_size,
        GetDpiForSystem(),
    ) {
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
    app.settings = settings;
    for window in [
        app.main,
        app.settings_window,
        app.result_window,
        app.appearance_window,
    ] {
        refresh_window_icons(app, window);
    }
    app.tray.hIcon = next.small();
    let notified = Shell_NotifyIconW(NIM_MODIFY, &app.tray) != 0;
    app.chip_pixels = None;
    ShowWindow(app.chip, SW_HIDE);
    // All native users have been updated before the previous owned HICON drops.
    app.icons = next;
    update_appearance_summary(app);
    InvalidateRect(app.chip, null(), 0);
    set_text(
        app.appearance_ui.status,
        if notified {
            ""
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
    set_text(
        GetDlgItem(app.settings_window, ID_MANUAL_HOTKEY as i32),
        &app.settings.manual_hotkey.label(),
    );
    update_preset_controls(app);
    mask_api_key(app);
    update_pause_ui(app);
    for (control, languages, value) in [
        (
            app.settings_ui.source,
            config::SOURCE_LANGUAGES,
            app.settings.source_language,
        ),
        (
            app.settings_ui.target,
            config::TARGET_LANGUAGES,
            app.settings.target_language,
        ),
    ] {
        let index = languages
            .iter()
            .position(|language| *language == value)
            .unwrap_or(0);
        SendMessageW(control, CB_SETCURSEL, index, 0);
    }
    update_language_labels(app);
    update_appearance_summary(app);
    update_swap_button(app);
    set_text(
        GetDlgItem(app.settings_window, ID_THEME as i32),
        if app.settings.theme_mode == config::ThemeMode::Dark {
            "切换到白天模式"
        } else {
            "切换到暗夜模式"
        },
    );
    set_text(app.settings_ui.base, &app.settings.base_url);
    set_text(app.settings_ui.key, &app.settings.api_key);
    set_text(app.settings_ui.model, &app.settings.model);
    set_text(
        app.settings_ui.timeout,
        &app.settings.timeout_seconds.to_string(),
    );
    AUTO_CHECKED.set(app.settings.selection_button);
    COMPATIBILITY_CHECKED.set(app.settings.compatibility_capture);
    InvalidateRect(app.settings_ui.auto, null(), 0);
    InvalidateRect(app.settings_ui.compatibility, null(), 0);
    SendMessageW(
        app.settings_ui.mode,
        CB_SETCURSEL,
        if app.settings.selection_mode == config::SelectionMode::Direct {
            1
        } else {
            0
        },
        0,
    );
    update_mode_hint(app);
    let provider = provider_index(&app.settings.base_url);
    SendMessageW(app.settings_ui.provider, CB_SETCURSEL, provider, 0);
}

unsafe fn read_settings(app: &App) -> Result<config::Settings, String> {
    let settings = config::Settings {
        manual_hotkey: config::Hotkey::parse(&text(GetDlgItem(
            app.settings_window,
            ID_MANUAL_HOTKEY as i32,
        )))?,
        base_url: text(app.settings_ui.base).trim().into(),
        api_key: text(app.settings_ui.key).trim().into(),
        model: text(app.settings_ui.model).trim().into(),
        timeout_seconds: text(app.settings_ui.timeout)
            .trim()
            .parse()
            .map_err(|_| "超时秒数必须是整数。".to_string())?,
        source_language: selected_language(app.settings_ui.source, config::SOURCE_LANGUAGES)?,
        target_language: selected_language(app.settings_ui.target, config::TARGET_LANGUAGES)?,
        selection_button: AUTO_CHECKED.get(),
        selection_mode: selected_mode(app),
        compatibility_capture: COMPATIBILITY_CHECKED.get(),
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
        ID_MANUAL_WINDOW as usize,
        wide("手动翻译").as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_SETTINGS as usize,
        wide("设置…").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(
        menu,
        MF_STRING
            | if app.settings.selection_button {
                0
            } else {
                MF_GRAYED
            },
        ID_PAUSE as usize,
        wide(if app.selection_paused {
            "恢复自动划词"
        } else {
            "暂停自动划词"
        })
        .as_ptr(),
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
    PostMessageW(app.main, WM_WORK, 0, 0);
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

// EDIT handles WM_CONTEXTMENU itself; it will not propagate the native menu
// request to the parent the way a button does. Route only translated outputs,
// keeping ordinary editing behavior and keyboard selection/copy unchanged.
unsafe extern "system" fn translation_edit_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_CONTEXTMENU {
        return SendMessageW(GetParent(window), message, window as usize, lparam);
    }
    if message == WM_NCDESTROY {
        rich_text::cleanup(window);
        RemoveWindowSubclass(window, Some(translation_edit_proc), subclass_id);
    }
    DefSubclassProc(window, message, wparam, lparam)
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
    if class == "EDIT" || class == rich_text::CLASS {
        if matches!(id, 224 | 251) {
            SetWindowSubclass(hwnd, Some(translation_edit_proc), 1, 0);
        }
        SendMessageW(hwnd, EM_SETLIMITTEXT, 32768, 0);
        SendMessageW(
            hwnd,
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            10 | (10 << 16),
        );
        theme::style_edit(hwnd, CURRENT_THEME.get());
        surfaces::install_edit(hwnd);
        if class == rich_text::CLASS {
            rich_text::install(hwnd, CURRENT_THEME.get(), CURRENT_PRESET.get());
        }
    } else if class == "COMBOBOX" {
        surfaces::install_combo(hwnd);
    }
    hwnd
}

unsafe fn layout_chip(_app: &mut App) {}

unsafe fn redraw_layout(hwnd: HWND) {
    surfaces::round_window(hwnd);
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
    surfaces::layout_control(hwnd);
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

unsafe fn position_centered(hwnd: HWND, width: i32, height: i32) {
    let monitor = MonitorFromPoint(cursor(), MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        return;
    }
    let mut scale = 96;
    let mut y_dpi = 96;
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut scale, &mut y_dpi);
    let work = info.rcWork;
    let w = scaled(width, scale).min(work.right - work.left);
    let h = scaled(height, scale).min(work.bottom - work.top);
    SetWindowPos(
        hwnd,
        HWND_TOP,
        work.left + (work.right - work.left - w) / 2,
        work.top + (work.bottom - work.top - h) / 2,
        w,
        h,
        SWP_NOACTIVATE,
    );
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
