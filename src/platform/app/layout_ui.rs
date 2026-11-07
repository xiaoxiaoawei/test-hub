//! Compact native layouts. State and commands stay in the parent module.
use super::super::surfaces;
use super::*;

const COMBO_STYLE: u32 =
    (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32 | WS_VSCROLL | WS_TABSTOP;

struct TooltipWindow {
    owner: HWND,
    window: HWND,
    // The tooltip control retains these pointers after TTM_ADDTOOLW returns.
    text: Vec<Box<[u16]>>,
}

impl Drop for TooltipWindow {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.window) != 0 {
                DestroyWindow(self.window);
            }
        }
    }
}

thread_local! {
    static TOOLTIPS: RefCell<Vec<TooltipWindow>> = const { RefCell::new(Vec::new()) };
    static COMMON_CONTROLS_READY: Cell<bool> = const { Cell::new(false) };
}

/// The text is owned for the native control's lifetime; no temporary UTF-16
/// pointers survive this call. Do not hold the storage borrow across Win32 calls.
pub(super) unsafe fn tooltip(parent: HWND, control: HWND, label: &str) {
    if parent.is_null() || control.is_null() {
        return;
    }
    if !COMMON_CONTROLS_READY.get() {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        if InitCommonControlsEx(&controls) == 0 {
            return;
        }
        COMMON_CONTROLS_READY.set(true);
    }
    let existing = TOOLTIPS.with(|storage| {
        storage
            .borrow()
            .iter()
            .find(|entry| entry.owner == parent && IsWindow(entry.window) != 0)
            .map(|entry| entry.window)
    });
    let window = if let Some(window) = existing {
        window
    } else {
        let window = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            parent,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if window.is_null() {
            return;
        }
        SendMessageW(
            window,
            TTM_SETMAXTIPWIDTH,
            0,
            scaled(320, dpi(parent)) as isize,
        );
        SendMessageW(window, TTM_SETDELAYTIME, TTDT_INITIAL as usize, 350);
        TOOLTIPS.with(|storage| {
            storage.borrow_mut().push(TooltipWindow {
                owner: parent,
                window,
                text: Vec::new(),
            });
        });
        window
    };
    let mut label = wide(label).into_boxed_slice();
    let pointer = label.as_mut_ptr();
    TOOLTIPS.with(|storage| {
        if let Some(entry) = storage
            .borrow_mut()
            .iter_mut()
            .find(|entry| entry.window == window)
        {
            entry.text.push(label);
        }
    });
    let mut tool: TTTOOLINFOW = zeroed();
    tool.cbSize = size_of::<TTTOOLINFOW>() as u32;
    tool.uFlags = TTF_IDISHWND | TTF_SUBCLASS | TTF_TRANSPARENT;
    tool.hwnd = parent;
    tool.uId = control as usize;
    tool.lpszText = pointer;
    SendMessageW(window, TTM_ADDTOOLW, 0, &tool as *const _ as isize);
}

unsafe fn icon_button(parent: HWND, label: &str, id: u16, help: &str) -> HWND {
    let control = child(parent, "BUTTON", label, WS_TABSTOP, id);
    tooltip(parent, control, help);
    control
}

