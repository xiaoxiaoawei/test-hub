//! Selection capture runs on a worker thread, never in a hook or window procedure.
//!
//! UI Automation is read-only. The opt-in copy fallback deliberately refuses rich
//! clipboards: losing HTML, images, or delayed-rendered data is worse than asking
//! the user to copy explicitly and use “Translate clipboard”.

use std::{
    mem::size_of,
    ptr, thread,
    time::{Duration, Instant},
};

use windows::{
    core::Interface,
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        },
        UI::Accessibility::{
            CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement,
            IUIAutomationTextPattern, IUIAutomationTreeWalker, UIA_TextPatternId,
        },
    },
};
use windows_sys::Win32::{
    Foundation::{GetLastError, GlobalFree, SetLastError, HGLOBAL, HWND, POINT},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
            GetClipboardOwner, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
        },
        Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYEVENTF_KEYUP, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetAncestor, GetClassNameW, GetForegroundWindow,
            GetGUIThreadInfo, GetWindowLongPtrW, GetWindowThreadProcessId, SendMessageTimeoutW,
            WindowFromPoint, ES_PASSWORD, GA_ROOT, GUITHREADINFO, GWL_STYLE, HWND_MESSAGE,
            SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, WM_GETTEXT,
        },
    },
};

const UNICODE_TEXT: u32 = 13;
const MAX_SELECTION_UNITS: usize = 32_000;
const MAX_CLIPBOARD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CapturedText {
    pub text: String,
    pub source: &'static str,
}

#[derive(Debug)]
enum UiaFailure {
    Protected,
    Unverified,
    Unavailable(String),
}

/// Capture the current foreground selection. Automatic mouse selection must pass
/// `false`; only a deliberate user hotkey is allowed to pass `true`.
pub fn capture_selection(allow_clipboard_fallback: bool) -> Result<CapturedText, String> {
    capture(None, allow_clipboard_fallback)
}

/// Read a mouse selection without changing the clipboard. Hit testing matters
/// for documents whose text selection does not move Windows keyboard focus.
pub fn capture_selection_at(point: POINT) -> Result<CapturedText, String> {
    capture(Some(point), false)
}

fn capture(point: Option<POINT>, allow_copy: bool) -> Result<CapturedText, String> {
    let target = Target::foreground()?;
    let point_window = point.map(|point| unsafe { WindowFromPoint(point) });
    let focus_window = target.focus_window();
    // A true EDIT selection is more direct and cheaper than COM, and also works
    // when an old application does not expose a UIA TextPattern provider.
    let native_first = if point.is_some() {
        point_window
    } else {
        focus_window
    };
    if let Some(window) = native_first {
        match native_edit_selection(&target, window) {
            Ok(NativeSelection::Text(text)) => return target.finish(text, "原生文本选区"),
            Ok(NativeSelection::Empty) => return Err(no_selection()),
            Err(UiaFailure::Protected) => return Err(protected_message()),
            Ok(NativeSelection::Unsupported)
            | Err(UiaFailure::Unavailable(_))
            | Err(UiaFailure::Unverified) => {}
        }
    }

    let uia = capture_uia(&target, point);
    match uia {
        Ok(text) => target.finish(text, "UI Automation"),
        Err(UiaFailure::Protected) => Err(protected_message()),
        Err(failure) => {
            let (reason, allow_copy) = match failure {
                UiaFailure::Unverified => (protected_message(), false),
                UiaFailure::Unavailable(reason) => (reason, allow_copy),
                UiaFailure::Protected => unreachable!("protected candidate handled above"),
            };
            // Focused native controls are a secondary source for a drag whose
            // pointer ends outside the document. Never cross the foreground root.
            if let Some(window) = focus_window {
                if Some(window) != native_first {
                    match native_edit_selection(&target, window) {
                        Ok(NativeSelection::Text(text)) => {
                            return target.finish(text, "原生文本选区")
                        }
                        Err(UiaFailure::Protected) => return Err(protected_message()),
                        _ => {}
                    }
                }
            }
            if allow_copy {
                capture_by_copy(target.foreground)
                    .map_err(|fallback| format!("{reason}\n{fallback}"))
            } else {
                Err(reason)
            }
        }
    }
}

