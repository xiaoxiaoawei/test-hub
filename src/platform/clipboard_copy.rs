//! Opt-in selection capture by copying. The old clipboard is eagerly copied
//! before input is injected; unsupported formats leave it completely untouched.
//!
//! This cannot undo clipboard-history/monitor side effects. The caller must
//! authorize the operation using a recent physical gesture and a verified,
//! non-password source, and cancel it on a new mouse gesture. A temporary
//! keyboard hook also detects other physical/injected input during the copy.

use std::{
    cell::Cell,
    mem::size_of,
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, GlobalFree, SetLastError, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
            GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber, OpenClipboard,
            SetClipboardData,
        },
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYEVENTF_KEYUP, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, CreateWindowExW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
            GetWindowThreadProcessId, PeekMessageW, SetWindowsHookExW, TranslateMessage,
            UnhookWindowsHookEx, HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
            PM_REMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
        },
    },
};

const CF_TEXT: u32 = 1;
const CF_BITMAP: u32 = 2;
const CF_OEMTEXT: u32 = 7;
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_LOCALE: u32 = 16;
const CF_DIBV5: u32 = 17;
const MAX_FORMATS: usize = 64;
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SELECTION_UNITS: usize = 32_000;
const INPUT_MARKER: usize = 0x4c4c4350;
const COPY_WAIT: Duration = Duration::from_millis(1_000);

thread_local! { static OTHER_KEY_INPUT: Cell<bool> = const { Cell::new(false) }; }

/// Caller validation is required: this function does not authorize automatic
/// capture by itself. `cancelled` must change on a new physical mouse gesture.
pub fn capture(
    foreground: isize,
    expected_sequence: u32,
    cancelled: impl Fn() -> bool,
) -> Result<String, String> {
    capture_checked(foreground, expected_sequence, cancelled).result
}

/// Reports whether input really reached SendInput, independently of preflight
/// refusal. A stale preflight sequence can be retried for the same live gesture.
pub struct CopyOutcome {
    pub result: Result<String, String>,
    pub injected: bool,
    pub retryable: bool,
}

pub fn capture_checked(
    foreground: isize,
    expected_sequence: u32,
    cancelled: impl Fn() -> bool,
) -> CopyOutcome {
    let injected = Cell::new(false);
    let retryable = Cell::new(false);
    let result = capture_inner(
        foreground,
        expected_sequence,
        cancelled,
        &injected,
        &retryable,
    );
    CopyOutcome {
        result,
        injected: injected.get(),
        retryable: retryable.get(),
    }
}