unsafe fn combo(parent: HWND, id: u16, labels: impl IntoIterator<Item = &'static str>) -> HWND {
    let control = child(parent, "COMBOBOX", "", COMBO_STYLE, id);
    for label in labels {
        SendMessageW(control, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
    }
    control
}

unsafe fn question_input(parent: HWND) -> HWND {
    let control = child(
        parent,
        "EDIT",
        "",
        ES_AUTOHSCROLL as u32 | WS_TABSTOP,
        ID_QUESTION,
    );
    SendMessageW(
        control,
        EM_SETLIMITTEXT,
        translation::MAX_QUESTION_CHARS * 2,
        0,
    );
    SendMessageW(
        control,
        EM_SETCUEBANNER,
        1,
        wide("围绕这段文字提问…").as_ptr() as isize,
    );
    tooltip(parent, control, "围绕当前选区提问，Enter 发送");
    control
}

pub(super) unsafe fn make_settings(parent: HWND) -> SettingsUi {
    child(parent, "STATIC", "轻译", SS_LEFT, 200);
    icon_button(parent, "切换主题", ID_THEME, "切换白天 / 暗夜");
    child(parent, "STATIC", "翻译服务", SS_LEFT, 294);
    child(parent, "BUTTON", "翻译服务", WS_TABSTOP, ID_SERVICE_TAB);
    child(parent, "BUTTON", "偏好设置", WS_TABSTOP, ID_PREFERENCES_TAB);
    child(parent, "BUTTON", "外观", WS_TABSTOP, ID_APPEARANCE_TAB);
    child(parent, "STATIC", "服务", SS_LEFT, 202);
    let provider = combo(
        parent,
        ID_PROVIDER,
        ["DeepSeek", "OpenAI", "自定义兼容服务"],
    );
    child(parent, "STATIC", "API 密钥", SS_LEFT, 204);
    let key = child(
        parent,
        "EDIT",
        "",
        (ES_AUTOHSCROLL | ES_PASSWORD) as u32 | WS_TABSTOP,
        211,
    );
    icon_button(
        parent,
        "显示密钥",
        ID_KEY_VISIBILITY,
        "显示 / 隐藏 API 密钥",
    );
    child(parent, "STATIC", "服务地址", SS_LEFT, 203);
    let base = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 210);
    child(parent, "STATIC", "模型", SS_LEFT, 205);
    let model = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 212);
    icon_button(parent, "测试连接", ID_TEST, "测试模型连接");
    child(parent, "STATIC", "超时 / 秒", SS_LEFT, 206);
    let timeout = child(
        parent,
        "EDIT",
        "",
        (ES_NUMBER | ES_AUTOHSCROLL) as u32 | WS_TABSTOP,
        213,
    );
    child(parent, "BUTTON", "朗读接口", WS_TABSTOP, 303);
    child(parent, "STATIC", "源语言", SS_LEFT, 274);
    let source = combo(
        parent,
        ID_SOURCE_LANGUAGE,
        config::SOURCE_LANGUAGES
            .iter()
            .map(|language| language.label()),
    );
    child(parent, "STATIC", "目标语言", SS_LEFT, 275);
    let target = combo(
        parent,
        ID_TARGET_LANGUAGE,
        config::TARGET_LANGUAGES
            .iter()
            .map(|language| language.label()),
    );
    icon_button(
        parent,
        "交换语言",
        ID_SWAP_LANGUAGES,
        "交换源语言与目标语言",
    );
    child(parent, "STATIC", "手动翻译快捷键", SS_LEFT, 300);
    let hotkey = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 301);
    tooltip(parent, hotkey, "打开手动翻译框，例如 Ctrl+Alt+Space");
    let auto = child(parent, "BUTTON", "自动划词", WS_TABSTOP, ID_AUTO);
    let mode = combo(
        parent,
        ID_SELECTION_MODE,
        ["点击图标后翻译", "直接显示译文"],
    );
    let compatibility = child(parent, "BUTTON", "兼容取词", WS_TABSTOP, ID_COMPATIBILITY);
    tooltip(
        parent,
        compatibility,
        "网页 / PDF 取词失败时临时复制并恢复剪贴板",
    );
    let pause = child(parent, "BUTTON", "暂停划词", WS_TABSTOP, ID_PAUSE);
    tooltip(parent, pause, "仅本次运行暂停自动划词");
    for (id, preset) in [
        (ID_PRESET_OCEAN, config::ThemePreset::Ocean),
        (ID_PRESET_AURORA, config::ThemePreset::Aurora),
        (ID_PRESET_NEBULA, config::ThemePreset::Nebula),
        (ID_PRESET_AMBER, config::ThemePreset::Amber),
    ] {
        let control = child(parent, "BUTTON", preset.label(), WS_TABSTOP, id);
        tooltip(parent, control, &format!("{} · 点击应用", preset.label()));
    }
    child(parent, "BUTTON", "划词图标", WS_TABSTOP, ID_APPEARANCE);
    child(parent, "BUTTON", "检查更新", WS_TABSTOP, ID_UPDATE);
    let update_status = child(
        parent,
        "STATIC",
        &format!("v{}", env!("CARGO_PKG_VERSION")),
        SS_LEFT,
        219,
    );
    let status = child(parent, "STATIC", "", SS_LEFT, 215);
    child(parent, "BUTTON", "保存", WS_TABSTOP, ID_SAVE);
    SettingsUi {
        source,
        target,
        provider,
        base,
        key,
        model,
        timeout,
        auto,
        compatibility,
        mode,
        status,
        update_status,
    }
}