fn no_selection() -> String {
    "未读取到选中文字，请重新选择。".into()
}
fn protected_message() -> String {
    "不会读取密码字段或无法确认安全属性的文本控件。".into()
}

struct Target {
    foreground: HWND,
    root: HWND,
    pid: u32,
    thread: u32,
}

impl Target {
    fn foreground() -> Result<Self, String> {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return Err("没有可读取的前台窗口。".into());
        }
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(foreground, &mut pid) };
        let root = unsafe { GetAncestor(foreground, GA_ROOT) };
        if root.is_null() || pid == 0 || thread == 0 {
            return Err("前台窗口已经失效。".into());
        }
        Ok(Self {
            foreground,
            root,
            pid,
            thread,
        })
    }

    fn owns_window(&self, window: HWND) -> bool {
        !window.is_null() && unsafe { GetAncestor(window, GA_ROOT) } == self.root
    }

    fn focus_window(&self) -> Option<HWND> {
        let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        if unsafe { GetGUIThreadInfo(self.thread, &mut info) } != 0
            && self.owns_window(info.hwndFocus)
        {
            Some(info.hwndFocus)
        } else {
            None
        }
    }

    fn finish(&self, text: String, source: &'static str) -> Result<CapturedText, String> {
        if unsafe { GetForegroundWindow() } != self.foreground {
            return Err("前台窗口已改变，请重新选择文字。".into());
        }
        Ok(CapturedText { text, source })
    }
}

const NATIVE_TIMEOUT_MS: u32 = 120;
const EM_GETSEL_MESSAGE: u32 = 0x00B0;

enum NativeSelection {
    Unsupported,
    Empty,
    Text(String),
}

fn native_edit_selection(target: &Target, window: HWND) -> Result<NativeSelection, UiaFailure> {
    if !target.owns_window(window) {
        return Ok(NativeSelection::Unsupported);
    }
    let mut class = [0u16; 64];
    let count = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    let class = String::from_utf16_lossy(&class[..count.max(0) as usize]);
    // RichEdit's WM_USER range messages do not marshal pointers across processes,
    // and its paragraph offsets can differ from WM_GETTEXT. Keep it on UIA.
    if !class.eq_ignore_ascii_case("edit") {
        return Ok(NativeSelection::Unsupported);
    }
    if native_password(window)? {
        return Err(UiaFailure::Protected);
    }
    let (start, end) = native_range(window)?;
    if start == end {
        return Ok(NativeSelection::Empty);
    }
    if end > MAX_SELECTION_UNITS || end < start {
        return Err(UiaFailure::Unavailable(
            "原生控件选区超出安全读取范围，请尝试快捷键。".into(),
        ));
    }
    let mut text = vec![0u16; end + 1];
    let read = native_message(window, WM_GETTEXT, text.len(), text.as_mut_ptr() as isize)?;
    if native_password(window)? {
        return Err(UiaFailure::Protected);
    }
    if !target.owns_window(window) || read < end || native_range(window)? != (start, end) {
        return Err(UiaFailure::Unavailable(
            "选区已变化，请重新选择文字。".into(),
        ));
    }
    let selected = String::from_utf16(&text[start..end])
        .map_err(|_| UiaFailure::Unavailable("选区含不完整的 Unicode 字符，请重新选择。".into()))?;
    let selected = selected.trim().to_owned();
    Ok(if selected.is_empty() {
        NativeSelection::Empty
    } else {
        NativeSelection::Text(selected)
    })
}

fn native_password(window: HWND) -> Result<bool, UiaFailure> {
    unsafe { SetLastError(0) };
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
    if style == 0 && unsafe { GetLastError() } != 0 {
        return Err(UiaFailure::Unverified);
    }
    Ok(style & ES_PASSWORD as isize != 0)
}

