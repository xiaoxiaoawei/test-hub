//! Independent online speech settings; opening this dialog never sends text.
use super::*;

pub(super) unsafe fn show(app: &mut App, restore_translation: bool) {
    if IsWindowVisible(app.speech_window) != 0 {
        SetForegroundWindow(app.speech_window);
        return;
    }
    if IsWindowVisible(app.appearance_window) != 0 {
        SetForegroundWindow(app.appearance_window);
        return;
    }
    app.speech_restore_view = if restore_translation {
        if IsWindowVisible(app.inline_window) != 0 {
            Some(true)
        } else if IsWindowVisible(app.result_window) != 0 {
            Some(false)
        } else {
            None
        }
    } else {
        None
    };
    app.speech_settings_was_visible = IsWindowVisible(app.settings_window) != 0;
    show_settings(app);
    set_text(app.speech_ui.base, &app.settings.speech_base_url);
    set_text(app.speech_ui.key, &app.settings.speech_api_key);
    set_text(app.speech_ui.model, &app.settings.speech_model);
    set_text(app.speech_ui.voice, &app.settings.speech_voice);
    set_text(app.speech_ui.status, "");
    EnableWindow(app.settings_window, 0);
    position_over_owner(
        app.speech_window,
        app.settings_window,
        SPEECH_WIDTH,
        SPEECH_HEIGHT,
    );
    layout_speech_settings(app.speech_window);
    ShowWindow(app.speech_window, SW_SHOW);
    SetForegroundWindow(app.speech_window);
    SetFocus(app.speech_ui.base);
}

pub(super) unsafe fn save(app: &mut App) {
    let mut settings = app.settings.clone();
    settings.speech_base_url = text(app.speech_ui.base).trim().to_owned();
    settings.speech_api_key = text(app.speech_ui.key).trim().to_owned();
    settings.speech_model = text(app.speech_ui.model).trim().to_owned();
    settings.speech_voice = text(app.speech_ui.voice).trim().to_owned();
    if let Err(error) = settings
        .speech_settings()
        .validate()
        .and_then(|_| config::save(&settings))
    {
        set_text(app.speech_ui.status, &error);
        return;
    }
    stop_speech(app);
    app.settings = settings;
    close(app);
}

pub(super) unsafe fn close(app: &mut App) {
    ShowWindow(app.speech_window, SW_HIDE);
    set_text(app.speech_ui.key, "");
    EnableWindow(app.settings_window, 1);
    if !app.speech_settings_was_visible {
        ShowWindow(app.settings_window, SW_HIDE);
    }
    if let Some(inline) = app.speech_restore_view.take() {
        if app.translation_ready && !app.streaming {
            app.display_pending = false;
            present_translation(app, inline, cursor());
            refresh_translation_actions(app);
            if !inline {
                SetForegroundWindow(app.result_window);
            }
            return;
        }
    }
    if app.speech_settings_was_visible {
        SetForegroundWindow(app.settings_window);
    }
}
