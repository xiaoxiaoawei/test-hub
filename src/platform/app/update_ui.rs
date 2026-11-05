//! Update work stays off the UI thread; dialogs run outside App's RefCell borrow.
use super::*;

pub(super) unsafe fn enqueue_notice(app: &App, notice: Notice) {
    NOTICES.with(|queue| queue.borrow_mut().push_back(notice));
    PostMessageW(app.main, WM_NOTICE, 0, 0);
}

pub(super) unsafe fn handle_notice(main: HWND) {
    if NOTICE_ACTIVE.replace(true) {
        return;
    }
    let notice = NOTICES.with(|queue| queue.borrow_mut().pop_front());
    let mut owner = main;
    with_app(|app| owner = app.settings_window);
    match notice {
        Some(Notice::Saved) => {
            MessageBoxW(
                owner,
                wide("设置保存成功，已立即生效。").as_ptr(),
                wide("轻译 · 保存成功").as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Some(Notice::Information(message)) => {
            MessageBoxW(
                owner,
                wide(&message).as_ptr(),
                wide("轻译 · 软件更新").as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Some(Notice::Error(message)) => {
            MessageBoxW(
                owner,
                wide(&message).as_ptr(),
                wide("轻译").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
        Some(Notice::UpdateAvailable) => {
            let mut version = None;
            with_app(|app| {
                version = app
                    .available_update
                    .as_ref()
                    .map(|release| release.version.clone())
            });
            if let Some(version) = version {
                let message = format!(
                    "发现新版本 v{version}（当前 v{}）。\n\n是否下载更新？下载期间可继续使用轻译。",
                    env!("CARGO_PKG_VERSION")
                );
                if MessageBoxW(
                    owner,
                    wide(&message).as_ptr(),
                    wide("轻译 · 发现更新").as_ptr(),
                    MB_YESNO | MB_ICONINFORMATION,
                ) == IDYES
                {
                    with_app(|app| download_available_update(app));
                }
            }
        }
        Some(Notice::RestartUpdate) => {
            let mut version = None;
            with_app(|app| {
                version = app
                    .downloaded_update
                    .as_ref()
                    .map(|download| download.version.clone())
            });
            if let Some(version) = version {
                let message = format!("v{version} 已下载并通过校验。\n\n是否立即重启并安装更新？请先保存正在修改的设置。");
                if MessageBoxW(
                    owner,
                    wide(&message).as_ptr(),
                    wide("轻译 · 重启更新").as_ptr(),
                    MB_YESNO | MB_DEFBUTTON2 | MB_ICONINFORMATION,
                ) == IDYES
                {
                    with_app(|app| launch_downloaded_update(app));
                }
            }
        }
        None => {}
    }
    NOTICE_ACTIVE.set(false);
    if NOTICES.with(|queue| !queue.borrow().is_empty()) {
        PostMessageW(main, WM_NOTICE, 0, 0);
    }
}

pub(super) unsafe fn check_updates(app: &mut App, manual: bool) {
    if app.update_busy {
        return;
    }
    app.update_busy = true;
    set_text(
        app.settings_ui.update_status,
        &format!("v{} · 正在检查更新…", env!("CARGO_PKG_VERSION")),
    );
    set_text(GetDlgItem(app.settings_window, ID_UPDATE as i32), "检查中…");
    EnableWindow(GetDlgItem(app.settings_window, ID_UPDATE as i32), 0);
    let sender = app.ui_events.clone();
    let destination = app.main as isize;
    let started = thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let result = update::check_for_updates();
            let _ = sender.send(Event::UpdateChecked { manual, result });
            PostMessageW(destination as HWND, WM_WORK, 0, 0);
        });
    if started.is_err() {
        app.update_busy = false;
        refresh_update_button(app);
        set_text(app.settings_ui.update_status, "更新检查未启动，请稍后重试");
    }
}

pub(super) unsafe fn checked(
    app: &mut App,
    manual: bool,
    result: Result<update::CheckResult, String>,
) {
    app.update_busy = false;
    match result {
        Ok(update::CheckResult::Available(release)) => {
            let ready = app.downloaded_update.as_ref().is_some_and(|download| {
                download.version == release.version
                    && download.sha256 == release.sha256
                    && download.size == release.size
            });
            if ready {
                set_text(
                    app.settings_ui.update_status,
                    &format!("v{} 已下载并校验 · 等待重启", release.version),
                );
                app.available_update = Some(release);
                enqueue_notice(app, Notice::RestartUpdate);
                refresh_update_button(app);
                return;
            }
            app.downloaded_update = None;
            set_text(
                app.settings_ui.update_status,
                &format!(
                    "当前 v{} · 可更新至 v{}",
                    env!("CARGO_PKG_VERSION"),
                    release.version
                ),
            );
            app.available_update = Some(release);
            enqueue_notice(app, Notice::UpdateAvailable);
        }
        Ok(update::CheckResult::Current) => {
            app.available_update = None;
            set_text(
                app.settings_ui.update_status,
                &format!("v{} · 已是最新版本", env!("CARGO_PKG_VERSION")),
            );
            if manual {
                enqueue_notice(
                    app,
                    Notice::Information(format!(
                        "当前已是最新版本（v{}）。",
                        env!("CARGO_PKG_VERSION")
                    )),
                );
            }
        }
        Ok(update::CheckResult::NotPublished) => {
            set_text(
                app.settings_ui.update_status,
                &format!("v{} · 暂无已发布更新", env!("CARGO_PKG_VERSION")),
            );
            if manual {
                enqueue_notice(
                    app,
                    Notice::Information(format!(
                        "暂时没有已发布的更新。\n当前版本：v{}。",
                        env!("CARGO_PKG_VERSION")
                    )),
                );
            }
        }
        Err(error) => {
            set_text(
                app.settings_ui.update_status,
                "更新检查失败，可稍后手动重试",
            );
            if manual {
                enqueue_notice(app, Notice::Error(format!("更新检查失败：{error}")));
            }
        }
    }
    refresh_update_button(app);
}

pub(super) unsafe fn download_available_update(app: &mut App) {
    if app.update_busy {
        return;
    }
    let Some(release) = app.available_update.clone() else {
        return;
    };
    app.update_busy = true;
    set_text(
        app.settings_ui.update_status,
        &format!("正在下载 v{}…", release.version),
    );
    set_text(GetDlgItem(app.settings_window, ID_UPDATE as i32), "下载中…");
    EnableWindow(GetDlgItem(app.settings_window, ID_UPDATE as i32), 0);
    let sender = app.ui_events.clone();
    let destination = app.main as isize;
    let started = thread::Builder::new()
        .name("update-download".into())
        .spawn(move || {
            let mut last = Instant::now();
            let result = update::download_update(&release, |received, total| {
                if last.elapsed() >= Duration::from_millis(100) || received == total {
                    let _ = sender.send(Event::UpdateProgress { received, total });
                    PostMessageW(destination as HWND, WM_WORK, 0, 0);
                    last = Instant::now();
                }
            });
            let _ = sender.send(Event::UpdateDownloaded(result));
            PostMessageW(destination as HWND, WM_WORK, 0, 0);
        });
    if started.is_err() {
        app.update_busy = false;
        refresh_update_button(app);
        set_text(app.settings_ui.update_status, "更新下载未启动，请稍后重试");
    }
}

pub(super) unsafe fn downloaded(app: &mut App, result: Result<update::VerifiedDownload, String>) {
    app.update_busy = false;
    match result {
        Ok(download) => {
            set_text(
                app.settings_ui.update_status,
                &format!("v{} 已下载并校验 · 等待重启", download.version),
            );
            app.downloaded_update = Some(download);
            enqueue_notice(app, Notice::RestartUpdate);
        }
        Err(error) => {
            set_text(app.settings_ui.update_status, "更新下载失败，可重试");
            enqueue_notice(app, Notice::Error(format!("更新下载失败：{error}")));
        }
    }
    refresh_update_button(app);
}

unsafe fn launch_downloaded_update(app: &mut App) {
    if app.update_busy {
        return;
    }
    let Some(download) = app.downloaded_update.clone() else {
        return;
    };
    app.update_busy = true;
    set_text(app.settings_ui.update_status, "正在准备重启更新…");
    EnableWindow(app.settings_window, 0);
    EnableWindow(GetDlgItem(app.settings_window, ID_UPDATE as i32), 0);
    let sender = app.ui_events.clone();
    let destination = app.main as isize;
    let started = thread::Builder::new()
        .name("update-install".into())
        .spawn(move || {
            let result = update_install::launch(&download);
            let _ = sender.send(Event::UpdateLaunched(result));
            PostMessageW(destination as HWND, WM_WORK, 0, 0);
        });
    if started.is_err() {
        app.update_busy = false;
        EnableWindow(app.settings_window, 1);
        refresh_update_button(app);
        enqueue_notice(app, Notice::Error("无法启动更新任务，请稍后重试。".into()));
    }
}

pub(super) unsafe fn launched(app: &mut App, result: Result<(), String>) {
    match result {
        Ok(()) => PostQuitMessage(0),
        Err(error) => {
            app.update_busy = false;
            EnableWindow(app.settings_window, 1);
            refresh_update_button(app);
            set_text(
                app.settings_ui.update_status,
                "更新未安装 · 当前版本继续运行",
            );
            enqueue_notice(
                app,
                Notice::Error(format!(
                    "暂时无法安装更新：{error}\n\n当前版本仍可继续使用，也可手动下载新版。"
                )),
            );
        }
    }
}

unsafe fn refresh_update_button(app: &App) {
    let label = if app.downloaded_update.is_some() {
        "重启更新"
    } else if app.available_update.is_some() {
        "下载更新"
    } else {
        "检查更新"
    };
    set_text(GetDlgItem(app.settings_window, ID_UPDATE as i32), label);
    EnableWindow(
        GetDlgItem(app.settings_window, ID_UPDATE as i32),
        !app.update_busy as i32,
    );
}