fn native_range(window: HWND) -> Result<(usize, usize), UiaFailure> {
    let mut start = u32::MAX;
    let mut end = u32::MAX;
    native_message(
        window,
        EM_GETSEL_MESSAGE,
        (&mut start as *mut u32) as usize,
        (&mut end as *mut u32) as isize,
    )?;
    if start == u32::MAX || end == u32::MAX {
        return Err(UiaFailure::Unavailable(
            "此控件没有提供可读取的原生选区。".into(),
        ));
    }
    Ok((start as usize, end as usize))
}

fn native_message(
    window: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> Result<usize, UiaFailure> {
    let mut result = 0;
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            message,
            wparam,
            lparam,
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
            NATIVE_TIMEOUT_MS,
            &mut result,
        )
    };
    if sent == 0 {
        Err(UiaFailure::Unavailable(
            "目标程序暂时没有响应选区读取。".into(),
        ))
    } else {
        Ok(result)
    }
}

/// Read the clipboard after the user has explicitly copied text. This operation
/// never clears, replaces, or attempts to restore the clipboard.
pub fn read_clipboard_text() -> Result<CapturedText, String> {
    let _clipboard = ClipboardLock::open(ptr::null_mut())?;
    let text = unsafe { read_unicode_text()? };
    Ok(CapturedText {
        text,
        source: "剪贴板",
    })
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, UiaFailure> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| UiaFailure::Unavailable(format!("无法初始化文字读取组件：{error}")))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

const UIA_BUDGET: Duration = Duration::from_millis(900);
const MAX_ANCESTORS: usize = 10;

fn capture_uia(target: &Target, point: Option<POINT>) -> Result<String, UiaFailure> {
    let _apartment = ComApartment::initialize()?;
    // UIA timeouts bound individual provider transactions. They cannot make a
    // misbehaving COM provider synchronously cancellable; the UI never waits here.
    let deadline = Instant::now() + UIA_BUDGET;
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
                .map_err(|_| UiaFailure::Unavailable("此窗口暂时无法读取选区。".into()))?;
        if let Ok(settings) = automation.cast::<IUIAutomation2>() {
            let _ = settings.SetConnectionTimeout(400);
            let _ = settings.SetTransactionTimeout(500);
        }
        let walker = automation.ControlViewWalker().ok();
        let mut unverified = false;
        if let Some(point) = point {
            if target.owns_window(WindowFromPoint(point)) && Instant::now() < deadline {
                if let Ok(element) =
                    automation.ElementFromPoint(windows::Win32::Foundation::POINT {
                        x: point.x,
                        y: point.y,
                    })
                {
                    match candidate_text(target, element, walker.as_ref(), deadline) {
                        Ok(Some(text)) => return Ok(text),
                        Err(UiaFailure::Unverified) => unverified = true,
                        Err(failure) => return Err(failure),
                        Ok(None) => {}
                    }
                }
            }
        }
        if Instant::now() < deadline {
            if let Ok(element) = automation.GetFocusedElement() {
                match candidate_text(target, element, walker.as_ref(), deadline) {
                    Ok(Some(text)) => return Ok(text),
                    Err(UiaFailure::Unverified) => unverified = true,
                    Err(failure) => return Err(failure),
                    Ok(None) => {}
                }
            }
        }
        if unverified {
            return Err(UiaFailure::Unverified);
        }
    }
    Err(UiaFailure::Unavailable(
        "未读取到选中文字；请重新划选或使用快捷键。".into(),
    ))
}

unsafe fn candidate_text(
    target: &Target,
    first: IUIAutomationElement,
    walker: Option<&IUIAutomationTreeWalker>,
    deadline: Instant,
) -> Result<Option<String>, UiaFailure> {
    let chain = unsafe { scoped_chain(target, first, walker, deadline) };
    let mut verified_leaf = false;
    for element in chain {
        if Instant::now() >= deadline {
            break;
        }
        match unsafe { element.CurrentIsPassword() } {
            Ok(value) if value.as_bool() => return Err(UiaFailure::Protected),
            Ok(_) => verified_leaf = true,
            // An unknown first candidate is unsafe to read. After a known-safe
            // leaf, skip an unsupported ancestor and try its next ancestor;
            // never extract text from the unknown element itself.
            Err(_) if verified_leaf => continue,
            Err(_) => return Err(UiaFailure::Unverified),
        }
        if Instant::now() >= deadline {
            break;
        }
        if let Ok(pattern) =
            unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
        {
            if let Some(text) = unsafe { selected_text(&pattern, deadline)? } {
                return Ok(Some(text));
            }
        }
    }
    Ok(None)
}