fn capture_inner(
    foreground: isize,
    expected_sequence: u32,
    cancelled: impl Fn() -> bool,
    injected: &Cell<bool>,
    retryable: &Cell<bool>,
) -> Result<String, String> {
    let foreground = foreground as HWND;
    if foreground.is_null() || unsafe { GetForegroundWindow() } != foreground || cancelled() {
        return Err("选区或前台窗口已改变，未执行兼容复制。".into());
    }
    if unsafe { GetClipboardSequenceNumber() } != expected_sequence {
        retryable.set(true);
        return Err("剪贴板已被更新，已保留最新内容，未执行兼容复制。".into());
    }
    let mut source_pid = 0;
    if unsafe { GetWindowThreadProcessId(foreground, &mut source_pid) } == 0 || source_pid == 0 {
        return Err("无法确认取词窗口，未执行兼容复制。".into());
    }
    let observer = InputObserver::install()?;
    let invalidated = || {
        observer.pump();
        cancelled() || observer.has_other_input() || unsafe { GetForegroundWindow() } != foreground
    };
    let released = Instant::now();
    while modifiers_down() {
        if invalidated() {
            return Err("窗口或输入已改变，未执行兼容复制。".into());
        }
        if unsafe { GetClipboardSequenceNumber() } != expected_sequence {
            retryable.set(true);
            return Err("剪贴板已改变，未执行兼容复制。".into());
        }
        if released.elapsed() >= Duration::from_millis(1_200) {
            return Err("请松开修饰键后重试兼容取词。".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    if invalidated() {
        return Err("选区或输入已改变，未执行兼容复制。".into());
    }
    let owner = ClipboardOwner::new()?;
    let (mut snapshot, baseline) = {
        let _lock = ClipboardLock::open(owner.0)?;
        if invalidated() {
            return Err("选区或输入已改变，未执行兼容复制。".into());
        }
        if unsafe { GetClipboardSequenceNumber() } != expected_sequence {
            retryable.set(true);
            return Err("剪贴板已改变，未执行兼容复制。".into());
        }
        let snapshot = Snapshot::take()?;
        // Materializing a delayed old format can advance the sequence. This
        // happened while we held the lock, before any Ctrl+C was sent.
        (snapshot, unsafe { GetClipboardSequenceNumber() })
    };
    if invalidated() || modifiers_down() {
        return Err("窗口或输入已改变，未执行兼容复制。".into());
    }
    if unsafe { GetClipboardSequenceNumber() } != baseline {
        retryable.set(true);
        return Err("剪贴板已改变，未执行兼容复制。".into());
    }
    let input = [
        key(VK_CONTROL, false),
        key(0x43, false),
        key(0x43, true),
        key(VK_CONTROL, true),
    ];
    let sent = unsafe {
        SendInput(
            input.len() as u32,
            input.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    injected.set(sent > 0);
    let incomplete = sent != input.len() as u32;
    if incomplete {
        if sent > 0 {
            let releases = [key(0x43, true), key(VK_CONTROL, true)];
            unsafe {
                SendInput(
                    releases.len() as u32,
                    releases.as_ptr(),
                    size_of::<INPUT>() as i32,
                );
            }
        }
        if sent < 2 {
            return Err("目标窗口不允许兼容复制，请手动复制后翻译剪贴板。".into());
        }
        // C-down may already have copied. Still observe and restore a result
        // proven to belong to this attempt instead of abandoning its side effect.
    }
    let started = Instant::now();
    loop {
        if invalidated() {
            // A new physical input may itself be the user's copy, even in the
            // same process. Do not restore merely because the owner PID matches.
            return Err("检测到新的输入或窗口变化，已保留当前剪贴板内容。".into());
        }
        let sequence = unsafe { GetClipboardSequenceNumber() };
        if sequence != baseline {
            let result = finish_copy(&mut snapshot, owner.0, sequence, source_pid, &invalidated);
            return if incomplete && result.is_ok() {
                Err("兼容复制未完整发送，已恢复原剪贴板，请重试。".into())
            } else {
                result
            };
        }
        if started.elapsed() >= COPY_WAIT {
            return Err("未收到新的复制结果，未把旧剪贴板当作选区。请手动复制后翻译。".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn finish_copy(
    snapshot: &mut Snapshot,
    owner: HWND,
    sequence: u32,
    source_pid: u32,
    invalidated: &impl Fn() -> bool,
) -> Result<String, String> {
    if invalidated() {
        return Err("检测到新的输入，已保留当前剪贴板内容。".into());
    }
    let _lock = ClipboardLock::open(owner)?;
    if unsafe { GetClipboardSequenceNumber() } != sequence {
        return Err("检测到另一次复制，已保留最新剪贴板内容。".into());
    }
    let clipboard_owner = unsafe { GetClipboardOwner() };
    let mut clipboard_pid = 0;
    unsafe {
        GetWindowThreadProcessId(clipboard_owner, &mut clipboard_pid);
    }
    if source_pid == 0 || clipboard_pid != source_pid {
        return Err("无法确认复制来源，已保留最新剪贴板；请手动复制后翻译。".into());
    }
    // Provenance was checked without an intervening input. From here until
    // restoration we continuously hold OpenClipboard, so a new user copy cannot
    // replace the clipboard. A cancellation during this lock must still undo
    // our known copy, rather than leave its temporary clipboard behind.
    let cancelled_before_read = invalidated();
    let selected = if cancelled_before_read {
        Err("选区已改变，已取消兼容取词。".into())
    } else {
        read_unicode()
    };
    let cancelled_after_read = invalidated();
    snapshot.restore()?;
    if cancelled_before_read || cancelled_after_read {
        Err("选区或输入已改变，已恢复原剪贴板并取消取词。".into())
    } else {
        selected
    }
}

struct InputObserver(HHOOK);
impl InputObserver {
    fn install() -> Result<Self, String> {
        OTHER_KEY_INPUT.with(|flag| flag.set(false));
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                GetModuleHandleW(ptr::null()),
                0,
            )
        };
        if hook.is_null() {
            Err("无法安全观察复制期间的输入，未执行兼容复制。".into())
        } else {
            Ok(Self(hook))
        }
    }
    fn pump(&self) {
        let mut message: MSG = unsafe { std::mem::zeroed() };
        // Low-level hook callbacks are delivered to this worker's message loop.
        for _ in 0..32 {
            if unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
                break;
            }
            if message.message == WM_QUIT {
                OTHER_KEY_INPUT.with(|flag| flag.set(true));
                break;
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    fn has_other_input(&self) -> bool {
        OTHER_KEY_INPUT.with(Cell::get)
    }
}
impl Drop for InputObserver {
    fn drop(&mut self) {
        unsafe {
            UnhookWindowsHookEx(self.0);
        }
    }
}
unsafe extern "system" fn keyboard_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 && data != 0 && matches!(message as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let event = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
        if event.flags & LLKHF_INJECTED == 0 || event.dwExtraInfo != INPUT_MARKER {
            OTHER_KEY_INPUT.with(|flag| flag.set(true));
        }
    }
    unsafe { CallNextHookEx(ptr::null_mut(), code, message, data) }
}

fn modifiers_down() -> bool {
    [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&key| unsafe { GetAsyncKeyState(key as i32) } < 0)
}
fn key(value: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: value,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: INPUT_MARKER,
            },
        },
    }
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
            Err("无法创建剪贴板备份窗口，未执行兼容复制。".into())
        } else {
            Ok(Self(window))
        }
    }
}
impl Drop for ClipboardOwner {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
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
                return Err("剪贴板正忙，已取消兼容取词，请稍后重试。".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for ClipboardLock {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

struct FormatData {
    format: u32,
    memory: HGLOBAL,
}
impl Drop for FormatData {
    fn drop(&mut self) {
        if !self.memory.is_null() {
            unsafe {
                GlobalFree(self.memory);
            }
        }
    }
}
struct Snapshot {
    formats: Vec<FormatData>,
}
impl Snapshot {
    /// The caller holds OpenClipboard. All data is copied and all restoration
    /// allocations are prepared before injection; no lazy source object is kept.
    fn take() -> Result<Self, String> {
        let formats = enumerate_formats()?;
        let has_dib = formats.contains(&CF_DIB) || formats.contains(&CF_DIBV5);
        for &format in &formats {
            if format == CF_BITMAP && has_dib {
                continue;
            }
            if !supported_hglobal(format) {
                return Err(
                    "剪贴板含无法完整备份的格式，未执行兼容复制；原内容已保留。请手动复制后翻译。"
                        .into(),
                );
            }
        }
        let mut saved = Vec::new();
        let mut total = 0;
        for &format in &formats {
            // Windows regenerates CF_BITMAP from a materialized DIB. A bitmap
            // handle is not HGLOBAL and must never be copied as raw bytes.
            if format == CF_BITMAP {
                continue;
            }
            let source = unsafe { GetClipboardData(format) };
            if source.is_null() {
                return Err("无法完整读取旧剪贴板，未执行兼容复制。".into());
            }
            let length = unsafe { GlobalSize(source) };
            if length == 0 || length > MAX_SNAPSHOT_BYTES - total {
                return Err("旧剪贴板过大或格式不可复制，未执行兼容复制。".into());
            }
            total += length;
            saved.push(duplicate_memory(format, source, length)?);
        }
        if enumerate_formats()? != formats {
            return Err("备份过程中剪贴板格式改变，未执行兼容复制。".into());
        }
        Ok(Self { formats: saved })
    }
    fn restore(&mut self) -> Result<(), String> {
        if unsafe { EmptyClipboard() } == 0 {
            return Err("兼容取词后未能恢复原剪贴板，请手动重新复制所需内容。".into());
        }
        for data in &mut self.formats {
            if unsafe { SetClipboardData(data.format, data.memory) }.is_null() {
                return Err("兼容取词后未能完整恢复剪贴板，请手动重新复制所需内容。".into());
            }
            data.memory = ptr::null_mut(); // Ownership transferred to Windows.
        }
        Ok(())
    }
}
fn enumerate_formats() -> Result<Vec<u32>, String> {
    let mut formats = Vec::new();
    let mut previous = 0;
    loop {
        unsafe {
            SetLastError(0);
        }
        let format = unsafe { EnumClipboardFormats(previous) };
        if format == 0 {
            if unsafe { GetLastError() } != 0 {
                return Err("无法枚举剪贴板，未执行兼容复制。".into());
            }
            return Ok(formats);
        }
        if formats.len() >= MAX_FORMATS || formats.contains(&format) {
            return Err("剪贴板格式过多或无效，未执行兼容复制。".into());
        }
        formats.push(format);
        previous = format;
    }
}
fn supported_hglobal(format: u32) -> bool {
    if matches!(
        format,
        CF_TEXT | CF_OEMTEXT | CF_UNICODETEXT | CF_LOCALE | CF_DIB | CF_DIBV5
    ) {
        return true;
    }
    if format < 0xc000 {
        return false;
    }
    let mut name = [0_u16; 256];
    let length = unsafe { GetClipboardFormatNameW(format, name.as_mut_ptr(), name.len() as i32) };
    if length <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&name[..length as usize]);
    [
        "HTML Format",
        "Rich Text Format",
        "Rich Text Format Without Objects",
        "PNG",
        "image/png",
        "text/html",
        "UniformResourceLocator",
        "UniformResourceLocatorW",
        "Chromium Web Custom MIME Data Format",
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ]
    .iter()
    .any(|allowed| name.eq_ignore_ascii_case(allowed))
}
fn duplicate_memory(format: u32, source: HGLOBAL, length: usize) -> Result<FormatData, String> {
    let input = unsafe { GlobalLock(source) };
    if input.is_null() {
        return Err("无法锁定旧剪贴板数据，未执行兼容复制。".into());
    }
    let mut result = FormatData {
        format,
        memory: unsafe { GlobalAlloc(GMEM_MOVEABLE, length) },
    };
    if result.memory.is_null() {
        unsafe {
            GlobalUnlock(source);
        }
        return Err("无法为旧剪贴板分配备份内存，未执行兼容复制。".into());
    }
    let output = unsafe { GlobalLock(result.memory) };
    if output.is_null() {
        unsafe {
            GlobalUnlock(source);
        }
        return Err("无法锁定备份内存，未执行兼容复制。".into());
    }
    unsafe {
        ptr::copy_nonoverlapping(input.cast::<u8>(), output.cast::<u8>(), length);
        GlobalUnlock(result.memory);
        GlobalUnlock(source);
    }
    // Keep ownership explicit; Drop frees it until restore transfers it.
    result.format = format;
    Ok(result)
}
fn read_unicode() -> Result<String, String> {
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
    if handle.is_null() {
        return Err("本次复制没有产生文字，原剪贴板已恢复。".into());
    }
    let length = unsafe { GlobalSize(handle) };
    if length < 2 || length % 2 != 0 || length > (MAX_SELECTION_UNITS + 1) * 2 {
        return Err("本次复制文字过长或格式无效，原剪贴板已恢复。".into());
    }
    let data = unsafe { GlobalLock(handle) };
    if data.is_null() {
        return Err("无法读取本次复制的文字，原剪贴板已恢复。".into());
    }
    let units = unsafe { std::slice::from_raw_parts(data.cast::<u16>(), length / 2) };
    let result = match units.iter().position(|&unit| unit == 0) {
        Some(end) if end <= MAX_SELECTION_UNITS => String::from_utf16(&units[..end])
            .map_err(|_| "复制结果包含无效的 Unicode，原剪贴板已恢复。".to_owned()),
        _ => Err("复制结果格式无效或过长，原剪贴板已恢复。".into()),
    };
    unsafe {
        GlobalUnlock(handle);
    }
    let text = result?.trim().to_owned();
    if text.is_empty() {
        Err("本次复制没有选中文字，原剪贴板已恢复。".into())
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};
    use windows_sys::Win32::System::{
        DataExchange::RegisterClipboardFormatW, Threading::GetCurrentProcessId,
    };

    static CLIPBOARD_TEST: Mutex<()> = Mutex::new(());
    struct Fixture {
        _serial: MutexGuard<'static, ()>,
        owner: ClipboardOwner,
        original: Snapshot,
    }
    impl Fixture {
        fn new() -> Self {
            let serial = CLIPBOARD_TEST
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let owner = ClipboardOwner::new().unwrap();
            let original = {
                let _lock = ClipboardLock::open(owner.0).unwrap();
                Snapshot::take()
                    .expect("tests require a clipboard whose old formats can be safely backed up")
            };
            Self {
                _serial: serial,
                owner,
                original,
            }
        }
        fn set(&self, values: &[(u32, Vec<u8>)]) {
            let _lock = ClipboardLock::open(self.owner.0).unwrap();
            assert_ne!(unsafe { EmptyClipboard() }, 0);
            for (format, bytes) in values {
                let data = allocation(*format, bytes);
                assert!(!unsafe { SetClipboardData(*format, data.memory) }.is_null());
                std::mem::forget(data);
            }
        }
        fn snapshot(&self) -> Snapshot {
            let _lock = ClipboardLock::open(self.owner.0).unwrap();
            Snapshot::take().unwrap()
        }
        fn bytes(&self, format: u32) -> Vec<u8> {
            let _lock = ClipboardLock::open(self.owner.0).unwrap();
            let data = unsafe { GetClipboardData(format) };
            assert!(!data.is_null());
            let size = unsafe { GlobalSize(data) };
            let pointer = unsafe { GlobalLock(data) };
            assert!(!pointer.is_null());
            let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec() };
            unsafe {
                GlobalUnlock(data);
            }
            bytes
        }
        fn restore(&self, snapshot: &mut Snapshot) {
            let _lock = ClipboardLock::open(self.owner.0).unwrap();
            snapshot.restore().unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(_lock) = ClipboardLock::open(self.owner.0) {
                let _ = self.original.restore();
            }
        }
    }
    fn register(name: &str) -> u32 {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
        assert_ne!(format, 0);
        format
    }
    fn unicode(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    fn allocation(format: u32, bytes: &[u8]) -> FormatData {
        let data = FormatData {
            format,
            memory: unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) },
        };
        assert!(!data.memory.is_null());
        let pointer = unsafe { GlobalLock(data.memory) };
        assert!(!pointer.is_null());
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
            GlobalUnlock(data.memory);
        }
        data
    }

    #[test]
    fn empty_clipboard_roundtrip_restores_empty() {
        let fixture = Fixture::new();
        fixture.set(&[]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, unicode("temporary selection"))]);
        fixture.restore(&mut saved);
        let _lock = ClipboardLock::open(fixture.owner.0).unwrap();
        assert!(enumerate_formats().unwrap().is_empty());
    }
    #[test]
    fn unicode_with_synthesized_text_formats_roundtrips() {
        let fixture = Fixture::new();
        let old = unicode("旧文本\nkeep me");
        fixture.set(&[(CF_UNICODETEXT, old.clone())]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, unicode("new selection"))]);
        fixture.restore(&mut saved);
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..old.len()], old);
        let _lock = ClipboardLock::open(fixture.owner.0).unwrap();
        assert!(!unsafe { GetClipboardData(CF_TEXT) }.is_null());
    }
    #[test]
    fn html_and_unicode_are_both_preserved() {
        let fixture = Fixture::new();
        let html_format = register("HTML Format");
        let html = b"Version:1.0\r\n<html><body><b>keep bold</b></body></html>\0".to_vec();
        let old = unicode("keep bold");
        fixture.set(&[(html_format, html.clone()), (CF_UNICODETEXT, old.clone())]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, unicode("new"))]);
        fixture.restore(&mut saved);
        assert_eq!(fixture.bytes(html_format)[..html.len()], html);
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..old.len()], old);
    }
    #[test]
    fn dib_image_is_preserved_and_bitmap_is_regenerated() {
        let fixture = Fixture::new();
        let mut dib = vec![0u8; 44];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[20..24].copy_from_slice(&4u32.to_le_bytes());
        dib[40..44].copy_from_slice(&[10, 20, 30, 255]);
        fixture.set(&[(CF_DIB, dib.clone())]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, unicode("new"))]);
        fixture.restore(&mut saved);
        assert_eq!(fixture.bytes(CF_DIB)[..dib.len()], dib);
        let _lock = ClipboardLock::open(fixture.owner.0).unwrap();
        assert!(!unsafe { GetClipboardData(CF_BITMAP) }.is_null());
    }
    #[test]
    fn unknown_format_refuses_snapshot_without_mutating_clipboard() {
        let fixture = Fixture::new();
        let unsupported = register("LingoLeaf.Tests.UnsupportedDataObject");
        fixture.set(&[
            (unsupported, vec![1, 2, 3, 4]),
            (CF_UNICODETEXT, unicode("keep")),
        ]);
        let sequence = unsafe { GetClipboardSequenceNumber() };
        {
            let _lock = ClipboardLock::open(fixture.owner.0).unwrap();
            assert!(Snapshot::take().is_err());
        }
        assert_eq!(unsafe { GetClipboardSequenceNumber() }, sequence);
        assert_eq!(fixture.bytes(unsupported)[..4], [1, 2, 3, 4]);
    }
    #[test]
    fn owned_copy_returns_new_text_and_restores_all_old_formats() {
        let fixture = Fixture::new();
        let html_format = register("HTML Format");
        let html = b"<b>preserve old formatting</b>\0".to_vec();
        let old = unicode("preserve old formatting");
        fixture.set(&[(html_format, html.clone()), (CF_UNICODETEXT, old.clone())]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, unicode("Fresh selected text."))]);
        let sequence = unsafe { GetClipboardSequenceNumber() };
        let selected = finish_copy(
            &mut saved,
            fixture.owner.0,
            sequence,
            unsafe { GetCurrentProcessId() },
            &|| false,
        )
        .unwrap();
        assert_eq!(selected, "Fresh selected text.");
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..old.len()], old);
        assert_eq!(fixture.bytes(html_format)[..html.len()], html);
    }
    #[test]
    fn invalid_new_text_still_restores_the_old_clipboard() {
        let fixture = Fixture::new();
        let old = unicode("keep this even if the copy result is broken");
        fixture.set(&[(CF_UNICODETEXT, old.clone())]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, vec![0, 0xd8, 0, 0])]);
        let sequence = unsafe { GetClipboardSequenceNumber() };
        assert!(finish_copy(
            &mut saved,
            fixture.owner.0,
            sequence,
            unsafe { GetCurrentProcessId() },
            &|| false
        )
        .is_err());
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..old.len()], old);
    }
    #[test]
    fn changed_sequence_never_restores_over_new_user_content() {
        let fixture = Fixture::new();
        fixture.set(&[(CF_UNICODETEXT, unicode("old"))]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, unicode("our copy"))]);
        let observed = unsafe { GetClipboardSequenceNumber() };
        let latest = unicode("user copied something newer");
        fixture.set(&[(CF_UNICODETEXT, latest.clone())]);
        assert!(finish_copy(
            &mut saved,
            fixture.owner.0,
            observed,
            unsafe { GetCurrentProcessId() },
            &|| false
        )
        .is_err());
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..latest.len()], latest);
    }
    #[test]
    fn cancellation_before_provenance_keeps_latest_clipboard() {
        let fixture = Fixture::new();
        fixture.set(&[(CF_UNICODETEXT, unicode("old"))]);
        let mut saved = fixture.snapshot();
        let latest = unicode("user copy");
        fixture.set(&[(CF_UNICODETEXT, latest.clone())]);
        let sequence = unsafe { GetClipboardSequenceNumber() };
        assert!(finish_copy(
            &mut saved,
            fixture.owner.0,
            sequence,
            unsafe { GetCurrentProcessId() },
            &|| true
        )
        .is_err());
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..latest.len()], latest);
    }
    #[test]
    fn cancellation_after_lock_restores_known_copy_before_returning() {
        let fixture = Fixture::new();
        let old = unicode("old");
        fixture.set(&[(CF_UNICODETEXT, old.clone())]);
        let mut saved = fixture.snapshot();
        fixture.set(&[(CF_UNICODETEXT, unicode("our temporary copy"))]);
        let sequence = unsafe { GetClipboardSequenceNumber() };
        let calls = Cell::new(0);
        let invalidated = || {
            let before = calls.get();
            calls.set(before + 1);
            before > 0
        };
        assert!(finish_copy(
            &mut saved,
            fixture.owner.0,
            sequence,
            unsafe { GetCurrentProcessId() },
            &invalidated
        )
        .is_err());
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..old.len()], old);
    }
    #[test]
    fn foreign_owner_is_not_treated_as_this_selection() {
        let fixture = Fixture::new();
        fixture.set(&[(CF_UNICODETEXT, unicode("old"))]);
        let mut saved = fixture.snapshot();
        let latest = unicode("unknown source");
        fixture.set(&[(CF_UNICODETEXT, latest.clone())]);
        let sequence = unsafe { GetClipboardSequenceNumber() };
        assert!(finish_copy(&mut saved, fixture.owner.0, sequence, u32::MAX, &|| false).is_err());
        assert_eq!(fixture.bytes(CF_UNICODETEXT)[..latest.len()], latest);
    }
}
