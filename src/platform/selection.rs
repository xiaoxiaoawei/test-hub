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
            CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationTextPattern,
            UIA_TextPatternId,
        },
    },
};
use windows_sys::Win32::{
    Foundation::{GetLastError, GlobalFree, SetLastError, HGLOBAL, HWND},
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
            CreateWindowExW, DestroyWindow, GetForegroundWindow, GetWindowThreadProcessId,
            HWND_MESSAGE,
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

enum UiaFailure {
    Protected,
    Unavailable(String),
}

/// Capture the current foreground selection. Automatic mouse selection must pass
/// `false`; only a deliberate user hotkey is allowed to pass `true`.
pub fn capture_selection(allow_clipboard_fallback: bool) -> Result<CapturedText, String> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return Err("没有可读取的前台窗口。".into());
    }

    match capture_uia() {
        Ok(text) => {
            if unsafe { GetForegroundWindow() } != foreground {
                return Err("前台窗口已改变，请重新选择文字。".into());
            }
            Ok(CapturedText {
                text,
                source: "UI Automation",
            })
        }
        Err(UiaFailure::Protected) => Err("不会读取密码字段或无法确认安全属性的控件。".into()),
        Err(UiaFailure::Unavailable(reason)) if allow_clipboard_fallback => {
            capture_by_copy(foreground).map_err(|fallback| format!("{reason}\n{fallback}"))
        }
        Err(UiaFailure::Unavailable(reason)) => Err(reason),
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

fn capture_uia() -> Result<String, UiaFailure> {
    let _apartment = ComApartment::initialize()?;
    // Keep every COM object inside the apartment's lifetime. A provider may still
    // misbehave; the caller must not wait on this thread from its UI thread.
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
                .map_err(|error| {
                    UiaFailure::Unavailable(format!("此窗口暂时无法读取选区：{error}"))
                })?;
        if let Ok(settings) = automation.cast::<IUIAutomation2>() {
            let _ = settings.SetConnectionTimeout(1_000);
            let _ = settings.SetTransactionTimeout(1_200);
        }
        let mut element = automation.GetFocusedElement().map_err(|_| {
            UiaFailure::Unavailable("无法定位选中文字，请手动复制后使用“翻译剪贴板”。".into())
        })?;

        // Failing closed here prevents a failed accessibility property read from
        // silently falling through to simulated copy in a protected input.
        match element.CurrentIsPassword() {
            Ok(value) if !value.as_bool() => {}
            _ => return Err(UiaFailure::Protected),
        }

        let walker = automation.ControlViewWalker().ok();
        for _ in 0..7 {
            if element
                .CurrentIsPassword()
                .map(|value| value.as_bool())
                .unwrap_or(true)
            {
                return Err(UiaFailure::Protected);
            }
            if let Ok(pattern) =
                element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            {
                if let Some(selected) = selected_text(&pattern)? {
                    return Ok(selected);
                }
            }
            let Some(walker) = &walker else { break };
            let Ok(parent) = walker.GetParentElement(&element) else {
                break;
            };
            // Do not climb into unrelated windows or read the desktop tree.
            if parent.CurrentProcessId().ok() != element.CurrentProcessId().ok() {
                break;
            }
            element = parent;
        }
    }
    Err(UiaFailure::Unavailable(
        "未读取到选中文字；此软件可能未提供文本选区接口。".into(),
    ))
}

unsafe fn selected_text(pattern: &IUIAutomationTextPattern) -> Result<Option<String>, UiaFailure> {
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
        let range = unsafe { ranges.GetElement(index) }
            .map_err(|_| UiaFailure::Unavailable("选区已失效，请重新选择文字。".into()))?;
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
    use super::decode_unicode;

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