unsafe fn scoped_chain(
    target: &Target,
    first: IUIAutomationElement,
    walker: Option<&IUIAutomationTreeWalker>,
    deadline: Instant,
) -> Vec<IUIAutomationElement> {
    let initial_pid = unsafe { first.CurrentProcessId() }.ok().unwrap_or(0);
    let mut scoped = false;
    let mut chain = Vec::new();
    let mut element = first;
    for _ in 0..MAX_ANCESTORS {
        if Instant::now() >= deadline {
            break;
        }
        let native = unsafe { element.CurrentNativeWindowHandle() }
            .ok()
            .map(|window| window.0);
        if let Some(native) = native.filter(|window| !window.is_null()) {
            if !target.owns_window(native) {
                return Vec::new();
            }
            scoped = true;
        }
        let at_root = native == Some(target.root);
        let pid = unsafe { element.CurrentProcessId() }.ok().unwrap_or(0);
        chain.push((element.clone(), pid));
        if at_root {
            break;
        }
        let Some(walker) = walker else { break };
        if Instant::now() >= deadline {
            break;
        }
        let Ok(parent) = (unsafe { walker.GetParentElement(&element) }) else {
            break;
        };
        // Hosted browsers can cross renderer/host PIDs within one foreground
        // root. Native HWND ancestry is stronger evidence than PID equality.
        element = parent;
    }
    // Some providers do not expose HWNDs. A same-process candidate returned by
    // the foreground's hit test/focus is still useful, but never accept an
    // unproven cross-process document or walk into another root window.
    if scoped {
        chain.into_iter().map(|(element, _)| element).collect()
    } else if initial_pid == target.pid as i32 {
        chain
            .into_iter()
            .take_while(|(_, pid)| *pid == target.pid as i32)
            .map(|(element, _)| element)
            .collect()
    } else {
        Vec::new()
    }
}

unsafe fn selected_text(
    pattern: &IUIAutomationTextPattern,
    deadline: Instant,
) -> Result<Option<String>, UiaFailure> {
    if Instant::now() >= deadline {
        return Err(UiaFailure::Unavailable(
            "选区读取暂时超时，请重新划选。".into(),
        ));
    }
    let Ok(ranges) = (unsafe { pattern.GetSelection() }) else {
        return Ok(None);
    };
    let count = unsafe { ranges.Length() }.unwrap_or(0);
    if count > 32 {
        return Err(UiaFailure::Unavailable(
            "选区过多，请只选择需要翻译的一段文字。".into(),
        ));
    }
    let mut text = String::new();
    let mut units = 0;
    for index in 0..count.max(0) {
        if Instant::now() >= deadline {
            return Err(UiaFailure::Unavailable(
                "选区读取暂时超时，请重新划选。".into(),
            ));
        }
        let range = unsafe { ranges.GetElement(index) }
            .map_err(|_| UiaFailure::Unavailable("选区已失效，请重新选择文字。".into()))?;
        if Instant::now() >= deadline {
            return Err(UiaFailure::Unavailable(
                "选区读取暂时超时，请重新划选。".into(),
            ));
        }
        let selected = unsafe { range.GetText((MAX_SELECTION_UNITS + 1) as i32) }
            .map_err(|_| UiaFailure::Unavailable("无法读取这段选区，请尝试手动复制。".into()))?;
        let selected = selected.to_string();
        units += selected.encode_utf16().count();
        if units > MAX_SELECTION_UNITS {
            return Err(UiaFailure::Unavailable(
                "选中文字过长，请缩小选区后重试。".into(),
            ));
        }
        if !selected.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&selected);
        }
    }
    let text = text.trim().to_owned();
    Ok((!text.is_empty()).then_some(text))
}

