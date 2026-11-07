//! Explicit manual translation and its independently registered shortcut.
use super::*;

pub(super) unsafe fn register(window: HWND, hotkey: config::Hotkey) -> bool {
    RegisterHotKey(
        window,
        HOTKEY_MANUAL,
        hotkey.modifiers | MOD_NOREPEAT,
        hotkey.virtual_key,
    ) != 0
}

pub(super) unsafe fn save_settings(
    app: &mut App,
    settings: config::Settings,
) -> Result<config::Settings, String> {
    let changed =
        settings.manual_hotkey != app.settings.manual_hotkey || !app.manual_hotkey_registered;
    let was_registered = app.manual_hotkey_registered;
    if changed {
        UnregisterHotKey(app.main, HOTKEY_MANUAL);
        if !register(app.main, settings.manual_hotkey) {
            app.manual_hotkey_registered =
                was_registered && register(app.main, app.settings.manual_hotkey);
            return Err("快捷键已被占用，请换一个组合。".into());
        }
        app.manual_hotkey_registered = true;
    }
    if let Err(error) = config::save(&settings) {
        if changed {
            UnregisterHotKey(app.main, HOTKEY_MANUAL);
            app.manual_hotkey_registered =
                was_registered && register(app.main, app.settings.manual_hotkey);
        }
        return Err(error);
    }
    Ok(settings)
}

pub(super) unsafe fn leave(app: &mut App) {
    if app.manual_mode {
        app.manual_draft = text(app.result_ui.original);
        app.manual_mode = false;
        SendMessageW(app.result_ui.original, EM_SETREADONLY, 1, 0);
        ShowWindow(
            GetDlgItem(app.result_window, ID_TRANSLATE_INPUT as i32),
            SW_HIDE,
        );
        mouse::set_enabled(automatic_enabled(app));
    }
}

pub(super) unsafe fn open(app: &mut App) {
    if IsWindowVisible(app.speech_window) != 0 {
        SetForegroundWindow(app.speech_window);
        return;
    }
    if IsWindowVisible(app.appearance_window) != 0 {
        SetForegroundWindow(app.appearance_window);
        return;
    }
    if app.manual_mode {
        ShowWindow(app.result_window, SW_RESTORE);
        SetForegroundWindow(app.result_window);
        SetFocus(app.result_ui.original);
        return;
    }
    cancel_automatic(app);
    conversation_ui::reset(app);
    app.generation += 1;
    app.latest_translation
        .store(app.generation, Ordering::Release);
    app.streaming = false;
    app.active_started = None;
    app.active_text.clear();
    app.translated.clear();
    app.rendered_text.clear();
    app.display_pending = false;
    app.inline_active = false;
    ShowWindow(app.inline_window, SW_HIDE);
    app.manual_mode = true;
    mouse::set_enabled(false);
    translation_copy_enabled(app, false);
    let source_style = GetWindowLongPtrW(app.result_ui.original, GWL_STYLE);
    SetWindowLongPtrW(
        app.result_ui.original,
        GWL_STYLE,
        source_style | ES_WANTRETURN as isize,
    );
    SendMessageW(app.result_ui.original, EM_SETREADONLY, 0, 0);
    set_text(app.result_ui.original, &app.manual_draft);
    set_text(app.result_ui.translated, "");
    set_text(app.result_ui.status, "");
    ShowWindow(
        GetDlgItem(app.result_window, ID_TRANSLATE_INPUT as i32),
        SW_SHOWNA,
    );
    if !app.result_positioned {
        position_centered(app.result_window, RESULT_WIDTH, RESULT_HEIGHT);
        app.result_positioned = true;
    }
    layout_result(app);
    ShowWindow(app.result_window, SW_RESTORE);
    SetForegroundWindow(app.result_window);
    SetFocus(app.result_ui.original);
    refresh_translation_actions(app);
}

pub(super) unsafe fn submit(app: &mut App, force: bool) {
    if !app.manual_mode {
        return;
    }
    let source = text(app.result_ui.original);
    if !crate::selection_text::has_visible_text(&source) {
        translation_status(app, "请输入原文");
        SetFocus(app.result_ui.original);
        return;
    }
    if source.chars().count() > translation::MAX_TEXT_CHARS {
        translation_status(app, "原文过长，请缩短后重试");
        return;
    }
    app.manual_draft = source.clone();
    start_translation_request(app, source, "手动翻译", cursor(), false, force);
}
