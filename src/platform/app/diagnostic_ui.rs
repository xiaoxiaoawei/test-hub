//! Explicit local diagnosis. Reports contain only fixed states and counters.
use super::*;

pub(super) unsafe fn make(window: HWND) {
    let report = child(
        window,
        "EDIT",
        "",
        (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32 | WS_VSCROLL | WS_TABSTOP,
        ID_DIAGNOSTIC_REPORT,
    );
    SendMessageW(report, EM_SETLIMITTEXT, 64 * 1024, 0);
    child(window, "BUTTON", "刷新", WS_TABSTOP, ID_DIAGNOSTIC_REFRESH);
    child(window, "BUTTON", "复制报告", WS_TABSTOP, ID_DIAGNOSTIC_COPY);
    child(
        window,
        "BUTTON",
        "显示测试",
        WS_TABSTOP,
        ID_DIAGNOSTIC_DISPLAY,
    );
    child(
        window,
        "BUTTON",
        "恢复划词",
        WS_TABSTOP,
        ID_DIAGNOSTIC_RESUME,
    );
    child(window, "BUTTON", "关闭", WS_TABSTOP, ID_DIAGNOSTIC_CLOSE);
    child(
        window,
        "STATIC",
        "先在其他应用划词，再回到此处刷新。报告不含选中文字或密钥。",
        SS_LEFT,
        327,
    );
}

pub(super) unsafe fn layout(window: HWND) {
    let scale = dpi(window);
    let mut bounds: RECT = zeroed();
    GetClientRect(window, &mut bounds);
    let width = bounds.right * 96 / scale as i32;
    let height = bounds.bottom * 96 / scale as i32;
    let face = font(scale, 14, FW_NORMAL as i32);
    place(
        GetDlgItem(window, ID_DIAGNOSTIC_REPORT as i32),
        20,
        20,
        width - 40,
        (height - 128).max(200),
        scale,
        face,
    );
    place(
        GetDlgItem(window, 327),
        20,
        height - 100,
        width - 40,
        22,
        scale,
        font(scale, 13, FW_NORMAL as i32),
    );
    let gap = 10;
    let button = ((width - 40 - 4 * gap) / 5).max(70);
    for (index, id) in [
        ID_DIAGNOSTIC_REFRESH,
        ID_DIAGNOSTIC_COPY,
        ID_DIAGNOSTIC_DISPLAY,
        ID_DIAGNOSTIC_RESUME,
        ID_DIAGNOSTIC_CLOSE,
    ]
    .into_iter()
    .enumerate()
    {
        place(
            GetDlgItem(window, id as i32),
            20 + index as i32 * (button + gap),
            height - 60,
            button,
            36,
            scale,
            face,
        );
    }
    redraw_layout(window);
}

pub(super) fn disabled_reason(app: &App) -> DiagOutcome {
    if !app.settings.selection_button {
        DiagOutcome::Disabled
    } else if app.selection_paused {
        DiagOutcome::Paused
    } else if app.manual_mode {
        DiagOutcome::ManualMode
    } else {
        DiagOutcome::Ineligible
    }
}

pub(super) unsafe fn report(app: &App) -> String {
    let yes = |value| if value { "是" } else { "否" };
    let mode = if app.settings.selection_mode == config::SelectionMode::Click {
        "点击图标"
    } else {
        "直接译文"
    };
    let mut cloaked = 0_u32;
    let cloak_result = windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
        app.chip,
        windows_sys::Win32::Graphics::Dwm::DWMWA_CLOAKED as u32,
        (&mut cloaked as *mut u32).cast(),
        size_of::<u32>() as u32,
    );
    let mut chip_bounds: RECT = zeroed();
    GetWindowRect(app.chip, &mut chip_bounds);
    let graphics = format!(
        "显示器数量：{}；图标DPI：{}；尺寸：{}×{}；DWM遮蔽：{} (HRESULT {})\r\n",
        GetSystemMetrics(SM_CMONITORS),
        dpi(app.chip),
        chip_bounds.right - chip_bounds.left,
        chip_bounds.bottom - chip_bounds.top,
        cloaked,
        cloak_result
    );
    format!("轻译 v{} 划词诊断\r\n自动划词：{}\r\n本次暂停：{}\r\n手动编辑保护：{}\r\n正在屏幕取色：{}\r\n监听开关：{}\r\n监听实例存在：{}\r\n兼容取词：{}\r\n模式：{}\r\n交换鼠标主键：{}\r\n取词任务占用：{}；预热占用：{}；待处理任务：{}\r\n图标当前可见：{}；译文当前可见：{}\r\n{}\r\n{}",
        env!("CARGO_PKG_VERSION"), yes(app.settings.selection_button), yes(app.selection_paused), yes(app.manual_mode), yes(app.picking_color), yes(mouse::is_enabled()), yes(app.mouse_hook.is_some()), yes(app.settings.compatibility_capture), mode, yes(GetSystemMetrics(SM_SWAPBUTTON) != 0), yes(app.capture_busy), yes(app.warm_busy), yes(app.pending.is_some()), yes(IsWindowVisible(app.chip) != 0), yes(IsWindowVisible(app.inline_window) != 0 || IsWindowVisible(app.result_window) != 0), graphics, diagnostics::snapshot().report())
}

