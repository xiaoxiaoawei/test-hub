//! Compact native layouts. State and commands stay in the parent module.
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
    child(
        parent,
        "STATIC",
        "英文 → 简体中文  ·  Ctrl + Alt + T",
        SS_LEFT,
        201,
    );
    child(parent, "BUTTON", "服务", WS_TABSTOP, ID_SERVICE_TAB);
    child(parent, "BUTTON", "偏好", WS_TABSTOP, ID_PREFERENCES_TAB);
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
    child(
        parent,
        "STATIC",
        "临时复制选区，并尝试恢复剪贴板。",
        SS_LEFT,
        218,
    );
    child(parent, "BUTTON", "图标外观", WS_TABSTOP, ID_APPEARANCE);
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
    let margin = 24;
    let inner = (width - margin * 2).max(260);
    let half = (inner - 16) / 2;
    let service = SETTINGS_PAGE.get() == SettingsPage::Service;
    page_visible(
        hwnd,
        &[
            202,
            ID_PROVIDER,
            204,
            211,
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
            ID_AUTO,
            ID_SELECTION_MODE,
            ID_COMPATIBILITY,
            218,
            ID_APPEARANCE,
            276,
            ID_UPDATE,
            219,
        ],
        !service,
    );
    let positions = [
        (200, margin, 16, inner - 48, 32),
        (ID_THEME, width - margin - 32, 18, 32, 32),
        (201, margin, 55, inner, 22),
        (ID_SERVICE_TAB, margin, 88, half, 32),
        (ID_PREFERENCES_TAB, margin + half + 16, 88, half, 32),
        (202, margin, 140, inner, 22),
        (ID_PROVIDER, margin, 164, inner, 180),
        (204, margin, 208, inner, 22),
        (211, margin, 232, inner, 32),
        (203, margin, 276, inner, 22),
        (210, margin, 300, inner, 32),
        (205, margin, 344, inner - 112, 22),
        (206, width - margin - 88, 344, 88, 22),
        (212, margin, 368, inner - 128, 32),
        (ID_TEST, width - margin - 120, 368, 32, 32),
        (213, width - margin - 80, 368, 80, 32),
        (274, margin, 140, half, 22),
        (275, margin + half + 16, 140, half, 22),
        (ID_SOURCE_LANGUAGE, margin, 164, half, 200),
        (ID_TARGET_LANGUAGE, margin + half + 16, 164, half, 200),
        (ID_AUTO, margin, 218, 166, 28),
        (ID_SELECTION_MODE, margin + 182, 214, inner - 182, 156),
        (ID_COMPATIBILITY, margin, 258, inner, 28),
        (218, margin + 28, 294, inner - 28, 36),
        (ID_APPEARANCE, margin, 342, 104, 32),
        (276, margin + 120, 348, inner - 120, 24),
        (ID_UPDATE, margin, 390, 104, 32),
        (219, margin + 120, 388, inner - 120, 44),
        (
            215,
            margin,
            if service { 416 } else { 436 },
            inner,
            if service { 36 } else { 20 },
        ),
        (ID_SAVE, width - margin - 104, 462, 104, 34),
    ];
    for (id, x, y, w, h) in positions {
        let face = if id == 200 {
            font(scale, 24, FW_SEMIBOLD as i32)
        } else {
            face
        };
        place(
            GetDlgItem(hwnd, id as i32),
            x,
            y - app.settings_scroll,
            w,
            h,
            scale,
            face,
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
    let half = (width - 60) / 2;
    let preview_size = app
        .preview_icons
        .as_ref()
        .map(|icon| icon.chip_size() * 96 / scale as i32)
        .unwrap_or(32)
        .clamp(16, 64);
    let positions = [
        (240, 22, 14, width - 44, 30),
        (241, 22, 48, width - 44, 22),
        (242, 22, 82, half, 22),
        (ID_ICON_STYLE, 22, 106, half, 150),
        (245, 38 + half, 82, half, 22),
        (ID_ICON_SHAPE, 38 + half, 106, half, 150),
        (246, 22, 151, width - 142, 22),
        (ID_ICON_SIZE, 22, 177, 96, 30),
        (
            234,
            width - 94 + (64 - preview_size) / 2,
            145 + (64 - preview_size) / 2,
            preview_size,
            preview_size,
        ),
        (243, 22, 224, 42, 22),
        (ID_ICON_COLOR, 74, 218, width - 218, 32),
        (ID_SCREEN_PICK, width - 132, 218, 110, 32),
        (235, 22, 260, width - 44, (height - 316).max(24)),
        (ID_ICON_RESET, 22, height - 48, 104, 32),
        (ID_ICON_APPLY, width - 126, height - 48, 104, 32),
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

pub(super) unsafe fn make_inline(parent: HWND) -> InlineUi {
    child(parent, "STATIC", "英文 → 简体中文", SS_LEFT, 250);
    let translated = child(
        parent,
        "EDIT",
        "",
        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL | WS_TABSTOP,
        251,
    );
    let status = child(parent, "STATIC", "正在翻译…", SS_LEFT, 254);
    let copy = icon_button(parent, "复制译文", ID_COPY, "复制译文");
    icon_button(parent, "展开", ID_INLINE_EXPAND, "展开翻译窗口");
    icon_button(parent, "关闭", ID_INLINE_CLOSE, "关闭翻译窗口");
    let question = question_input(parent);
    let send = icon_button(parent, "发送问题", ID_SEND, "发送问题 · Enter");
    InlineUi {
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
        (250, 12, 12, width - 124, 24),
        (ID_COPY, width - 108, 8, 28, 28),
        (ID_INLINE_EXPAND, width - 76, 8, 28, 28),
        (ID_INLINE_CLOSE, width - 44, 8, 28, 28),
        (251, 12, 44, width - 24, (height - 116).max(48)),
        (254, 12, height - 65, width - 24, 20),
        (ID_QUESTION, 12, height - 40, width - 64, 28),
        (ID_SEND, width - 40, height - 40, 28, 28),
    ];
    for (id, x, y, w, h) in positions {
        place(GetDlgItem(hwnd, id as i32), x, y, w, h, scale, face);
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
    let status = child(parent, "STATIC", "", SS_LEFT, 225);
    let copy = icon_button(parent, "复制译文", ID_COPY, "复制译文");
    icon_button(parent, "设置", ID_SETTINGS, "打开设置");
    let question = question_input(parent);
    let send = icon_button(parent, "发送问题", ID_SEND, "发送问题 · Enter");
    ResultUi {
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
    let inner = width - 40;
    let positions = [
        (220, 20, 16, inner - 80, 28),
        (ID_COPY, width - 88, 13, 32, 32),
        (ID_SETTINGS, width - 52, 13, 32, 32),
        (221, 20, 58, inner, 20),
        (222, 20, 82, inner, 58),
        (223, 20, 152, inner, 20),
        (224, 20, 176, inner, (height - 260).max(50)),
        (225, 20, height - 76, inner, 22),
        (ID_QUESTION, 20, height - 44, inner - 44, 30),
        (ID_SEND, width - 52, height - 44, 32, 30),
    ];
    for (id, x, y, w, h) in positions {
        let face = if id == 220 {
            font(scale, 19, FW_SEMIBOLD as i32)
        } else {
            face
        };
        place(GetDlgItem(hwnd, id as i32), x, y, w, h, scale, face);
    }
    redraw_layout(hwnd);
}