struct ClipboardLock;

impl ClipboardLock {
    fn open(owner: HWND) -> Result<Self, String> {
        let start = Instant::now();
        loop {
            if unsafe { OpenClipboard(owner) } != 0 {
                return Ok(Self);
            }
            if start.elapsed() >= Duration::from_millis(200) {
                return Err("剪贴板正被其他程序占用，请稍后重试。".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ClipboardLock {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

unsafe fn clipboard_bytes() -> Result<Vec<u8>, String> {
    let handle = unsafe { GetClipboardData(UNICODE_TEXT) };
    if handle.is_null() {
        return Err("剪贴板中没有可翻译的文字。".into());
    }
    let len = unsafe { GlobalSize(handle) };
    if !(2..=MAX_CLIPBOARD_BYTES).contains(&len) || len % 2 != 0 {
        return Err("剪贴板文字格式无效或内容过大。".into());
    }
    let data = unsafe { GlobalLock(handle) };
    if data.is_null() {
        return Err("暂时无法读取剪贴板文字。".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) }.to_vec();
    unsafe { GlobalUnlock(handle) };
    Ok(bytes)
}

fn decode_unicode(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err("剪贴板文字格式无效。".into());
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .ok_or_else(|| "剪贴板文字格式无效。".to_owned())?;
    if end > MAX_SELECTION_UNITS {
        return Err("剪贴板文字过长，请只复制需要翻译的一段文字。".into());
    }
    let text = String::from_utf16(&units[..end])
        .map_err(|_| "剪贴板包含无效的 Unicode 文字。".to_owned())?
        .trim()
        .to_owned();
    if text.is_empty() {
        return Err("剪贴板中没有可翻译的文字。".into());
    }
    Ok(text)
}

unsafe fn read_unicode_text() -> Result<String, String> {
    decode_unicode(&unsafe { clipboard_bytes()? })
}

struct ClipboardOwner(HWND);

impl ClipboardOwner {
    fn new() -> Result<Self, String> {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if window.is_null() {
            return Err("无法安全备份剪贴板，请手动复制后使用“翻译剪贴板”。".into());
        }
        Ok(Self(window))
    }
}

impl Drop for ClipboardOwner {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

struct ClipboardSnapshot {
    // Allocation is prepared before sending Ctrl+C, so an allocation failure
    // cannot prevent restoring the previous text after we clear the clipboard.
    original: Option<HGLOBAL>,
}

impl ClipboardSnapshot {
    unsafe fn take() -> Result<Self, String> {
        unsafe { SetLastError(0) };
        let first = unsafe { EnumClipboardFormats(0) };
        if first == 0 {
            if unsafe { GetLastError() } != 0 {
                return Err("无法检查剪贴板，已取消模拟复制。".into());
            }
            return Ok(Self { original: None });
        }
        if first != UNICODE_TEXT {
            return Err(
                "为保护剪贴板中的图片或富文本，未模拟复制。请先手动复制，再使用“翻译剪贴板”。"
                    .into(),
            );
        }
        unsafe { SetLastError(0) };
        if unsafe { EnumClipboardFormats(first) } != 0 || unsafe { GetLastError() } != 0 {
            return Err(
                "剪贴板包含多种格式，已保留原内容。请先手动复制，再使用“翻译剪贴板”。".into(),
            );
        }
        let bytes = unsafe { clipboard_bytes()? };
        let allocation = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
        if allocation.is_null() {
            return Err("无法备份剪贴板，已取消模拟复制。".into());
        }
        let output = unsafe { GlobalLock(allocation) };
        if output.is_null() {
            unsafe { GlobalFree(allocation) };
            return Err("无法备份剪贴板，已取消模拟复制。".into());
        }
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), bytes.len());
            GlobalUnlock(allocation);
        }
        Ok(Self {
            original: Some(allocation),
        })
    }

    /// The caller holds the clipboard lock and has verified its sequence number.
    unsafe fn restore(&mut self) -> Result<(), String> {
        if unsafe { EmptyClipboard() } == 0 {
            return Err("未能恢复原剪贴板内容。".into());
        }
        if let Some(handle) = self.original {
            if unsafe { SetClipboardData(UNICODE_TEXT, handle) }.is_null() {
                return Err("未能恢复原剪贴板内容。".into());
            }
            self.original = None; // The system now owns the allocation.
        }
        Ok(())
    }
}

impl Drop for ClipboardSnapshot {
    fn drop(&mut self) {
        if let Some(allocation) = self.original {
            unsafe { GlobalFree(allocation) };
        }
    }
}

fn modifiers_down() -> bool {
    [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&key| unsafe { GetAsyncKeyState(key as i32) } < 0)
}

fn keyboard_input(key: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn capture_by_copy(foreground: HWND) -> Result<CapturedText, String> {
    let released = Instant::now();
    while modifiers_down() {
        if released.elapsed() >= Duration::from_millis(1_200) {
            return Err("请松开快捷键后重试。".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    if unsafe { GetForegroundWindow() } != foreground {
        return Err("前台窗口已改变，已取消模拟复制。".into());
    }

    let owner = ClipboardOwner::new()?;
    let (mut snapshot, initial_sequence) = {
        let _clipboard = ClipboardLock::open(owner.0)?;
        let snapshot = unsafe { ClipboardSnapshot::take()? };
        (snapshot, unsafe { GetClipboardSequenceNumber() })
    };
    if unsafe { GetForegroundWindow() } != foreground
        || modifiers_down()
        || unsafe { GetClipboardSequenceNumber() } != initial_sequence
    {
        return Err("窗口、按键或剪贴板状态已改变，请重试。".into());
    }

    let input = [
        keyboard_input(VK_CONTROL, false),
        keyboard_input(0x43, false),
        keyboard_input(0x43, true),
        keyboard_input(VK_CONTROL, true),
    ];
    let sent = unsafe {
        SendInput(
            input.len() as u32,
            input.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != input.len() as u32 {
        if sent > 0 {
            let release = [keyboard_input(0x43, true), keyboard_input(VK_CONTROL, true)];
            unsafe {
                SendInput(
                    release.len() as u32,
                    release.as_ptr(),
                    size_of::<INPUT>() as i32,
                )
            };
        }
        return Err("此窗口不允许模拟复制（例如管理员程序）。请手动复制后翻译剪贴板。".into());
    }
    let started = Instant::now();
    loop {
        let sequence = unsafe { GetClipboardSequenceNumber() };
        if sequence != initial_sequence {
            let _clipboard = ClipboardLock::open(owner.0)?;
            if unsafe { GetClipboardSequenceNumber() } != sequence {
                return Err("检测到另一次复制，已保留最新剪贴板内容。".into());
            }
            let clipboard_owner = unsafe { GetClipboardOwner() };
            let mut source_pid = 0;
            let mut clipboard_pid = 0;
            unsafe {
                GetWindowThreadProcessId(foreground, &mut source_pid);
                GetWindowThreadProcessId(clipboard_owner, &mut clipboard_pid);
            }
            if source_pid == 0 || source_pid != clipboard_pid {
                return Err(
                    "无法确认这次复制的来源，已保留最新剪贴板内容。请手动复制后翻译。".into(),
                );
            }
            let selected = unsafe { read_unicode_text() };
            // Delayed rendering may itself advance the sequence. Nobody else can
            // replace the clipboard while this lock remains held.
            unsafe { snapshot.restore()? };
            if unsafe { GetForegroundWindow() } != foreground {
                return Err("前台窗口已改变，已取消翻译。".into());
            }
            return selected.map(|text| CapturedText {
                text,
                source: "模拟复制",
            });
        }
        if unsafe { GetForegroundWindow() } != foreground {
            return Err("前台窗口已改变，已取消读取；剪贴板保持当前内容。".into());
        }
        if started.elapsed() >= Duration::from_millis(700) {
            return Err("未收到复制结果，请手动复制后使用“翻译剪贴板”。".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_unicode, native_edit_selection, NativeSelection, Target, UiaFailure,
        MAX_SELECTION_UNITS,
    };
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetAncestor, GetWindowThreadProcessId, SendMessageW,
            ES_MULTILINE, ES_PASSWORD, GA_ROOT, WM_SETTEXT, WS_CHILD,
        },
    };

    struct TestWindow(HWND);
    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }

    fn edit(text: &str, password: bool) -> (TestWindow, HWND, Target) {
        let root_class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let edit_class: Vec<u16> = "EDIT\0".encode_utf16().collect();
        let root = unsafe {
            CreateWindowExW(
                0,
                root_class.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                100,
                100,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!root.is_null());
        let guard = TestWindow(root);
        let child = unsafe {
            CreateWindowExW(
                0,
                edit_class.as_ptr(),
                ptr::null(),
                WS_CHILD
                    | if password {
                        ES_PASSWORD as u32
                    } else {
                        ES_MULTILINE as u32
                    },
                0,
                0,
                100,
                100,
                root,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!child.is_null());
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        assert_ne!(
            unsafe { SendMessageW(child, WM_SETTEXT, 0, wide.as_ptr() as isize) },
            0
        );
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(root, &mut pid) };
        let target = Target {
            foreground: root,
            root: unsafe { GetAncestor(root, GA_ROOT) },
            pid,
            thread,
        };
        (guard, child, target)
    }

    fn select(window: HWND, start: usize, end: usize) {
        unsafe {
            SendMessageW(window, 0x00B1, start, end as isize);
        }
    }

    #[test]
    fn native_edit_reads_only_the_selected_utf16_range() {
        let prefix = "unselected prefix ";
        let selected = "Hello 世界 🦀";
        let (_root, window, target) = edit(&format!("{prefix}{selected} unselected suffix"), false);
        let start = prefix.encode_utf16().count();
        select(window, start, start + selected.encode_utf16().count());
        match native_edit_selection(&target, window).unwrap() {
            NativeSelection::Text(actual) => assert_eq!(actual, selected),
            _ => panic!("the actual native selection was not read"),
        }
    }

    #[test]
    fn native_edit_empty_selection_and_password_are_not_read_as_text() {
        let (_root, window, target) = edit("A harmless test sentence", false);
        select(window, 5, 5);
        assert!(matches!(
            native_edit_selection(&target, window),
            Ok(NativeSelection::Empty)
        ));
        let (_protected, password, target) = edit("not-a-real-password", true);
        select(password, 0, 5);
        assert!(matches!(
            native_edit_selection(&target, password),
            Err(UiaFailure::Protected)
        ));
    }

    #[test]
    fn native_edit_rejects_other_windows_and_unbounded_document_prefixes() {
        let (_first, first, first_target) = edit("first window", false);
        let (_second, second, _) = edit("second window", false);
        select(second, 0, 6);
        assert!(matches!(
            native_edit_selection(&first_target, second),
            Ok(NativeSelection::Unsupported)
        ));
        select(first, 0, 5);
        assert!(matches!(
            native_edit_selection(&first_target, first),
            Ok(NativeSelection::Text(_))
        ));
        let (_large, large, target) = edit(&"x".repeat(MAX_SELECTION_UNITS + 10), false);
        select(large, MAX_SELECTION_UNITS + 1, MAX_SELECTION_UNITS + 5);
        assert!(matches!(
            native_edit_selection(&target, large),
            Err(UiaFailure::Unavailable(_))
        ));
    }

    #[test]
    fn clipboard_decode_preserves_unicode_and_lines() {
        let bytes: Vec<u8> = " Hello\n世界 🦀 \0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_unicode(&bytes).unwrap(), "Hello\n世界 🦀");
    }

    #[test]
    fn clipboard_decode_rejects_missing_terminator_or_invalid_utf16() {
        assert!(decode_unicode(&[b'a', 0]).is_err());
        assert!(decode_unicode(&[0x00, 0xd8, 0, 0]).is_err());
        assert!(decode_unicode(&[0]).is_err());
    }
}