pub(super) struct SpeechSettingsUi {
    pub(super) base: HWND,
    pub(super) key: HWND,
    pub(super) model: HWND,
    pub(super) voice: HWND,
    pub(super) status: HWND,
}

pub(super) unsafe fn make_speech_settings(parent: HWND) -> SpeechSettingsUi {
    child(parent, "STATIC", "服务地址", SS_LEFT, 311);
    let base = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 304);
    child(parent, "STATIC", "API 密钥", SS_LEFT, 314);
    let key = child(
        parent,
        "EDIT",
        "",
        (ES_AUTOHSCROLL | ES_PASSWORD) as u32 | WS_TABSTOP,
        307,
    );
    child(parent, "STATIC", "模型", SS_LEFT, 312);
    let model = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 305);
    child(parent, "STATIC", "声音", SS_LEFT, 313);
    let voice = child(parent, "EDIT", "", ES_AUTOHSCROLL as u32 | WS_TABSTOP, 306);
    tooltip(parent, base, "OpenAI 兼容语音服务，也可接入远端 Kokoro");
    let status = child(parent, "STATIC", "", SS_LEFT, 310);
    child(parent, "BUTTON", "保存", WS_TABSTOP, 308);
    child(parent, "BUTTON", "取消", WS_TABSTOP, 309);
    SpeechSettingsUi {
        base,
        key,
        model,
        voice,
        status,
    }
}

