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
    child(parent, "STATIC", "轻译  LingoLeaf", SS_LEFT, 200);
    icon_button(parent, "切换主题", ID_THEME, "切换白天 / 暗夜");
    child(
        parent,
        "STATIC",
        "英文 → 简体中文  ·  Ctrl + Alt + T",
        SS_LEFT,
        201,
    );
    child(parent, "BUTTON", "服务", WS_TABSTOP, ID_SERVICE_TAB);
    child(parent, "BUTTON", "偏好", WS_TABSTOP, ID_PREFERENCES_TAB);
    child(parent, "BUTTON", "外观", WS_TABSTOP, ID_APPEARANCE_TAB);
    child(parent, "STATIC", "翻译服务", SS_LEFT, 202);
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
    child(parent, "STATIC", "原始语言", SS_LEFT, 274);
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
        "交换原始语言与目标语言；自动检测时不可交换",
    );
    let auto = child(parent, "BUTTON", "自动划词", WS_TABSTOP, ID_AUTO);
    let mode = combo(
        parent,
        ID_SELECTION_MODE,
        ["点击图标后翻译", "直接显示译文"],
    );
    let compatibility = child(
        parent,
        "BUTTON",
        "兼容取词 · 网页 / PDF",
        WS_TABSTOP,
        ID_COMPATIBILITY,
    );
    let pause = child(parent, "BUTTON", "暂停划词", WS_TABSTOP, ID_PAUSE);
    tooltip(parent, pause, "仅本次运行暂停自动划词，快捷键仍可用");
    child(
        parent,
        "STATIC",
        "临时复制选区，并尝试恢复剪贴板。",
        SS_LEFT,
        218,
    );
    child(parent, "STATIC", "配色模板", SS_LEFT, 290);
    child(parent, "STATIC", "选择即应用 · 自动保存", SS_LEFT, 291);
    for (id, name, description) in [
        (ID_PRESET_OCEAN, "深海蓝", "沉静蓝调 · 清晰专注"),
        (ID_PRESET_AURORA, "极光青", "冷冽青绿 · 轻盈明亮"),
        (ID_PRESET_NEBULA, "星云紫", "柔和紫调 · 灵感空间"),
        (ID_PRESET_AMBER, "暖砂金", "温润琥珀 · 舒适阅读"),
    ] {
        let control = child(
            parent,
            "BUTTON",
            &format!("{name} · {description}"),
            WS_TABSTOP,
            id,
        );
        tooltip(
            parent,
            control,
            &format!("{name} · 点击应用并保存，支持白天与暗夜模式"),
        );
    }
    child(parent, "BUTTON", "划词图标", WS_TABSTOP, ID_APPEARANCE);
    child(parent, "STATIC", "", SS_LEFT, 276);
    child(parent, "BUTTON", "检查更新", WS_TABSTOP, ID_UPDATE);
    let update_status = child(
        parent,
        "STATIC",
        &format!("v{} · 启动时自动检查", env!("CARGO_PKG_VERSION")),
        SS_LEFT,
        219,
    );
    let status = child(
        parent,
        "STATIC",
        "选择服务，填写密钥后测试连接。",
        SS_LEFT,
        215,
    );
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

