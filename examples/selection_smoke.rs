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
    use lingoleaf::config::{self, Language, SelectionMode, Settings};
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
        UI::{
            Input::KeyboardAndMouse::{
                IsWindowEnabled, SendInput, SetFocus, INPUT, INPUT_KEYBOARD, INPUT_MOUSE,
                KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, VK_DOWN, VK_HOME,
                VK_RETURN,
            },
            WindowsAndMessaging::*,
        },
    };

    const CLASS: &str = "LingoLeaf.NativeWindow";
    const BACKGROUND: &str = "轻译 · 后台";
    const SETTINGS: &str = "轻译 · 设置";
    const INLINE: &str = "轻译 · 划词译文";
    const EM_GETSEL: u32 = 0xB0;
    const EM_SETSEL: u32 = 0xB1;
    const EM_POSFROMCHAR: u32 = 0xD6;
    const SELECTION: u32 = WM_APP + 3;
    const SOURCE_TEXT: &str = "warm light.\r\nwarm light. calm. scroll wait retry manual. markdown";
    const QUESTION: i32 = 260;
    const ASK: i32 = 261;
    const ACTION: i32 = 262;
    const PIN: i32 = 263;
    const PAUSE: usize = 264;
    const EM_GETFIRSTVISIBLELINE: u32 = 0xCE;
    const EM_GETLINECOUNT: u32 = 0xBA;
    const EM_LINESCROLL: u32 = 0xB6;

    mod question_actions {
        include!("support/selection_questions_070.rs");
    }
    mod markdown_actions {
        include!("support/selection_markdown_090.rs");
    }
    const QUESTION_ONE: &str = "What does this word mean?";
    const QUESTION_TWO: &str = "Give a short example.";
    const QUESTION_OLD: &str = "Hold this old answer.";
    const QUESTION_NEW: &str = "Explain the new word.";
    const ANSWER_ONE: &str = "含义说明";
    const ANSWER_TWO: &str = "例句说明";
    const ANSWER_NEW: &str = "新词解释";
    const ALL_REQUESTS: &[&str] = &[
        "warm",
        "light",
        "question:first",
        "question:followup",
        "question:old",
        "calm",
        "question:new",
    ];

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
            source_language: Language::Auto,
            target_language: Language::Japanese,
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
        // A gesture alone no longer shows a placeholder. Only confirmed text
        // may make the reader visible; test the first valid result below.
        let switch_started = Instant::now();
        // Warm deliberately sends another fragment after light has been selected.
        // No subsequent snapshot may display that superseded translation.
        wait_for("the second streamed selection", || {
            let translated = text(inline, 251)?;
            if unsafe { IsWindowVisible(inline) } != 0 && translated.contains('旧') {
                return Err(
                    "A late fragment from the previous selection replaced the new selection."
                        .into(),
                );
            }
            Ok(translated == "新" && unsafe { IsWindowVisible(inline) } != 0)
        })?;
        require_visible(inline)?;
        if switch_started.elapsed() >= Duration::from_secs(3) {
            return Err(
                "The next selection waited too long for a blocked old HTTP response.".into(),
            );
        }
        // The server has not released the old body: progressing light first
        // proves transport cancellation, rather than just hiding old output.
        if mock.finish_warm.load(Ordering::Acquire) {
            return Err("The blocked previous translation was released too early.".into());
        }
        mock.finish_warm.store(true, Ordering::Release);
        if bounds(inline)? == first {
            return Err("The compact window did not follow the new selection position.".into());
        }

        // Select the same word on the next line while its HTTP stream is active.
        let before_move = bounds(inline)?;
        source.gesture(background, false, 18, 23)?;
        require_same_inline(&app, inline)?;
        thread::sleep(Duration::from_millis(200));
        mock.finish_light.store(true, Ordering::Release);
        wait_for("the reused in-flight translation", || {
            let translated = text(inline, 251)?;
            if translated.contains('旧') {
                return Err("A superseded translation became visible again.".into());
            }
            Ok(translated == "新译文" && text(inline, 254)?.contains("完成"))
        })?;
        require_visible(inline)?;
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

        check_questions(&source, &app, background, inline, &mock)?;
        question_actions::check(inline, &mock)?;
        check_controls(&source, &app, background, inline, &mock)?;
        markdown_actions::check(&source, &app, background, inline, &mock)?;
        let expected = all_requests();

        // Both an empty selection and a password selection must remain local.
        source.gesture(background, false, 4, 4)?;
        thread::sleep(Duration::from_millis(2400));
        mock.require_requests(&expected)?;
        source.gesture(background, true, 0, 5)?;
        thread::sleep(Duration::from_millis(2400));
        mock.require_requests(&expected)?;
        app.stop()?;
        println!("Selection controller passed: one compact HWND, updated positions, stale-progress isolation, in-flight/cache deduplication, and no requests for empty/password selections.");
        println!("Questions passed: current source/translation reference data, two-turn history, independent translation while an old answer is blocked, and a fresh conversation after reselection.");
        println!("Controls passed: blocked-request cancellation, forced retranslation, failure retry, pin across foreground changes, session pause with manual capture, and preserving text selection/scroll during streaming.");
        println!("{} local mock POSTs; no real API credential or remote service was used. Mouse-hook and keyboard behavior require separate actual-input checks.", expected.len());
        Ok(())
    }

    // New reading layout: the user's literal question follows its label; the
    // assistant label and answer are separate paragraphs so Markdown headings
    // and code fences can begin at a real block boundary. Visual paragraph
    // spacing is native RTF, rather than redundant empty text lines.
    fn displayed_conversation(base: &str, turns: &[(&str, &str)]) -> String {
        let mut output = base.to_owned();
        for (question, answer) in turns {
            output.push_str(&format!("\n你： {question}\n轻译：\n{answer}"));
        }
        output
    }

    fn check_questions(
        source: &Source,
        app: &Application,
        background: HWND,
        inline: HWND,
        mock: &Mock,
    ) -> Result<(), String> {
        ask(inline, QUESTION_ONE)?;
        let first = displayed_conversation("新译文", &[(QUESTION_ONE, ANSWER_ONE)]);
        wait_for("the first complete answer", || {
            Ok(text(inline, 251)? == first && text(inline, 254)?.contains("回答完成"))
        })
        .map_err(|error| question_failure(error, mock, inline))?;

        ask(inline, QUESTION_TWO)?;
        let second = displayed_conversation(
            "新译文",
            &[(QUESTION_ONE, ANSWER_ONE), (QUESTION_TWO, ANSWER_TWO)],
        );
        wait_for("the answer with preceding conversation history", || {
            Ok(text(inline, 251)? == second && text(inline, 254)?.contains("回答完成"))
        })
        .map_err(|error| question_failure(error, mock, inline))?;

        ask(inline, QUESTION_OLD)?;
        wait_for("a partial old answer before stream completion", || {
            Ok(text(inline, 251)?.ends_with("轻译：\n过期"))
        })
        .map_err(|error| question_failure(error, mock, inline))?;
        if mock.finish_answer.load(Ordering::Acquire) {
            return Err("The old-answer stream gate opened before reselection.".into());
        }

        // Translation has its own worker: calm must finish even while the old
        // question is still waiting for a response body on its separate stream.
        source.gesture(background, false, 25, 29)?;
        require_same_inline(app, inline)?;
        wait_for(
            "the new translation while an old question is blocked",
            || Ok(text(inline, 251)? == "静译文" && text(inline, 254)?.contains("完成")),
        )
        .map_err(|error| question_failure(error, mock, inline))?;
        if !text(inline, QUESTION)?.is_empty() {
            return Err("Reselection did not clear the question input.".into());
        }

        let observe = Instant::now();
        while observe.elapsed() < Duration::from_millis(350) {
            if text(inline, 251)? != "静译文" {
                return Err("An old answer or conversation replaced the new translation.".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        let restarted = Instant::now();
        ask(inline, QUESTION_NEW)?;
        let fresh = displayed_conversation("静译文", &[(QUESTION_NEW, ANSWER_NEW)]);
        wait_for(
            "a fresh answer containing no previous selection history",
            || {
                let output = text(inline, 251)?;
                if output.contains(QUESTION_ONE)
                    || output.contains(QUESTION_TWO)
                    || output.contains(QUESTION_OLD)
                    || output.contains("过期")
                {
                    return Err("An old conversation leaked into the new selected passage.".into());
                }
                Ok(output == fresh && text(inline, 254)?.contains("回答完成"))
            },
        )
        .map_err(|error| question_failure(error, mock, inline))?;
        if restarted.elapsed() >= Duration::from_secs(3)
            || mock.finish_answer.load(Ordering::Acquire)
        {
            return Err("A fresh question waited for the cancelled previous HTTP body.".into());
        }
        mock.finish_answer.store(true, Ordering::Release);
        mock.require_requests(ALL_REQUESTS)
    }

    fn question_failure(error: String, mock: &Mock, inline: HWND) -> String {
        let foreground = unsafe { GetForegroundWindow() };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(foreground, &mut pid) };
        format!(
            "{error}; {}; inline_visible={}; foreground={foreground:?}; foreground_root={:?}; foreground_pid={pid}; output={:?}; status={:?}",
            mock.diagnostics(),
            unsafe { IsWindowVisible(inline) },
            unsafe { GetAncestor(foreground, GA_ROOT) },
            text(inline, 251),
            text(inline, 254),
        )
    }

    const CONTROL_REQUESTS: &[&str] = &[
        "wait:blocked",
        "wait:retry",
        "wait:retry",
        "retry:failed",
        "retry:success",
        "manual",
        "scroll",
    ];

    fn controller_requests() -> Vec<&'static str> {
        ALL_REQUESTS
            .iter()
            .chain(question_actions::REQUESTS.iter())
            .chain(CONTROL_REQUESTS.iter())
            .copied()
            .collect()
    }

    fn all_requests() -> Vec<&'static str> {
        controller_requests()
            .into_iter()
            .chain(markdown_actions::REQUESTS.iter().copied())
            .collect()
    }

    fn select_word(source: &Source, background: HWND, word: &str) -> Result<(), String> {
        let start = SOURCE_TEXT
            .find(word)
            .ok_or("The public test word is missing.")?;
        source.gesture(background, false, start, start + word.len())
    }

    // Queue the same notification as a native click, so this controller check
    // cannot synchronously reenter a still-running WM_WORK paint callback.
    fn press_button(parent: HWND, id: i32) -> Result<(), String> {
        let button = unsafe { GetDlgItem(parent, id) };
        if button.is_null()
            || unsafe { IsWindowVisible(button) } == 0
            || unsafe { IsWindowEnabled(button) } == 0
        {
            return Err(format!("The action button {id} is not available."));
        }
        if unsafe { PostMessageW(parent, WM_COMMAND, id as usize, button as isize) } == 0 {
            return Err("Cannot queue an owned native button action.".into());
        }
        Ok(())
    }

    fn check_controls(
        source: &Source,
        app: &Application,
        background: HWND,
        inline: HWND,
        mock: &Mock,
    ) -> Result<(), String> {
        select_word(source, background, "wait")?;
        wait_for("a blocked translation with a partial response", || {
            Ok(text(inline, 251)? == "等待")
        })?;
        press_button(inline, ACTION)?;
        wait_for("stopping the current translation", || {
            Ok(text(inline, 254)?.contains("停止") && !text(inline, ACTION)?.contains("停止"))
        })?;
        if !text(inline, 251)?.contains("等待") {
            return Err("Stopping translation discarded its visible partial text.".into());
        }
        let retry_started = Instant::now();
        press_button(inline, ACTION)?;
        wait_for("retry after cancelling a blocked response", || {
            Ok(text(inline, 251)? == "重新完成" && text(inline, 254)?.contains("完成"))
        })?;
        if retry_started.elapsed() >= Duration::from_secs(3)
            || mock.finish_control.load(Ordering::Acquire)
        {
            return Err("Retry waited for a cancelled body to finish.".into());
        }
        let before = mock.request_count()?;
        press_button(inline, ACTION)?;
        wait_for(
            "explicit retranslation bypassing a successful cached value",
            || {
                Ok(mock.request_count()? == before + 1
                    && text(inline, 251)? == "重新完成"
                    && text(inline, 254)?.contains("完成"))
            },
        )?;

        // Pin only keeps the bubble visible. It must not freeze its text or
        // prevent the next genuine selection from moving/updating the same HWND.
        let before_pin = bounds(inline)?;
        let pin_label = text(inline, PIN)?;
        press_button(inline, PIN)?;
        wait_for("the pinned window action", || {
            Ok(text(inline, PIN)? != pin_label)
        })?;
        let other = Source::start()?;
        other.focus(false)?;
        thread::sleep(Duration::from_millis(500));
        require_visible(inline)?;
        if text(inline, 251)? != "重新完成" {
            return Err(
                "Pinning did not preserve the current translation on foreground change.".into(),
            );
        }
        drop(other);
        // Move the owned source below the pinned bubble. A real selection must
        // start on the source, not on a topmost translation covering its text.
        if unsafe {
            SetWindowPos(
                source.window as HWND,
                null_mut(),
                60,
                560,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        } == 0
        {
            return Err("Cannot move the owned source clear of the pinned bubble.".into());
        }
        select_word(source, background, "retry")?;
        wait_for("an HTTP failure on the new pinned selection", || {
            Ok(text(inline, 254)?.contains("失败"))
        })
        .map_err(|error| {
            format!(
                "{error}; {}; {}; output={:?}; status={:?}",
                mock.diagnostics(),
                source.diagnostics(),
                text(inline, 251),
                text(inline, 254)
            )
        })?;
        require_same_inline(app, inline)?;
        require_visible(inline)?;
        if bounds(inline)? == before_pin {
            return Err("Pinning incorrectly froze the next selection's position.".into());
        }
        press_button(inline, ACTION)?;
        wait_for("retrying an HTTP failure", || {
            Ok(text(inline, 251)? == "重试成功" && text(inline, 254)?.contains("完成"))
        })?;

        let before_pause = mock.request_count()?;
        send(background, WM_COMMAND, PAUSE, 0)?;
        select_word(source, background, "manual")?;
        thread::sleep(Duration::from_millis(650));
        if mock.request_count()? != before_pause || text(inline, 251)? != "重试成功" {
            return Err(
                "A paused automatic gesture sent text or replaced the pinned translation.".into(),
            );
        }
        // An explicit clipboard action remains available during session pause.
        // The source and clipboard contain fixed public fixture text only.
        send(source.edit as HWND, WM_COPY, 0, 0)?;
        send(background, WM_COMMAND, 101, 0)?;
        let detail = app.window("轻译 · 翻译与提问")?;
        wait_for(
            "manual clipboard translation while automatic capture is paused",
            || Ok(text(detail, 224)? == "手动译文" && text(detail, 225)?.contains("完成")),
        )?;
        if mock.request_count()? != before_pause + 1 {
            return Err("Manual capture during pause did not send exactly one request.".into());
        }
        send(background, WM_COMMAND, PAUSE, 0)?;

        select_word(source, background, "scroll")?;
        wait_for("a long partial translation", || {
            Ok(text(inline, 251)?.contains("line-079"))
        })?;
        let editor = unsafe { GetDlgItem(inline, 251) };
        if send(editor, EM_GETFIRSTVISIBLELINE, 0, 0)? == 0 {
            return Err("A fresh long stream did not initially follow its latest output.".into());
        }
        focus_question(inline)?;
        send(editor, EM_SETSEL, 0, 6)?;
        send(editor, EM_LINESCROLL, 0, -10_000)?;
        if send(editor, EM_GETFIRSTVISIBLELINE, 0, 0)? != 0 {
            return Err("Cannot scroll the owned long translation to its start.".into());
        }
        // Complete the stream while the native menu is pumping messages. Closing
        // the menu must drain the queued event, even if its first WM_WORK was
        // delivered during App's mutable menu borrow.
        let expected_partial = (0..80)
            .map(|line| format!("line-{line:03}: public test text"))
            .collect::<Vec<_>>()
            .join("\n");
        if send(editor, EM_GETLINECOUNT, 0, 0)? < 80 {
            return Err(
                "Markdown rendering did not preserve the long fixture's 80 visible lines.".into(),
            );
        }
        // RichEdit versions can expose soft line breaks as spaces through
        // WM_GETTEXT. Do not collapse arbitrary whitespace: native rendering
        // must contain exactly this fixture and copying must retain every LF.
        let before_menu = text(inline, 251)?;
        if before_menu != expected_partial && before_menu != expected_partial.replace('\n', " ") {
            return Err(
                "The native long translation lost or changed public fixture content.".into(),
            );
        }
        send(editor, EM_SETSEL, 0, -1)?;
        send(editor, WM_COPY, 0, 0)?;
        let native_copied = lingoleaf::platform::selection::read_clipboard_text()?
            .text
            .replace("\r\n", "\n");
        if native_copied != expected_partial {
            return Err(
                "Native RichEdit copying did not preserve the 80-line fixture's exact line breaks."
                    .into(),
            );
        }
        send(editor, EM_SETSEL, 0, 6)?;
        send(editor, WM_COPY, 0, 0)?;
        if lingoleaf::platform::selection::read_clipboard_text()?.text != "line-0" {
            return Err("Native selected-text copying changed the requested 0..6 range.".into());
        }
        // A distinct clipboard sentinel proves that the menu really performed
        // its second action; a no-op cannot reuse the preceding full-copy result.
        send(editor, EM_LINESCROLL, 0, -10_000)?;
        if unsafe { PostMessageW(editor, WM_CONTEXTMENU, editor as usize, -1) } == 0 {
            return Err("Cannot request the owned translation context menu.".into());
        }
        wait_for("the native translation context menu", || {
            Ok(owned_menu_visible(app.child.id()))
        })?;
        let popup = owned_menu(app.child.id()).ok_or("The native context menu disappeared.")?;
        if let Some(count) = native_menu_count(popup) {
            if count != 4 {
                return Err(format!("The editor opened the default {count}-item menu instead of four translation actions."));
            }
        } else {
            println!("Native menu count API is unavailable in this runtime; the actual second action and unchanged selection are checked below.");
        }
        mock.finish_control.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(250));
        // Custom menu: Home selects Copy selection, Down selects Copy translation.
        // The old native read-only EDIT menu instead reaches Select All, which
        // cannot satisfy the clipboard and original-selection assertions.
        let mut keys: [INPUT; 6] = unsafe { zeroed() };
        for (index, key) in keys.iter_mut().enumerate() {
            key.r#type = INPUT_KEYBOARD;
            key.Anonymous.ki.wVk = [VK_HOME, VK_DOWN, VK_RETURN][index / 2];
            key.Anonymous.ki.dwFlags = if index % 2 == 0 { 0 } else { KEYEVENTF_KEYUP };
        }
        if unsafe { SendInput(6, keys.as_ptr(), size_of::<INPUT>() as i32) } != 6 {
            return Err("Cannot invoke the owned context menu's Copy translation action.".into());
        }
        wait_for("closing the native translation menu", || {
            Ok(!owned_menu_visible(app.child.id()))
        })?;
        // Menu visibility can clear before its returned command finishes. Read
        // the clipboard only after Copy or the subsequent completion status, so
        // the harness cannot steal OpenClipboard from the app's own write.
        wait_for("the completed menu action", || {
            let status = text(inline, 254)?;
            Ok(status == "译文已复制" || status.contains("完成"))
        })?;
        let copied = lingoleaf::platform::selection::read_clipboard_text()?.text;
        if copied.replace("\r\n", "\n") != expected_partial {
            return Err("The editor's actual second menu action did not copy the exact complete 80-line partial translation.".into());
        }
        wait_for(
            "completion without stealing the user's scroll position",
            || Ok(text(inline, 251)?.ends_with("尾行结束") && text(inline, 254)?.contains("完成")),
        )?;
        let selected = send(editor, EM_GETSEL, 0, 0)?;
        if selected & 0xffff != 0
            || (selected >> 16) & 0xffff != 6
            || send(editor, EM_GETFIRSTVISIBLELINE, 0, 0)? != 0
        {
            return Err(
                "Streaming output moved the user's text selection or scroll position.".into(),
            );
        }
        if text(inline, 251)?.contains("过时片段")
            || text(inline, 251)?.contains(question_actions::LATE_ANSWER)
        {
            return Err("A cancelled response became visible after its server gate opened.".into());
        }
        mock.require_requests(&controller_requests())
    }

    fn owned_menu(pid: u32) -> Option<HWND> {
        unsafe {
            let mut window = GetTopWindow(null_mut());
            while !window.is_null() {
                let mut owner = 0;
                GetWindowThreadProcessId(window, &mut owner);
                if owner == pid && IsWindowVisible(window) != 0 {
                    let mut class = [0_u16; 64];
                    let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
                    if String::from_utf16_lossy(&class[..length.max(0) as usize]) == "#32768" {
                        return Some(window);
                    }
                }
                window = GetWindow(window, GW_HWNDNEXT);
            }
        }
        None
    }

    fn owned_menu_visible(pid: u32) -> bool {
        owned_menu(pid).is_some()
    }

    fn native_menu_count(popup: HWND) -> Option<i32> {
        unsafe {
            let mut info: MENUBARINFO = zeroed();
            info.cbSize = size_of::<MENUBARINFO>() as u32;
            if GetMenuBarInfo(popup, OBJID_CLIENT, 0, &mut info) != 0 && !info.hMenu.is_null() {
                let count = GetMenuItemCount(info.hMenu);
                if count >= 0 {
                    return Some(count);
                }
            }
            const MN_GETHMENU: u32 = 0x01E1;
            let mut handle = 0;
            if SendMessageTimeoutW(
                popup,
                MN_GETHMENU,
                0,
                0,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                1000,
                &mut handle,
            ) != 0
                && handle != 0
            {
                let count = GetMenuItemCount(handle as HMENU);
                if count >= 0 {
                    return Some(count);
                }
            }
        }
        None
    }

    fn ask(parent: HWND, question: &str) -> Result<(), String> {
        let input = unsafe { GetDlgItem(parent, QUESTION) };
        let submit = unsafe { GetDlgItem(parent, ASK) };
        if input.is_null() || submit.is_null() {
            return Err("The completed selection did not create a question composer.".into());
        }
        wait_for("the enabled question composer", || {
            Ok(unsafe { IsWindowVisible(input) } != 0
                && unsafe { IsWindowVisible(submit) } != 0
                && unsafe { IsWindowEnabled(submit) } != 0
                && !text(parent, ASK)?.contains("停止"))
        })
        .map_err(|error| {
            format!(
                "{error}; question={question:?}; input_visible={}; send_visible={}; send_enabled={}; output={:?}; status={:?}",
                unsafe { IsWindowVisible(input) },
                unsafe { IsWindowVisible(submit) },
                unsafe { IsWindowEnabled(submit) },
                text(parent, 251),
                text(parent, 254),
            )
        })?;
        focus_question(parent)?;
        let native_question = wide(question);
        send(input, WM_SETTEXT, 0, native_question.as_ptr() as isize)?;
        press_button(parent, ASK)?;
        // A local HTTP failure may finish and restore the question before the
        // harness can observe its initial empty field. Successful replies must
        // still leave it empty; failure-specific checks verify exact recovery.
        wait_for(
            "a submitted question or its immediate failure recovery",
            || {
                let remaining = text(parent, QUESTION)?;
                Ok(remaining.is_empty()
                    || (remaining == question && text(parent, 254)?.contains("提问失败")))
            },
        )
        .map_err(|error| {
            format!(
                "{error}; question={question:?}; input={:?}; status={:?}; action={:?}",
                text(parent, QUESTION),
                text(parent, 254),
                text(parent, ASK)
            )
        })?;
        Ok(())
    }

    fn focus_question(parent: HWND) -> Result<(), String> {
        let input = unsafe { GetDlgItem(parent, QUESTION) };
        if input.is_null() {
            return Err("The owned question editor is missing.".into());
        }
        // A compact translation first appears without activation. A real user
        // clicks its editor before typing; merely BM_CLICK-ing an inactive
        // button bypasses WM_MOUSEACTIVATE and is not that interaction.
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetWindowRect(input, &mut rect) } == 0
            || unsafe { SetCursorPos((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2) }
                == 0
        {
            return Err("Cannot point at the owned question editor.".into());
        }
        let mut clicks: [INPUT; 2] = unsafe { zeroed() };
        for (event, flags) in clicks
            .iter_mut()
            .zip([MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP])
        {
            event.r#type = INPUT_MOUSE;
            event.Anonymous.mi.dwFlags = flags;
        }
        if unsafe { SendInput(2, clicks.as_ptr(), size_of::<INPUT>() as i32) } != 2 {
            return Err("Cannot click the owned question editor.".into());
        }
        wait_for("question editor activation after a real click", || {
            let mut info: GUITHREADINFO = unsafe { zeroed() };
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            Ok(unsafe { GetForegroundWindow() } == parent
                && unsafe {
                    GetGUIThreadInfo(GetWindowThreadProcessId(parent, null_mut()), &mut info)
                } != 0
                && info.hwndFocus == input)
        })?;
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
            self.focus(password)?;
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
            let hit = unsafe { WindowFromPoint(down) };
            if unsafe { GetAncestor(hit, GA_ROOT) } != self.window as HWND {
                return Err(format!("The owned gesture origin ({},{}) is occluded: hit={hit:?}, root={:?}, expected={:?}.",down.x,down.y,unsafe{GetAncestor(hit,GA_ROOT)},self.window as HWND));
            }
            // Mirror the documented controller notification wire format. Real
            // mouse input is deliberately tested separately by the host runner.
            let packed_down = u64::from(down.x as u32) | (u64::from(down.y as u32) << 32);
            send(app, SELECTION, packed_down as usize, isize::MIN)?;
            let packed_up = u64::from(up.x as u32) | (u64::from(end > start) << 32);
            send(app, SELECTION, packed_up as usize, up.y as isize)?;
            Ok(())
        }
        fn focus(&self, password: bool) -> Result<(), String> {
            let edit = if password { self.password } else { self.edit } as HWND;
            let mut queue: MSG = unsafe { zeroed() };
            unsafe { PeekMessageW(&mut queue, null_mut(), 0, 0, PM_NOREMOVE) };
            let caller = unsafe { GetCurrentThreadId() };
            let started = Instant::now();
            loop {
                if unsafe { AttachThreadInput(caller, self.thread, 1) } == 0 {
                    return Err("Cannot attach to the owned source input queue.".into());
                }
                unsafe {
                    SetForegroundWindow(self.window as HWND);
                    SetFocus(edit);
                    AttachThreadInput(caller, self.thread, 0);
                }
                let mut info: GUITHREADINFO = unsafe { zeroed() };
                info.cbSize = size_of::<GUITHREADINFO>() as u32;
                if unsafe { GetForegroundWindow() } == self.window as HWND
                    && unsafe { GetGUIThreadInfo(self.thread, &mut info) } != 0
                    && info.hwndFocus == edit
                {
                    return Ok(());
                }
                if started.elapsed() >= Duration::from_millis(500) {
                    return Err("Cannot foreground and focus the owned selection source.".into());
                }
                thread::sleep(Duration::from_millis(20));
            }
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
        if RegisterClassW(&class) == 0
            && windows_sys::Win32::Foundation::GetLastError()
                != windows_sys::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
        {
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
        finish_answer: Arc<AtomicBool>,
        finish_control: Arc<AtomicBool>,
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
            let finish_answer = Arc::new(AtomicBool::new(false));
            let finish_control = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let state = (
                requests.clone(),
                error.clone(),
                finish_warm.clone(),
                finish_light.clone(),
                finish_answer.clone(),
                finish_control.clone(),
                stop.clone(),
            );
            let worker = thread::spawn(move || {
                let (
                    requests,
                    error,
                    finish_warm,
                    finish_light,
                    finish_answer,
                    finish_control,
                    stop,
                ) = state;
                let mut handlers = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let state = (
                                requests.clone(),
                                error.clone(),
                                finish_warm.clone(),
                                finish_light.clone(),
                                finish_answer.clone(),
                                finish_control.clone(),
                                stop.clone(),
                            );
                            handlers.push(thread::spawn(move || {
                                if let Err(problem) = serve(
                                    stream, &state.0, &state.2, &state.3, &state.4, &state.5,
                                    &state.6,
                                ) {
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
                finish_answer,
                finish_control,
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
        fn request_count(&self) -> Result<usize, String> {
            if self
                .error
                .lock()
                .map_err(|_| "Mock error lock failed.")?
                .is_some()
            {
                return Err(self.diagnostics());
            }
            Ok(self
                .requests
                .lock()
                .map_err(|_| "Mock request lock failed.")?
                .len())
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
            self.finish_answer.store(true, Ordering::Release);
            self.finish_control.store(true, Ordering::Release);
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
        answer: &AtomicBool,
        control: &AtomicBool,
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
        if body["model"] != "selection-smoke-model" || body["stream"] != true {
            return Err("Unexpected local selection request.".into());
        }
        if markdown_actions::try_serve(&body, &mut stream, requests, light, control, stop)? {
            return Ok(());
        }
        if question_actions::try_serve(&body, &mut stream, requests, control, stop)? {
            return Ok(());
        }
        if matches!(source, "wait" | "retry" | "manual" | "scroll") {
            if body["messages"].as_array().map(Vec::len) != Some(2)
                || body["messages"][0]["role"] != "system"
                || body["messages"][1]["role"] != "user"
                || !body["messages"][0]["content"].as_str().is_some_and(|system| {
                    system.starts_with("Detect the source language and translate the user message to Japanese.")
                })
            {
                return Err("Unexpected controller-test translation request.".into());
            }
            return serve_control(&mut stream, source, requests, control, stop);
        }
        let (label, first, last, gate) = match source {
            "warm" => ("warm", "旧", "译文", Some(warm)),
            "light" => ("light", "新", "译文", Some(light)),
            "calm" => ("calm", "静", "译文", None),
            _ => {
                let (label, first, last, blocked) = question_response(&body)?;
                (label, first, last, blocked.then_some(answer))
            }
        };
        if matches!(label, "warm" | "light" | "calm")
            && (body["messages"].as_array().map(Vec::len) != Some(2)
                || body["messages"][0]["role"] != "system"
                || body["messages"][1]["role"] != "user"
                || !body["messages"][0]["content"]
                    .as_str()
                    .is_some_and(|system| system.starts_with("Detect the source language and translate the user message to Japanese.")))
        {
            return Err("Unexpected local translation message roles.".into());
        }
        requests
            .lock()
            .map_err(|_| "Mock request lock failed.")?
            .push(label.to_owned());
        let tail = match begin_sse_response(&mut stream, first, last) {
            Ok(tail) => tail,
            Err(_) => return Ok(()),
        };
        let started = Instant::now();
        while gate.is_some_and(|gate| !gate.load(Ordering::Acquire))
            && !stop.load(Ordering::Acquire)
            && started.elapsed() < Duration::from_secs(12)
        {
            thread::sleep(Duration::from_millis(10));
        }
        // Disconnects are expected when the controller cancels an old generation.
        let _ = stream.write_all(&tail).and_then(|_| stream.flush());
        Ok(())
    }

    fn serve_control(
        stream: &mut TcpStream,
        source: &str,
        requests: &Mutex<Vec<String>>,
        control: &AtomicBool,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let (label, first, last, blocked, failure) = {
            let requests = requests.lock().map_err(|_| "Mock request lock failed.")?;
            match source {
                "wait" if !requests.iter().any(|request| request.starts_with("wait:")) => {
                    ("wait:blocked", "等待".into(), "过时片段", true, false)
                }
                "wait" => ("wait:retry", "重新".into(), "完成", false, false),
                "retry" if !requests.iter().any(|request| request.starts_with("retry:")) => {
                    ("retry:failed", String::new(), "", false, true)
                }
                "retry" => ("retry:success", "重试".into(), "成功", false, false),
                "manual" => ("manual", "手动".into(), "译文", false, false),
                "scroll" => (
                    "scroll",
                    (0..80)
                        .map(|line| format!("line-{line:03}: public test text\n"))
                        .collect(),
                    "尾行结束",
                    true,
                    false,
                ),
                _ => return Err("Unexpected controller fixture source.".into()),
            }
        };
        requests
            .lock()
            .map_err(|_| "Mock request lock failed.")?
            .push(label.into());
        if failure {
            return stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .map_err(|_| "Cannot write the fixture's HTTP failure.".into());
        }
        let tail = match begin_sse_response(stream, &first, last) {
            Ok(tail) => tail,
            Err(_) => return Ok(()),
        };
        let started = Instant::now();
        while blocked
            && !control.load(Ordering::Acquire)
            && !stop.load(Ordering::Acquire)
            && started.elapsed() < Duration::from_secs(20)
        {
            thread::sleep(Duration::from_millis(10));
        }
        // A cancelled client is expected to close its socket before this tail.
        let _ = stream.write_all(&tail).and_then(|_| stream.flush());
        Ok(())
    }

    fn question_response(
        body: &serde_json::Value,
    ) -> Result<(&'static str, &'static str, &'static str, bool), String> {
        let messages = body["messages"]
            .as_array()
            .ok_or("Missing local question messages.")?;
        let system = messages
            .first()
            .filter(|message| message["role"] == "system")
            .and_then(|message| message["content"].as_str())
            .ok_or("Missing local question system message.")?;
        if !system.contains("primarily in Japanese")
            || !system.contains("Infer the passage's source language")
        {
            return Err("The question did not preserve the target language preference.".into());
        }
        let context_message = messages
            .get(1)
            .filter(|message| message["role"] == "user")
            .and_then(|message| message["content"].as_str())
            .ok_or("Missing local question reference data.")?;
        let context: serde_json::Value = serde_json::from_str(context_message)
            .map_err(|_| "The question reference data was not a JSON object.")?;
        let question = messages
            .last()
            .filter(|message| message["role"] == "user")
            .and_then(|message| message["content"].as_str())
            .ok_or("Missing local question text.")?;
        let (label, first, last, blocked, history): (_, _, _, _, &[(&str, &str)]) = match question {
            QUESTION_ONE => ("question:first", "含义", "说明", false, &[]),
            QUESTION_TWO => (
                "question:followup",
                "例句",
                "说明",
                false,
                &[(QUESTION_ONE, ANSWER_ONE)],
            ),
            QUESTION_OLD => (
                "question:old",
                "过期",
                "回答",
                true,
                &[(QUESTION_ONE, ANSWER_ONE), (QUESTION_TWO, ANSWER_TWO)],
            ),
            QUESTION_NEW => ("question:new", "新词", "解释", false, &[]),
            _ => return Err("Unexpected local question text.".into()),
        };
        let (source, translation) = if question == QUESTION_NEW {
            ("calm", "静译文")
        } else {
            ("light", "新译文")
        };
        if context
            != serde_json::json!({
                "kind": "selection_context",
                "source_text": source,
                "translation": translation,
            })
        {
            return Err(
                "The question did not reference only the current selected text and translation."
                    .into(),
            );
        }
        let mut expected = Vec::new();
        for (question, answer) in history {
            expected.push(serde_json::json!({"role":"user","content":question}));
            expected.push(serde_json::json!({"role":"assistant","content":answer}));
        }
        expected.push(serde_json::json!({"role":"user","content":question}));
        if messages.len() != expected.len() + 2 || messages[2..] != expected {
            return Err("The question conversation history omitted a completed turn or leaked a previous selection.".into());
        }
        Ok((label, first, last, blocked))
    }

    // A finite, fixed-length SSE body avoids Wine's HTTP chunk-parser boundary
    // bug without releasing any gated response bytes. The real Windows WinHTTP
    // tests separately cover arbitrary chunk/header timing and cancellation.
    // The old synchronous client still blocks waiting for this held tail body.
    fn begin_sse_response(
        stream: &mut TcpStream,
        first: &str,
        last: &str,
    ) -> std::io::Result<Vec<u8>> {
        let first = format!(
            "data: {}\n\n: heartbeat\n\n",
            serde_json::json!({"choices":[{"delta":{"content":first}}]})
        );
        let tail = format!("data: {}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n", serde_json::json!({"choices":[{"delta":{"content":last}}]})).into_bytes();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", first.len()+tail.len())?;
        stream.write_all(first.as_bytes())?;
        stream.flush()?;
        Ok(tail)
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
        // Native multiline EDIT controls expose CRLF; compare the same logical
        // text used by the conversation fixture and network request payloads.
        Ok(String::from_utf16_lossy(&buffer[..count])
            .replace("\r\n", "\n")
            .replace('\r', "\n"))
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