unsafe fn input_margins(control: HWND, scale: u32) {
    let inset = scaled(10, scale) as isize;
    SendMessageW(
        control,
        EM_SETMARGINS,
        (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
        inset | (inset << 16),
    );
}

pub(super) unsafe fn layout_speech_settings(hwnd: HWND) {
    let scale = dpi(hwnd);
    let mut bounds: RECT = zeroed();
    GetClientRect(hwnd, &mut bounds);
    let width = bounds.right * 96 / scale as i32;
    let height = bounds.bottom * 96 / scale as i32;
    let inner = (width - 48).max(260);
    let half = (inner - 16) / 2;
    let positions = [
        (311, 24, 20, inner, 20),
        (304, 24, 46, inner, 40),
        (314, 24, 104, inner, 20),
        (307, 24, 130, inner, 40),
        (312, 24, 188, half, 20),
        (313, 40 + half, 188, half, 20),
        (305, 24, 214, half, 40),
        (306, 40 + half, 214, half, 40),
        (310, 24, 274, inner, 36),
        (309, width - 264, height - 60, 112, 36),
        (308, width - 136, height - 60, 112, 36),
    ];
    for (id, x, y, w, h) in positions {
        let face = font(
            scale,
            if matches!(id, 310..=314) { 13 } else { 16 },
            FW_NORMAL as i32,
        );
        let control = GetDlgItem(hwnd, id);
        place(control, x, y, w, h, scale, face);
        if matches!(id, 304..=307) {
            input_margins(control, scale);
        }
    }
    redraw_layout(hwnd);
}

unsafe fn page_visible(parent: HWND, ids: &[u16], visible: bool) {
    for &id in ids {
        let control = GetDlgItem(parent, id as i32);
        let currently_visible = GetWindowLongPtrW(control, GWL_STYLE) & WS_VISIBLE as isize != 0;
        if visible != currently_visible {
            ShowWindow(control, if visible { SW_SHOWNA } else { SW_HIDE });
        }
    }
}

unsafe fn mark_cards(parent: HWND, ids: &[u16]) {
    let mut control = GetWindow(parent, GW_CHILD);
    while !control.is_null() {
        surfaces::set_card_control(control, ids.contains(&(GetDlgCtrlID(control) as u16)));
        control = GetWindow(control, GW_HWNDNEXT);
    }
}

/// Logical geometry shared by layout and the parent background painter.
pub(super) fn settings_geometry(width: i32) -> (i32, i32, i32) {
    let rail = if width < 680 { 132 } else { 156 };
    let left = rail + 24;
    (rail, left, (width - left - 24).max(280))
}

pub(super) fn settings_panels(width: i32, page: SettingsPage) -> Vec<RECT> {
    let (_, left, main_width) = settings_geometry(width);
    let content = main_width - 40;
    let rect = |top, bottom| RECT {
        left,
        top,
        right: left + main_width,
        bottom,
    };
    match page {
        SettingsPage::Service => vec![rect(88, 420)],
        SettingsPage::Preferences => vec![
            rect(88, 280),
            rect(296, if content < 360 { 466 } else { 420 }),
        ],
        SettingsPage::Appearance => Vec::new(),
    }
}

pub(super) unsafe fn layout_settings(app: &mut App) {
    let hwnd = app.settings_window;
    refresh_window_icons(app, hwnd);
    let scale = dpi(hwnd);
    let face = font(scale, 16, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let height = rect.bottom * 96 / scale as i32;
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
    SetScrollInfo(hwnd, SB_VERT, &scroll, 0);
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / scale as i32;
    let (rail, left, main_width) = settings_geometry(width);
    let content = main_width - 40;
    let x = left + 20;
    let right = left + main_width;
    let language_width = (content - 52) / 2;
    let template_width = (main_width - 16) / 2;
    let page = SETTINGS_PAGE.get();
    page_visible(
        hwnd,
        &[
            202,
            ID_PROVIDER,
            204,
            211,
            ID_KEY_VISIBILITY,
            203,
            210,
            205,
            212,
            ID_TEST,
            206,
            213,
            303,
        ],
        page == SettingsPage::Service,
    );
    page_visible(
        hwnd,
        &[
            274,
            ID_SOURCE_LANGUAGE,
            275,
            ID_TARGET_LANGUAGE,
            ID_SWAP_LANGUAGES,
            300,
            301,
            ID_AUTO,
            ID_SELECTION_MODE,
            ID_COMPATIBILITY,
            ID_PAUSE,
        ],
        page == SettingsPage::Preferences,
    );
    page_visible(
        hwnd,
        &[
            ID_PRESET_OCEAN,
            ID_PRESET_AURORA,
            ID_PRESET_NEBULA,
            ID_PRESET_AMBER,
            ID_APPEARANCE,
        ],
        page == SettingsPage::Appearance,
    );
    page_visible(
        hwnd,
        &[
            200,
            ID_THEME,
            ID_SERVICE_TAB,
            ID_PREFERENCES_TAB,
            ID_APPEARANCE_TAB,
            ID_UPDATE,
            294,
            215,
            ID_SAVE,
        ],
        true,
    );
    page_visible(hwnd, &[219], height >= 400);
    mark_cards(
        hwnd,
        &[
            200,
            ID_THEME,
            ID_SERVICE_TAB,
            ID_PREFERENCES_TAB,
            ID_APPEARANCE_TAB,
            ID_UPDATE,
            219,
            202,
            ID_PROVIDER,
            204,
            211,
            ID_KEY_VISIBILITY,
            203,
            210,
            205,
            212,
            ID_TEST,
            206,
            213,
            274,
            ID_SOURCE_LANGUAGE,
            275,
            ID_TARGET_LANGUAGE,
            ID_SWAP_LANGUAGES,
            300,
            301,
            ID_AUTO,
            ID_SELECTION_MODE,
            ID_COMPATIBILITY,
            ID_PAUSE,
        ],
    );
    set_text(
        GetDlgItem(hwnd, 294),
        match page {
            SettingsPage::Service => "翻译服务",
            SettingsPage::Preferences => "偏好设置",
            SettingsPage::Appearance => "外观",
        },
    );
    let compact = height < 400;
    let nav_start = if compact { 76 } else { 84 };
    let nav_step = if compact { 40 } else { 48 };
    let nav_height = if compact { 36 } else { 40 };
    let theme_y = if compact {
        (height - 100).max(204)
    } else {
        (height - 156).max(244)
    };
    let update_y = if compact {
        height - 48
    } else {
        (height - 104).max(296)
    };
    let logical_height = height.max(SETTINGS_CONTENT_HEIGHT);
    let narrow = content < 360;
    let mode_x = if narrow { x } else { x + 164 };
    let mode_y = if narrow { 350 } else { 312 };
    let mode_width = if narrow { content } else { content - 164 };
    let compatibility_y = if narrow { 414 } else { 370 };
    let positions = [
        (200, 20, 24, rail - 40, 30),
        (ID_SERVICE_TAB, 16, nav_start, rail - 32, nav_height),
        (
            ID_PREFERENCES_TAB,
            16,
            nav_start + nav_step,
            rail - 32,
            nav_height,
        ),
        (
            ID_APPEARANCE_TAB,
            16,
            nav_start + nav_step * 2,
            rail - 32,
            nav_height,
        ),
        (ID_THEME, 16, theme_y, 36, 36),
        (ID_UPDATE, 16, update_y, rail - 32, 36),
        (219, 20, height - 56, rail - 40, 40),
        (294, left, 24, main_width, 38),
        (202, x, 106, content, 20),
        (ID_PROVIDER, x, 132, content, 220),
        (204, x, 184, content, 20),
        (211, x, 208, content - 48, 40),
        (ID_KEY_VISIBILITY, right - 56, 210, 36, 36),
        (203, x, 260, content, 20),
        (210, x, 284, content, 40),
        (205, x, 336, content - 148, 20),
        (206, right - 104, 336, 84, 20),
        (212, x, 360, content - 148, 40),
        (ID_TEST, right - 156, 360, 40, 40),
        (213, right - 104, 360, 84, 40),
        (303, left, 436, 120, 36),
        (274, x, 108, language_width, 20),
        (275, x + language_width + 52, 108, language_width, 20),
        (ID_SOURCE_LANGUAGE, x, 134, language_width, 220),
        (ID_SWAP_LANGUAGES, x + language_width + 8, 136, 36, 36),
        (
            ID_TARGET_LANGUAGE,
            x + language_width + 52,
            134,
            language_width,
            220,
        ),
        (300, x, 190, content, 20),
        (301, x, 216, content, 40),
        (ID_AUTO, x, 316, 148, 28),
        (ID_SELECTION_MODE, mode_x, mode_y, mode_width, 190),
        (ID_COMPATIBILITY, x, compatibility_y, content - 128, 30),
        (ID_PAUSE, right - 132, compatibility_y - 4, 112, 36),
        (ID_PRESET_OCEAN, left, 88, template_width, 90),
        (
            ID_PRESET_AURORA,
            left + template_width + 16,
            88,
            template_width,
            90,
        ),
        (ID_PRESET_NEBULA, left, 192, template_width, 90),
        (
            ID_PRESET_AMBER,
            left + template_width + 16,
            192,
            template_width,
            90,
        ),
        (ID_APPEARANCE, left, 304, main_width, 44),
        (215, left, logical_height - 112, main_width, 32),
        (ID_SAVE, right - 112, logical_height - 64, 112, 40),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = match id {
            200 => font(scale, 20, FW_SEMIBOLD as i32),
            294 => font(scale, 26, FW_SEMIBOLD as i32),
            ID_SERVICE_TAB | ID_PREFERENCES_TAB | ID_APPEARANCE_TAB => {
                font(scale, 15, FW_NORMAL as i32)
            }
            202..=206 | 215 | 219 | 274 | 275 | 300 => font(scale, 13, FW_NORMAL as i32),
            _ => face,
        };
        let sidebar = matches!(
            id,
            200 | ID_THEME
                | ID_SERVICE_TAB
                | ID_PREFERENCES_TAB
                | ID_APPEARANCE_TAB
                | ID_UPDATE
                | 219
        );
        let control = GetDlgItem(hwnd, id as i32);
        place(
            control,
            x,
            if sidebar { y } else { y - app.settings_scroll },
            w,
            h,
            scale,
            control_font,
        );
        if id == 301 {
            input_margins(control, scale);
        }
    }
    redraw_layout(hwnd);
}

pub(super) unsafe fn make_appearance(parent: HWND) -> AppearanceUi {
    child(parent, "STATIC", "图标外观", SS_LEFT, 240);
    child(parent, "STATIC", "样式", SS_LEFT, 242);
    let style = combo(parent, ID_ICON_STYLE, ["叶片", "译字", "字母 A"]);
    child(parent, "STATIC", "形状", SS_LEFT, 245);
    let shape = combo(parent, ID_ICON_SHAPE, ["圆角", "方形", "圆形", "叶形"]);
    child(parent, "STATIC", "尺寸", SS_LEFT, 246);
    let size = child(
        parent,
        "EDIT",
        "32",
        (ES_NUMBER | ES_AUTOHSCROLL) as u32 | WS_TABSTOP,
        ID_ICON_SIZE,
    );
    let preview = child(
        parent,
        "STATIC",
        "",
        SS_ICON | SS_CENTERIMAGE | SS_REALSIZECONTROL,
        234,
    );
    child(parent, "STATIC", "颜色", SS_LEFT, 243);
    let color = child(parent, "BUTTON", "选择颜色…", WS_TABSTOP, ID_ICON_COLOR);
    child(parent, "BUTTON", "屏幕取色", WS_TABSTOP, ID_SCREEN_PICK);
    let status = child(parent, "STATIC", "", SS_LEFT, 235);
    child(parent, "BUTTON", "恢复默认", WS_TABSTOP, ID_ICON_RESET);
    child(parent, "BUTTON", "应用", WS_TABSTOP, ID_ICON_APPLY);
    AppearanceUi {
        style,
        shape,
        size,
        preview,
        color,
        status,
    }
}

pub(super) unsafe fn layout_appearance(app: &mut App) {
    let hwnd = app.appearance_window;
    refresh_window_icons(app, hwnd);
    let scale = dpi(hwnd);
    let face = font(scale, 16, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / scale as i32;
    let height = rect.bottom * 96 / scale as i32;
    let half = (width - 88) / 2;
    let preview_size = app
        .preview_icons
        .as_ref()
        .map(|icon| icon.chip_size() * 96 / scale as i32)
        .unwrap_or(32)
        .clamp(16, 64);
    mark_cards(
        hwnd,
        &[
            242,
            ID_ICON_STYLE,
            245,
            ID_ICON_SHAPE,
            246,
            ID_ICON_SIZE,
            243,
            ID_ICON_COLOR,
            ID_SCREEN_PICK,
        ],
    );
    let positions = [
        (240, 24, 16, width - 120, 30),
        (
            234,
            width - 84 + (64 - preview_size) / 2,
            10 + (64 - preview_size) / 2,
            preview_size,
            preview_size,
        ),
        (242, 36, 98, half, 18),
        (ID_ICON_STYLE, 36, 122, half, 170),
        (245, 52 + half, 98, half, 18),
        (ID_ICON_SHAPE, 52 + half, 122, half, 170),
        (246, 36, 176, 104, 18),
        (ID_ICON_SIZE, 36, 200, 88, 34),
        (243, 144, 176, width - 180, 18),
        (ID_ICON_COLOR, 144, 200, width - 304, 34),
        (ID_SCREEN_PICK, width - 144, 200, 108, 34),
        (235, 24, height - 94, width - 48, 30),
        (ID_ICON_RESET, 24, height - 48, 104, 32),
        (ID_ICON_APPLY, width - 128, height - 48, 104, 32),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = match id {
            240 => font(scale, 22, FW_SEMIBOLD as i32),
            235 | 242 | 245 | 246 | 243 => font(scale, 13, FW_NORMAL as i32),
            _ => face,
        };
        place(GetDlgItem(hwnd, id as i32), x, y, w, h, scale, control_font);
    }
    redraw_layout(hwnd);
}

pub(super) unsafe fn make_inline(parent: HWND) -> InlineUi {
    let heading = child(parent, "STATIC", "英文 → 简体中文", SS_LEFT, 250);
    tooltip(parent, heading, "拖动标题可移动译文框");
    let translated = child(
        parent,
        super::super::rich_text::CLASS,
        "",
        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL | WS_TABSTOP,
        251,
    );
    // Long streamed answers and retained exchanges can exceed the native EDIT
    // default. This remains bounded by the transport's response limit.
    SendMessageW(translated, EM_SETLIMITTEXT, 2 * 1024 * 1024, 0);
    let status = child(parent, "STATIC", "正在翻译…", SS_LEFT, 254);
    let action = icon_button(parent, "重新翻译", ID_RETRY, "重新调用翻译服务 / 停止翻译");
    let pin = icon_button(
        parent,
        "保持窗口显示",
        ID_PIN,
        "切换软件时保持窗口显示；新划词仍更新译文",
    );
    let copy = icon_button(
        parent,
        "复制译文",
        ID_COPY,
        "复制译文；右键可复制选区或全部问答",
    );
    icon_button(parent, "展开", ID_INLINE_EXPAND, "展开翻译窗口");
    icon_button(parent, "关闭", ID_INLINE_CLOSE, "关闭翻译窗口");
    icon_button(parent, "朗读译文", ID_SPEAK, "朗读当前译文；再次点击停止");
    icon_button(
        parent,
        "检查润色",
        ID_POLISH,
        "调用当前模型检查译文，提供更自然的表达",
    );
    let question = question_input(parent);
    let send = icon_button(
        parent,
        "发送问题",
        ID_SEND,
        "发送问题 / 停止回答；Enter 发送",
    );
    InlineUi {
        action,
        pin,
        question,
        send,
        translated,
        status,
        copy,
    }
}

pub(super) unsafe fn layout_inline(app: &mut App) {
    let hwnd = app.inline_window;
    let scale = dpi(hwnd);
    let face = font(scale, 16, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / scale as i32;
    let height = rect.bottom * 96 / scale as i32;
    let positions = [
        (250, 20, 16, width - 112, 24),
        (ID_INLINE_EXPAND, width - 88, 12, 32, 32),
        (ID_INLINE_CLOSE, width - 48, 12, 32, 32),
        (ID_RETRY, 20, 56, 32, 32),
        (ID_PIN, 60, 56, 32, 32),
        (ID_COPY, 100, 56, 32, 32),
        (ID_SPEAK, 140, 56, 32, 32),
        (ID_POLISH, 180, 56, 32, 32),
        (251, 20, 100, width - 40, (height - 196).max(60)),
        (254, 20, height - 84, width - 40, 20),
        (ID_QUESTION, 20, height - 52, width - 80, 36),
        (ID_SEND, width - 52, height - 52, 32, 36),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = if id == 254 {
            font(scale, 13, FW_NORMAL as i32)
        } else {
            face
        };
        place(GetDlgItem(hwnd, id as i32), x, y, w, h, scale, control_font);
    }
    redraw_layout(hwnd);
}

pub(super) unsafe fn make_result(parent: HWND) -> ResultUi {
    child(parent, "STATIC", "英文 → 简体中文", SS_LEFT, 220);
    child(parent, "STATIC", "原文", SS_LEFT, 221);
    let original = child(
        parent,
        "EDIT",
        "",
        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL | WS_TABSTOP,
        222,
    );
    let manual_translate = child(parent, "BUTTON", "翻译", WS_TABSTOP, 302);
    ShowWindow(manual_translate, SW_HIDE);
    child(parent, "STATIC", "译文", SS_LEFT, 223);
    let translated = child(
        parent,
        super::super::rich_text::CLASS,
        "",
        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL | WS_TABSTOP,
        224,
    );
    SendMessageW(translated, EM_SETLIMITTEXT, 2 * 1024 * 1024, 0);
    let status = child(parent, "STATIC", "", SS_LEFT, 225);
    let action = icon_button(parent, "重新翻译", ID_RETRY, "重新调用翻译服务 / 停止翻译");
    let pin = icon_button(
        parent,
        "保持窗口显示",
        ID_PIN,
        "切换软件时保持窗口显示；新划词仍更新译文",
    );
    let copy = icon_button(
        parent,
        "复制译文",
        ID_COPY,
        "复制译文；右键可复制选区或全部问答",
    );
    icon_button(parent, "设置", ID_SETTINGS, "打开设置");
    icon_button(parent, "朗读译文", ID_SPEAK, "朗读当前译文；再次点击停止");
    icon_button(
        parent,
        "检查润色",
        ID_POLISH,
        "调用当前模型检查译文，提供更自然的表达",
    );
    let question = question_input(parent);
    let send = icon_button(
        parent,
        "发送问题",
        ID_SEND,
        "发送问题 / 停止回答；Enter 发送",
    );
    ResultUi {
        action,
        pin,
        question,
        send,
        original,
        translated,
        status,
        copy,
    }
}

pub(super) unsafe fn layout_result(app: &mut App) {
    let hwnd = app.result_window;
    refresh_window_icons(app, hwnd);
    let scale = dpi(hwnd);
    let face = font(scale, 16, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / scale as i32;
    let height = rect.bottom * 96 / scale as i32;
    let inner = width - 48;
    let positions = [
        (220, 24, 20, inner - 48, 28),
        (ID_SETTINGS, width - 56, 18, 32, 32),
        (ID_RETRY, 24, 64, 32, 32),
        (ID_PIN, 64, 64, 32, 32),
        (ID_COPY, 104, 64, 32, 32),
        (ID_SPEAK, 144, 64, 32, 32),
        (ID_POLISH, 184, 64, 32, 32),
        (221, 24, 112, inner - 116, 24),
        (302, width - 124, 106, 100, 36),
        (222, 24, 150, inner, 80),
        (223, 24, 246, inner, 20),
        (224, 24, 272, inner, (height - 364).max(64)),
        (225, 24, height - 78, inner, 20),
        (ID_QUESTION, 24, height - 48, inner - 44, 36),
        (ID_SEND, width - 56, height - 48, 32, 36),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = match id {
            220 => font(scale, 18, FW_SEMIBOLD as i32),
            221 | 223 | 225 => font(scale, 13, FW_NORMAL as i32),
            _ => face,
        };
        let control = GetDlgItem(hwnd, id as i32);
        place(control, x, y, w, h, scale, control_font);
        if id == 222 {
            input_margins(control, scale);
        }
    }
    redraw_layout(hwnd);
}