pub(super) unsafe fn layout_settings(app: &mut App) {
    let hwnd = app.settings_window;
    let scale = dpi(hwnd);
    let face = font(scale, 14, FW_NORMAL as i32);
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
    let inner = (width - 48).max(280);
    let content = inner - 40;
    let tab_width = (inner - 16) / 3;
    let template_width = (inner - 16) / 2;
    let language_width = (content - 60) / 2;
    let service = SETTINGS_PAGE.get() == SettingsPage::Service;
    let preferences = SETTINGS_PAGE.get() == SettingsPage::Preferences;
    let appearance = SETTINGS_PAGE.get() == SettingsPage::Appearance;
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
        ],
        service,
    );
    page_visible(
        hwnd,
        &[
            274,
            ID_SOURCE_LANGUAGE,
            275,
            ID_TARGET_LANGUAGE,
            ID_SWAP_LANGUAGES,
            ID_AUTO,
            ID_SELECTION_MODE,
            ID_COMPATIBILITY,
            ID_PAUSE,
            218,
            ID_UPDATE,
            219,
        ],
        preferences,
    );
    page_visible(
        hwnd,
        &[
            290,
            291,
            ID_PRESET_OCEAN,
            ID_PRESET_AURORA,
            ID_PRESET_NEBULA,
            ID_PRESET_AMBER,
            ID_APPEARANCE,
            276,
        ],
        appearance,
    );
    mark_cards(
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
            274,
            ID_SOURCE_LANGUAGE,
            275,
            ID_TARGET_LANGUAGE,
            ID_SWAP_LANGUAGES,
            ID_AUTO,
            ID_SELECTION_MODE,
            ID_COMPATIBILITY,
            ID_PAUSE,
            218,
            ID_UPDATE,
            219,
            ID_APPEARANCE,
            276,
        ],
    );
    let positions = [
        (200, 24, 18, inner - 48, 34),
        (ID_THEME, width - 60, 20, 36, 36),
        (201, 24, 60, inner, 20),
        (ID_SERVICE_TAB, 24, 96, tab_width, 36),
        (ID_PREFERENCES_TAB, 24 + tab_width + 8, 96, tab_width, 36),
        (
            ID_APPEARANCE_TAB,
            24 + (tab_width + 8) * 2,
            96,
            inner - (tab_width + 8) * 2,
            36,
        ),
        (202, 44, 164, content, 18),
        (ID_PROVIDER, 44, 186, content, 200),
        (204, 44, 230, content, 18),
        (211, 44, 252, content - 44, 36),
        (ID_KEY_VISIBILITY, width - 80, 252, 36, 36),
        (203, 44, 296, content, 18),
        (210, 44, 318, content, 36),
        (205, 44, 362, content - 128, 18),
        (206, width - 120, 362, 76, 18),
        (212, 44, 384, content - 128, 36),
        (ID_TEST, width - 164, 384, 36, 36),
        (213, width - 120, 384, 76, 36),
        (274, 44, 164, language_width, 18),
        (275, 104 + language_width, 164, language_width, 18),
        (ID_SOURCE_LANGUAGE, 44, 188, language_width, 210),
        (ID_SWAP_LANGUAGES, 56 + language_width, 188, 36, 36),
        (
            ID_TARGET_LANGUAGE,
            104 + language_width,
            188,
            language_width,
            210,
        ),
        (ID_AUTO, 44, 252, 164, 28),
        (ID_SELECTION_MODE, 224, 248, content - 180, 170),
        (ID_COMPATIBILITY, 44, 302, content - 116, 28),
        (ID_PAUSE, width - 148, 296, 104, 36),
        (218, 72, 340, content - 28, 32),
        (ID_UPDATE, 44, 392, 112, 36),
        (219, 172, 392, content - 128, 42),
        (290, 24, 152, 144, 22),
        (291, 176, 154, inner - 152, 20),
        (ID_PRESET_OCEAN, 24, 184, template_width, 104),
        (
            ID_PRESET_AURORA,
            40 + template_width,
            184,
            template_width,
            104,
        ),
        (ID_PRESET_NEBULA, 24, 300, template_width, 104),
        (
            ID_PRESET_AMBER,
            40 + template_width,
            300,
            template_width,
            104,
        ),
        (ID_APPEARANCE, 44, 426, 112, 32),
        (276, 172, 432, content - 128, 24),
        (215, 24, 484, inner, 36),
        (ID_SAVE, width - 136, 540, 112, 36),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = match id {
            200 => font(scale, 26, FW_SEMIBOLD as i32),
            201 | 215 | 218 | 219 | 276 | 291 => font(scale, 12, FW_NORMAL as i32),
            202..=206 | 274 | 275 => font(scale, 12, FW_NORMAL as i32),
            290 => font(scale, 15, FW_SEMIBOLD as i32),
            _ => face,
        };
        place(
            GetDlgItem(hwnd, id as i32),
            x,
            y - app.settings_scroll,
            w,
            h,
            scale,
            control_font,
        );
    }
    redraw_layout(hwnd);
}