pub(super) unsafe fn refresh(app: &mut App) {
    set_text(
        GetDlgItem(app.diagnostic_window, ID_DIAGNOSTIC_REPORT as i32),
        &report(app),
    );
}

pub(super) unsafe fn show(app: &mut App) {
    theme::apply_window(
        app.diagnostic_window,
        app.settings.theme_mode,
        app.settings.theme_preset,
    );
    set_window_icons(app.diagnostic_window, &app.icons);
    if IsWindowVisible(app.diagnostic_window) == 0 {
        position_centered(app.diagnostic_window, 800, 620);
    }
    for reader in [app.result_window, app.inline_window] {
        SetWindowPos(
            reader,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    refresh(app);
    layout(app.diagnostic_window);
    ShowWindow(app.diagnostic_window, SW_RESTORE);
    SetForegroundWindow(app.diagnostic_window);
}

pub(super) unsafe fn close(app: &mut App) {
    if app.diagnostic_icon_test {
        ShowWindow(app.chip, SW_HIDE);
        app.diagnostic_icon_test = false;
    }
    ShowWindow(app.diagnostic_window, SW_HIDE);
}

pub(super) unsafe fn copy(app: &mut App) {
    refresh(app);
    match write_clipboard(app.diagnostic_window, &report(app)) {
        Ok(()) => set_text(GetDlgItem(app.diagnostic_window, 327), "诊断报告已复制。"),
        Err(_) => set_text(GetDlgItem(app.diagnostic_window, 327), "复制失败，请重试。"),
    }
}

pub(super) unsafe fn display_test(app: &mut App) {
    // Fixed local fixture only: never read a selection or enqueue translation.
    diagnostics::record(DiagStage::SelfTest, DiagOutcome::Started);
    cancel_automatic(app);
    app.chip_text = "LingoLeaf display test".into();
    app.chip_foreground = GetForegroundWindow() as isize;
    app.diagnostic_icon_test = true;
    let appearance = (
        app.settings.icon_style,
        app.settings.icon_color,
        app.settings.icon_shape,
        app.settings.icon_size,
    );
    let pixels = app.chip_pixels.take();
    let pixels_dpi = app.chip_dpi;
    app.settings.icon_style = config::IconStyle::Leaf;
    app.settings.icon_shape = config::IconShape::Leaf;
    app.settings.icon_color = 0x19A97C;
    app.settings.icon_size = 32;
    let result = show_chip(app, cursor());
    (
        app.settings.icon_style,
        app.settings.icon_color,
        app.settings.icon_shape,
        app.settings.icon_size,
    ) = appearance;
    app.chip_pixels = pixels;
    app.chip_dpi = pixels_dpi;
    match result {
        Ok(()) => {
            diagnostics::record(DiagStage::SelfTest, DiagOutcome::Succeeded);
            app.chip_deadline = Some(Instant::now() + Duration::from_secs(60));
            set_text(
                GetDlgItem(app.diagnostic_window, 327),
                "显示调用成功。请确认按钮旁是否可见叶片；测试不请求翻译接口。",
            );
        }
        Err(_) => {
            app.diagnostic_icon_test = false;
            diagnostics::record(DiagStage::SelfTest, DiagOutcome::Failed);
            set_text(
                GetDlgItem(app.diagnostic_window, 327),
                "图标显示失败，请复制报告中的系统错误码。",
            );
        }
    }
    refresh(app);
}

pub(super) unsafe fn resume(app: &mut App) {
    if app.picking_color {
        set_text(
            GetDlgItem(app.diagnostic_window, 327),
            "请先按 Esc 退出屏幕取色，再恢复划词。",
        );
        return;
    }
    let mut settings = app.settings.clone();
    settings.selection_button = true;
    if config::save(&settings).is_err() {
        set_text(
            GetDlgItem(app.diagnostic_window, 327),
            "恢复未应用：设置保存失败。",
        );
        return;
    }
    manual_ui::leave(app);
    cancel_automatic(app);
    app.settings = settings;
    app.selection_paused = false;
    AUTO_CHECKED.set(true);
    InvalidateRect(app.settings_ui.auto, null(), 0);
    update_pause_ui(app);
    mouse::set_enabled(true);
    let requested = if let Some(hook) = app.mouse_hook.as_ref() {
        hook.request_restart()
    } else {
        match mouse::install(app.main as isize, WM_SELECTION) {
            Ok(hook) => {
                app.mouse_hook = Some(hook);
                true
            }
            Err(_) => false,
        }
    };
    set_text(
        GetDlgItem(app.diagnostic_window, 327),
        if requested {
            "已恢复自动划词并请求重新安装监听。请在其他应用重新划词后刷新。"
        } else {
            "监听启动失败，请复制诊断报告。"
        },
    );
    refresh(app);
}
