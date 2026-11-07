// Real keyboard shortcut, clipboard paste, plain Enter and Ctrl+Enter actions.
use super::*;

pub(super) const REQUESTS: [&str; 2] = ["manual:paste", "manual:keyboard"];

fn chord(modifiers: &[u16], key: u16) -> Result<(), String> {
    let mut events = Vec::new();
    for &modifier in modifiers {
        events.push((modifier, 0));
    }
    events.push((key, 0));
    events.push((key, KEYEVENTF_KEYUP));
    for &modifier in modifiers.iter().rev() {
        events.push((modifier, KEYEVENTF_KEYUP));
    }
    let inputs: Vec<INPUT> = events
        .into_iter()
        .map(|(key, flags)| {
            let mut input: INPUT = unsafe { zeroed() };
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki.wVk = key;
            input.Anonymous.ki.dwFlags = flags;
            input
        })
        .collect();
    if unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    } != inputs.len() as u32
    {
        return Err("Cannot inject the owned manual-translation keyboard gesture.".into());
    }
    Ok(())
}

fn focus_original(detail: HWND) -> Result<HWND, String> {
    let input = unsafe { GetDlgItem(detail, 222) };
    let mut rect: RECT = unsafe { zeroed() };
    if input.is_null()
        || unsafe { GetWindowRect(input, &mut rect) } == 0
        || unsafe { SetCursorPos((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2) } == 0
    {
        return Err("The manual original editor is missing or cannot be pointed at.".into());
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
        return Err("Cannot click the manual original editor.".into());
    }
    wait_for("manual original editor focus", || {
        let mut info: GUITHREADINFO = unsafe { zeroed() };
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        Ok(unsafe { GetForegroundWindow() } == detail
            && unsafe { GetGUIThreadInfo(GetWindowThreadProcessId(detail, null_mut()), &mut info) }
                != 0
            && info.hwndFocus == input)
    })?;
    Ok(input)
}

pub(super) fn check(
    source: &Source,
    app: &Application,
    background: HWND,
    inline: HWND,
    mock: &Mock,
) -> Result<(), String> {
    let detail = app.window("轻译 · 翻译与提问")?;
    send(detail, WM_CLOSE, 0, 0)?;
    send(inline, WM_CLOSE, 0, 0)?;
    source.focus(false)?;
    let fixture = wide("manual paste");
    send(
        source.edit as HWND,
        WM_SETTEXT,
        0,
        fixture.as_ptr() as isize,
    )?;
    send(source.edit as HWND, EM_SETSEL, 0, -1)?;
    chord(&[0x11], b'C' as u16)?;
    wait_for("the public manual clipboard fixture", || {
        Ok(lingoleaf::platform::selection::read_clipboard_text()?.text == "manual paste")
    })?;
    let before = mock.request_count()?;
    chord(&[0x11, 0x12], 0x20)?;
    wait_for(
        "Ctrl+Alt+Space opening the manual translation editor",
        || {
            Ok(unsafe { IsWindowVisible(detail) } != 0
                && unsafe { IsWindowVisible(GetDlgItem(detail, 302)) } != 0)
        },
    )?;
    let anchor = bounds(detail)?;
    let input = focus_original(detail)?;
    if unsafe { GetWindowLongPtrW(input, GWL_STYLE) } & ES_READONLY as isize != 0 {
        return Err("Manual translation opened a read-only original editor.".into());
    }
    chord(&[0x11], b'A' as u16)?;
    chord(&[0x11], b'V' as u16)?;
    wait_for(
        "real clipboard pasting into the manual original editor",
        || Ok(text(detail, 222)? == "manual paste"),
    )?;
    if mock.request_count()? != before {
        return Err(
            "Opening or pasting into the manual editor sent text before explicit translation."
                .into(),
        );
    }
    press_button(detail, 302)?;
    wait_for(
        "manual translation by its explicit Translate button",
        || Ok(text(detail, 224)? == "手动粘贴译文" && text(detail, 225)?.contains("完成")),
    )?;
    if bounds(detail)? != anchor {
        return Err("Manual translation moved its original window position.".into());
    }
    // A genuine source gesture while editing must not replace the manual draft.
    source.gesture(background, false, 0, 4)?;
    thread::sleep(Duration::from_millis(650));
    if mock.request_count()? != before + 1 || text(detail, 222)? != "manual paste" {
        return Err(
            "Automatic selection replaced a manual draft or sent an unexpected request.".into(),
        );
    }
    let input = focus_original(detail)?;
    let value = wide("manual keyboard");
    send(input, WM_SETTEXT, 0, value.as_ptr() as isize)?;
    send(input, EM_SETSEL, 15, 15)?;
    chord(&[], VK_RETURN)?;
    wait_for("plain Enter inserting an original-text newline", || {
        Ok(text(detail, 222)? == "manual keyboard\n")
    })?;
    if mock.request_count()? != before + 1 {
        return Err(
            "Plain Enter submitted the manual source instead of inserting a newline.".into(),
        );
    }
    let value = wide("manual keyboard\r\nsecond line");
    send(input, WM_SETTEXT, 0, value.as_ptr() as isize)?;
    mock.finish_control.store(false, Ordering::Release);
    chord(&[0x11], VK_RETURN)?;
    wait_for(
        "a manual keyboard translation with a blocked response tail",
        || Ok(text(detail, 224)? == "手动键盘"),
    )?;
    send(background, WM_APP + 4, 0, 0)?;
    let settings = app.window(SETTINGS)?;
    wait_for(
        "settings opened while the manual translation remains active",
        || Ok(unsafe { IsWindowVisible(settings) } != 0),
    )?;
    send(background, WM_COMMAND, PAUSE, 0)?;
    mock.finish_control.store(true, Ordering::Release);
    wait_for(
        "Ctrl+Enter submitting the complete multiline manual source",
        || Ok(text(detail, 224)? == "手动键盘译文" && text(detail, 225)?.contains("完成")),
    )?;
    if bounds(detail)? != anchor {
        return Err("Manual Ctrl+Enter translation moved its window.".into());
    }
    send(background, WM_COMMAND, PAUSE, 0)?;
    send(settings, WM_CLOSE, 0, 0)?;
    send(detail, WM_CLOSE, 0, 0)?;
    source.focus(false)?;
    chord(&[0x11, 0x12], 0x20)?;
    wait_for(
        "manual hotkey preserving its draft after close and reopen",
        || {
            Ok(unsafe { IsWindowVisible(detail) } != 0
                && text(detail, 222)? == "manual keyboard\nsecond line")
        },
    )?;
    if mock.request_count()? != before + 2 {
        return Err("Reopening a saved manual draft unexpectedly submitted it.".into());
    }
    println!("Manual translation passed: real global shortcut from another app, editable source, real clipboard paste, explicit button and Ctrl+Enter submit, plain Enter newline, automatic-selection suspension and retained draft.");
    Ok(())
}

pub(super) fn try_serve(
    body: &serde_json::Value,
    stream: &mut TcpStream,
    requests: &Mutex<Vec<String>>,
    gate: &AtomicBool,
    stop: &AtomicBool,
) -> Result<bool, String> {
    let source = body["messages"][1]["content"]
        .as_str()
        .unwrap_or("")
        .replace("\r\n", "\n");
    let (label, output) = match source.as_str() {
        "manual paste" => (REQUESTS[0], "手动粘贴译文"),
        "manual keyboard\nsecond line" => (REQUESTS[1], "手动键盘译文"),
        _ => return Ok(false),
    };
    if body["messages"].as_array().map(Vec::len) != Some(2)
        || body["messages"][0]["role"] != "system"
        || body["messages"][1]["role"] != "user"
        || !body["messages"][0]["content"].as_str().is_some_and(|s| {
            s.starts_with("Detect the source language and translate the user message to Japanese.")
        })
    {
        return Err("Manual translation changed the configured language preference or wrapped the original text.".into());
    }
    requests
        .lock()
        .map_err(|_| "Manual request lock failed.")?
        .push(label.into());
    let (first, last) = if label == REQUESTS[1] {
        ("手动键盘", "译文")
    } else {
        (output, "")
    };
    let tail = begin_sse_response(stream, first, last).map_err(|e| e.to_string())?;
    let started = Instant::now();
    while label == REQUESTS[1]
        && !gate.load(Ordering::Acquire)
        && !stop.load(Ordering::Acquire)
        && started.elapsed() < Duration::from_secs(12)
    {
        thread::sleep(Duration::from_millis(10));
    }
    stream
        .write_all(&tail)
        .and_then(|_| stream.flush())
        .map_err(|e| e.to_string())?;
    Ok(true)
}