pub(super) unsafe fn make_appearance(parent: HWND) -> AppearanceUi {
    child(parent, "STATIC", "图标外观", SS_LEFT, 240);
    child(parent, "STATIC", "划词图标 · 无阴影", SS_LEFT, 241);
    child(parent, "STATIC", "样式", SS_LEFT, 242);
    let style = combo(parent, ID_ICON_STYLE, ["叶片", "译字", "字母 A"]);
    child(parent, "STATIC", "形状", SS_LEFT, 245);
    let shape = combo(parent, ID_ICON_SHAPE, ["圆角", "方形", "圆形"]);
    child(parent, "STATIC", "尺寸 · 16–64 px", SS_LEFT, 246);
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
    let status = child(parent, "STATIC", "预览后点击应用。", SS_LEFT, 235);
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
    let scale = dpi(hwnd);
    let face = font(scale, 14, FW_NORMAL as i32);
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
        (241, 24, 50, width - 120, 20),
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
            241 | 235 | 242 | 245 | 246 | 243 => font(scale, 12, FW_NORMAL as i32),
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
        "EDIT",
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
    let face = font(scale, 14, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / scale as i32;
    let height = rect.bottom * 96 / scale as i32;
    let positions = [
        (250, 16, 14, width - 196, 24),
        (ID_RETRY, width - 172, 12, 28, 28),
        (ID_PIN, width - 140, 12, 28, 28),
        (ID_COPY, width - 108, 12, 28, 28),
        (ID_INLINE_EXPAND, width - 76, 12, 28, 28),
        (ID_INLINE_CLOSE, width - 44, 12, 28, 28),
        (251, 16, 52, width - 32, (height - 128).max(48)),
        (254, 16, height - 68, width - 32, 18),
        (ID_QUESTION, 16, height - 44, width - 72, 30),
        (ID_SEND, width - 44, height - 44, 28, 30),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = if id == 254 {
            font(scale, 12, FW_NORMAL as i32)
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
    child(parent, "STATIC", "译文", SS_LEFT, 223);
    let translated = child(
        parent,
        "EDIT",
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
    let scale = dpi(hwnd);
    let face = font(scale, 14, FW_NORMAL as i32);
    let mut rect: RECT = zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right * 96 / scale as i32;
    let height = rect.bottom * 96 / scale as i32;
    let inner = width - 48;
    let positions = [
        (220, 24, 20, inner - 168, 28),
        (ID_RETRY, width - 164, 17, 32, 32),
        (ID_PIN, width - 128, 17, 32, 32),
        (ID_COPY, width - 92, 17, 32, 32),
        (ID_SETTINGS, width - 56, 17, 32, 32),
        (221, 24, 66, inner, 18),
        (222, 24, 90, inner, 64),
        (223, 24, 170, inner, 18),
        (224, 24, 194, inner, (height - 286).max(48)),
        (225, 24, height - 80, inner, 18),
        (ID_QUESTION, 24, height - 48, inner - 44, 32),
        (ID_SEND, width - 56, height - 48, 32, 32),
    ];
    for (id, x, y, w, h) in positions {
        let control_font = match id {
            220 => font(scale, 18, FW_SEMIBOLD as i32),
            221 | 223 | 225 => font(scale, 12, FW_NORMAL as i32),
            _ => face,
        };
        place(GetDlgItem(hwnd, id as i32), x, y, w, h, scale, control_font);
    }
    redraw_layout(hwnd);
}
