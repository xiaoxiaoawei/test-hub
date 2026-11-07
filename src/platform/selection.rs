//! Selection capture runs on a worker thread, never in a hook or window procedure.
//!
//! UI Automation is read-only. Explicit copy fallback uses a shared clipboard
//! backend that eagerly snapshots supported formats and refuses incomplete
//! backups before sending any input.

use std::{
    cell::RefCell,
    mem::size_of,
    ptr, thread,
    time::{Duration, Instant},
};

use crate::selection_text::has_visible_text;

use windows::{
    core::Interface,
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        },
        System::Ole::{
            SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElemsize,
            SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData,
        },
        UI::Accessibility::{
            CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2,
            IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationTextPattern,
            IUIAutomationTextPattern2, IUIAutomationTextRange, IUIAutomationTreeWalker,
            SupportedTextSelection_None, TreeScope_Element, UIA_BoundingRectanglePropertyId,
            UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
            UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
            UIA_IsTextPattern2AvailablePropertyId, UIA_IsTextPatternAvailablePropertyId,
            UIA_NativeWindowHandlePropertyId, UIA_ProcessIdPropertyId, UIA_TextControlTypeId,
            UIA_TextPattern2Id, UIA_TextPatternId, UIA_WindowControlTypeId,
        },
    },
};

#[path = "selection/probe.rs"]
mod probe;
#[cfg(test)]
#[path = "selection/provider_tests.rs"]
mod provider_tests;
use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND, POINT, RECT},
    Graphics::Gdi::ClientToScreen,
    System::{
        DataExchange::{
            CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
    },
    UI::WindowsAndMessaging::{
        GetAncestor, GetClassNameW, GetClientRect, GetCursorPos, GetForegroundWindow,
        GetGUIThreadInfo, GetWindowLongPtrW, GetWindowThreadProcessId, IsChild,
        SendMessageTimeoutW, WindowFromPoint, ES_PASSWORD, GA_ROOT, GUITHREADINFO, GWL_STYLE,
        SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, WM_GETTEXT,
    },
};

const UNICODE_TEXT: u32 = 13;
const MAX_SELECTION_UNITS: usize = 32_000;
const MAX_CLIPBOARD_BYTES: usize = 8 * 1024 * 1024;
const NO_SELECTION_MESSAGE: &str = "未读取到选中文字，请重新选择。";
const PROTECTED_MESSAGE: &str = "不会读取密码字段或无法确认安全属性的文本控件。";

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
    capture(None, None, allow_clipboard_fallback)
}

/// Read a mouse selection without changing the clipboard. Hit testing matters
/// for documents whose text selection does not move Windows keyboard focus.
pub fn capture_selection_at(point: POINT) -> Result<CapturedText, String> {
    capture(Some(point), None, false)
}

/// Associate a mouse selection with its physical gesture so that an old address
/// bar selection cannot win over the PDF region where the drag occurred.
pub fn capture_selection_for_gesture(
    origin: Option<POINT>,
    point: POINT,
) -> Result<CapturedText, String> {
    capture(Some(point), origin, false)
}

/// Eligibility only; this never copies, reads selected text, or changes focus.
/// The caller must separately require the user's compatibility setting, a real
/// selection gesture, and an unchanged foreground/clipboard before simulating C.
pub fn can_copy_gesture(origin: Option<POINT>, point: POINT) -> bool {
    let Ok(target) = Target::foreground() else {
        return false;
    };
    let Some(anchor) = source_hit_point(&target, Some(point), origin) else {
        return false;
    };
    let root_class = native_class(target.root);
    if !browser_window(target.root)
        && !matches!(
            root_class.as_str(),
            "SUMATRA_PDF_FRAME" | "AcrobatSDIWindow"
        )
    {
        return false;
    }
    // A PDF drag must not copy a toolbar EDIT's retained selection. Ignore an
    // unrelated release over our nonactivating popup; its source origin remains.
    for hit in [Some(point), origin]
        .into_iter()
        .flatten()
        .map(|point| unsafe { WindowFromPoint(point) })
    {
        if target.owns_window(hit) && native_edit_class(&native_class(hit)) {
            return false;
        }
    }
    let hit = unsafe { WindowFromPoint(anchor) };
    let native_content = probe::native_document_class(&root_class, &native_class(hit));
    let eligible = with_uia(|session| unsafe {
        let deadline = Instant::now() + Duration::from_millis(220);
        let element = session
            .automation
            .ElementFromPointBuildCache(
                windows::Win32::Foundation::POINT {
                    x: anchor.x,
                    y: anchor.y,
                },
                &session.cache,
            )
            .map_err(uia_setup_error)?;
        let chain = scoped_chain(
            &target,
            UiaNode::cached(element),
            session,
            &session.raw,
            deadline,
        );
        let Some(leaf) = chain.first() else {
            return Ok(false);
        };
        match leaf.element.CurrentIsPassword() {
            Ok(value) if value.as_bool() => return Err(UiaFailure::Protected),
            Ok(_) if leaf.password == Some(false) => {}
            _ => return Err(UiaFailure::Unverified),
        }
        if chain
            .iter()
            .any(|node| node.metadata.edit || node.password == Some(true))
        {
            return Ok(false);
        }
        let document = chain.iter().any(|node| {
            node.metadata.document
                && node.password == Some(false)
                && probe::priority(node.metadata, Some((anchor.x, anchor.y)), 0, 0).is_some()
        });
        Ok(native_content || document)
    });
    let eligible = match eligible {
        Ok(eligible) => eligible,
        Err(UiaFailure::Unavailable(_)) => native_content,
        Err(UiaFailure::Protected | UiaFailure::Unverified) => false,
    };
    eligible && unsafe { GetForegroundWindow() } == target.foreground
}

fn capture(
    point: Option<POINT>,
    origin: Option<POINT>,
    allow_copy: bool,
) -> Result<CapturedText, String> {
    let target = Target::foreground()?;
    let source_point = source_hit_point(&target, point, origin);
    if point.is_some() && source_point.is_none() {
        return Err(no_selection());
    }
    let point_window = source_point.map(|point| unsafe { WindowFromPoint(point) });
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

    let uia = capture_uia(&target, point, origin);
    match uia {
        Ok(text) => target.finish(text, "UI Automation"),
        Err(UiaFailure::Protected) => Err(protected_message()),
        Err(failure) => {
            let (reason, allow_copy) = match failure {
                UiaFailure::Unverified => (protected_message(), false),
                UiaFailure::Unavailable(reason) => (reason, allow_copy),
                UiaFailure::Protected => unreachable!("protected candidate handled above"),
            };
            // A focused toolbar EDIT can retain an old selection while the user
            // drags in a PDF. Automatic fallback needs an actual gesture hit on
            // that control (or a child), not just the same foreground root.
            if let Some(window) = focus_window {
                let hits = [point, origin]
                    .into_iter()
                    .flatten()
                    .map(|point| unsafe { WindowFromPoint(point) });
                if Some(window) != native_first
                    && focused_native_fallback_allowed(&target, window, point.is_some(), hits)
                {
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

fn focused_native_fallback_allowed(
    target: &Target,
    focused: HWND,
    automatic: bool,
    hits: impl IntoIterator<Item = HWND>,
) -> bool {
    target.owns_window(focused)
        && (!automatic
            || hits.into_iter().any(|hit| {
                target.owns_window(hit) && (hit == focused || unsafe { IsChild(focused, hit) } != 0)
            }))
}

fn no_selection() -> String {
    NO_SELECTION_MESSAGE.into()
}
fn protected_message() -> String {
    PROTECTED_MESSAGE.into()
}

fn source_hit_point(target: &Target, end: Option<POINT>, origin: Option<POINT>) -> Option<POINT> {
    // A nonactivating translation popup can cover the drag's release point.
    // Locate the source at the original press instead; never probe our popup
    // or accept a UIA element merely because it belongs to this process.
    choose_source_point(end, origin, |point| {
        target.owns_window(unsafe { WindowFromPoint(point) })
    })
}

fn choose_source_point(
    end: Option<POINT>,
    origin: Option<POINT>,
    mut owned: impl FnMut(POINT) -> bool,
) -> Option<POINT> {
    end.filter(|point| owned(*point))
        .or_else(|| origin.filter(|point| owned(*point)))
}

pub fn is_no_selection_error(message: &str) -> bool {
    message == NO_SELECTION_MESSAGE
}

pub fn is_protected_error(message: &str) -> bool {
    message == PROTECTED_MESSAGE
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
        if !has_visible_text(&text) {
            return Err(no_selection());
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
    Ok(if !has_visible_text(&selected) {
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
const MAX_ANCESTORS: usize = 12;

// Keep the apartment and automation object on their creating thread. Repeated
// drags and delayed PDF-provider retries should not reconnect COM each time.
thread_local! {
    static UIA_SESSION: RefCell<Option<UiaSession>> = const { RefCell::new(None) };
}

struct UiaSession {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    raw: IUIAutomationTreeWalker,
    control: IUIAutomationTreeWalker,
    // Fields drop in declaration order: release the COM objects before COM.
    _apartment: ComApartment,
}

impl UiaSession {
    fn new() -> Result<Self, UiaFailure> {
        let apartment = ComApartment::initialize()?;
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                    .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
                    .map_err(|_| UiaFailure::Unavailable("此窗口暂时无法读取选区。".into()))?;
            if let Ok(settings) = automation.cast::<IUIAutomation2>() {
                let _ = settings.SetConnectionTimeout(300);
                let _ = settings.SetTransactionTimeout(200);
            }
            let cache = automation.CreateCacheRequest().map_err(uia_setup_error)?;
            cache
                .SetTreeScope(TreeScope_Element)
                .map_err(uia_setup_error)?;
            cache
                .SetTreeFilter(&automation.RawViewCondition().map_err(uia_setup_error)?)
                .map_err(uia_setup_error)?;
            // One provider transaction returns the geometry and safety metadata.
            // No Name, Value, DocumentRange or document text is cached.
            for property in [
                UIA_ProcessIdPropertyId,
                UIA_NativeWindowHandlePropertyId,
                UIA_IsPasswordPropertyId,
                UIA_BoundingRectanglePropertyId,
                UIA_IsOffscreenPropertyId,
                UIA_ControlTypePropertyId,
                UIA_IsTextPatternAvailablePropertyId,
                UIA_IsTextPattern2AvailablePropertyId,
            ] {
                cache.AddProperty(property).map_err(uia_setup_error)?;
            }
            let raw = automation.RawViewWalker().map_err(uia_setup_error)?;
            let control = automation.ControlViewWalker().map_err(uia_setup_error)?;
            Ok(Self {
                automation,
                cache,
                raw,
                control,
                _apartment: apartment,
            })
        }
    }
}

fn uia_setup_error(_: windows::core::Error) -> UiaFailure {
    UiaFailure::Unavailable("此窗口暂时无法读取选区。".into())
}

fn with_uia<T>(
    operation: impl FnOnce(&UiaSession) -> Result<T, UiaFailure>,
) -> Result<T, UiaFailure> {
    UIA_SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(UiaSession::new()?);
        }
        operation(slot.as_ref().expect("initialized UIA session"))
    })
}

#[derive(Clone)]
struct UiaNode {
    element: IUIAutomationElement,
    native: HWND,
    pid: i32,
    password: Option<bool>,
    text_available: Option<bool>,
    text2_available: Option<bool>,
    text_candidate: bool,
    window: bool,
    metadata: probe::Metadata,
}

#[derive(Default)]
struct ProbeFailures {
    unverified: bool,
    unavailable: Option<String>,
}

impl ProbeFailures {
    fn observe(
        &mut self,
        result: Result<Option<String>, UiaFailure>,
    ) -> Result<Option<String>, UiaFailure> {
        match result {
            Err(UiaFailure::Protected) => Err(UiaFailure::Protected),
            Err(UiaFailure::Unverified) => {
                self.unverified = true;
                Ok(None)
            }
            Err(UiaFailure::Unavailable(reason)) => {
                self.unavailable = Some(reason);
                Ok(None)
            }
            other => other,
        }
    }
}

impl UiaNode {
    fn may_have_text(&self) -> bool {
        probe::should_request_pattern(
            self.text_available,
            self.text2_available,
            self.text_candidate,
        )
    }

    unsafe fn cached(element: IUIAutomationElement) -> Self {
        unsafe {
            let bounds = element
                .CachedBoundingRectangle()
                .ok()
                .map(|rect| probe::Bounds {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                });
            let kind = element.CachedControlType().ok();
            Self {
                native: element
                    .CachedNativeWindowHandle()
                    .ok()
                    .map(|window| window.0)
                    .unwrap_or(ptr::null_mut()),
                pid: element.CachedProcessId().unwrap_or(0),
                password: element.CachedIsPassword().ok().map(|value| value.as_bool()),
                text_available: cached_bool(&element, UIA_IsTextPatternAvailablePropertyId),
                text2_available: cached_bool(&element, UIA_IsTextPattern2AvailablePropertyId),
                text_candidate: kind.is_some_and(|kind| {
                    [
                        UIA_DocumentControlTypeId,
                        UIA_EditControlTypeId,
                        UIA_TextControlTypeId,
                    ]
                    .contains(&kind)
                }),
                window: kind == Some(UIA_WindowControlTypeId),
                metadata: probe::Metadata {
                    bounds,
                    offscreen: element
                        .CachedIsOffscreen()
                        .is_ok_and(|value| value.as_bool()),
                    document: kind == Some(UIA_DocumentControlTypeId),
                    edit: kind == Some(UIA_EditControlTypeId),
                },
                element,
            }
        }
    }
}

unsafe fn cached_bool(
    element: &IUIAutomationElement,
    property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Option<bool> {
    let value = unsafe { element.GetCachedPropertyValue(property) }.ok()?;
    bool::try_from(&value).ok()
}

/// Connect browser accessibility before a drag, without reading any text. This
/// belongs on the capture worker; a provider call must never run in a UI hook.
/// Chromium PDF accessibility trees may still load asynchronously after this.
pub fn warm_selection_provider(foreground: isize) {
    let Ok(target) = Target::foreground() else {
        return;
    };
    if target.foreground as isize != foreground || !browser_window(target.root) {
        return;
    }
    let _ = with_uia(|session| unsafe {
        let deadline = Instant::now() + Duration::from_millis(350);
        // Foreground activation often happens while the cursor is on a taskbar
        // or toolbar. Hit the browser client centre first, then its cursor hit;
        // avoid spending the whole warm budget enumerating browser chrome.
        for point in browser_probe_points(target.root) {
            if Instant::now() >= deadline || GetForegroundWindow() != target.foreground {
                break;
            }
            if !target.owns_window(WindowFromPoint(point)) {
                continue;
            }
            let Ok(element) = session.automation.ElementFromPointBuildCache(
                windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                },
                &session.cache,
            ) else {
                continue;
            };
            let first = UiaNode::cached(element);
            let chain = scoped_chain(
                &target,
                first,
                session,
                &session.raw,
                (Instant::now() + Duration::from_millis(100)).min(deadline),
            );
            for node in &chain {
                if Instant::now() >= deadline {
                    break;
                }
                if node.metadata.document
                    && node.password == Some(false)
                    && text_pattern(node, true).is_some()
                {
                    // Only request the pattern; never read selection or text.
                    return Ok(());
                }
            }
            if let Some(scope) = subtree_scope(&chain) {
                search_subtree(
                    &target,
                    session,
                    &session.raw,
                    scope,
                    SearchOptions {
                        point: Some(point),
                        deadline: (Instant::now() + Duration::from_millis(120)).min(deadline),
                        read_text: false,
                        gesture: None,
                    },
                    &mut probe::Budget::new(32, 6),
                )?;
            }
        }
        // The fallback stays inside this foreground HWND. Control view omits
        // many toolbar implementation nodes; the cache itself includes Raw view.
        if Instant::now() < deadline {
            let element = session
                .automation
                .ElementFromHandleBuildCache(
                    windows::Win32::Foundation::HWND(target.root),
                    &session.cache,
                )
                .map_err(uia_setup_error)?;
            let root = UiaNode::cached(element);
            if root.native == target.root {
                search_subtree(
                    &target,
                    session,
                    &session.control,
                    root,
                    SearchOptions {
                        point: None,
                        deadline,
                        read_text: false,
                        gesture: None,
                    },
                    &mut probe::Budget::new(32, 6),
                )?;
            }
        }
        Ok(())
    });
}

fn browser_probe_points(window: HWND) -> Vec<POINT> {
    let mut points = Vec::with_capacity(2);
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetClientRect(window, &mut rect) } != 0
        && rect.right > rect.left
        && rect.bottom > rect.top
    {
        let mut centre = POINT {
            x: rect.left + (rect.right - rect.left) / 2,
            y: rect.top + (rect.bottom - rect.top) / 2,
        };
        if unsafe { ClientToScreen(window, &mut centre) } != 0 {
            points.push(centre);
        }
    }
    let mut cursor = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } != 0
        && !points
            .iter()
            .any(|point| point.x == cursor.x && point.y == cursor.y)
    {
        points.push(cursor);
    }
    points
}

fn browser_window(window: HWND) -> bool {
    let class = native_class(window);
    class.starts_with("Chrome_WidgetWin_") || class == "MozillaWindowClass"
}

fn native_edit_class(class: &str) -> bool {
    class.eq_ignore_ascii_case("edit") || class.to_ascii_lowercase().starts_with("richedit")
}

fn native_class(window: HWND) -> String {
    let mut class = [0u16; 64];
    let count = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    String::from_utf16_lossy(&class[..count.max(0) as usize])
}

fn capture_uia(
    target: &Target,
    point: Option<POINT>,
    origin: Option<POINT>,
) -> Result<String, UiaFailure> {
    with_uia(|session| unsafe {
        // A transaction can exceed this soft deadline inside an external COM
        // provider. The worker remains isolated from painting and mouse hooks.
        let deadline = Instant::now() + UIA_BUDGET;
        let gesture = point.map(|end| probe::Gesture {
            origin: origin.map(|start| (start.x, start.y)),
            end: (end.x, end.y),
        });
        let anchor = source_hit_point(target, point, origin);
        let mut failures = ProbeFailures::default();
        let mut point_scope = None;
        if let Some(point) = anchor {
            if target.owns_window(WindowFromPoint(point)) {
                if let Ok(element) = session.automation.ElementFromPointBuildCache(
                    windows::Win32::Foundation::POINT {
                        x: point.x,
                        y: point.y,
                    },
                    &session.cache,
                ) {
                    let first = UiaNode::cached(element);
                    // A directly verified document/text hit should get its
                    // first pattern request before walking provider ancestry.
                    if !first.native.is_null() && target.owns_window(first.native) {
                        if let Some(text) = failures.observe(chain_text(
                            std::slice::from_ref(&first),
                            Some(point),
                            gesture,
                            (Instant::now() + Duration::from_millis(120)).min(deadline),
                        ))? {
                            return Ok(text);
                        }
                    }
                    let mut chain = scoped_chain(
                        target,
                        first.clone(),
                        session,
                        &session.raw,
                        (Instant::now() + Duration::from_millis(180)).min(deadline),
                    );
                    if chain.is_empty() && Instant::now() < deadline {
                        chain = scoped_chain(
                            target,
                            first,
                            session,
                            &session.control,
                            (Instant::now() + Duration::from_millis(120)).min(deadline),
                        );
                    }
                    if let Some(text) = failures.observe(chain_text(
                        &chain,
                        Some(point),
                        gesture,
                        (Instant::now() + Duration::from_millis(160)).min(deadline),
                    ))? {
                        return Ok(text);
                    }
                    point_scope = subtree_scope(&chain);
                }
                // A PDF's TextPattern may be on an embedded descendant of the
                // hit-tested host, rather than on a hit/focus ancestor. Reserve
                // time for this before querying unrelated keyboard focus.
                if let Some(scope) = point_scope.clone() {
                    if let Some(text) = failures.observe(search_subtree(
                        target,
                        session,
                        &session.raw,
                        scope,
                        SearchOptions::capture(
                            Some(point),
                            gesture,
                            (Instant::now() + Duration::from_millis(300)).min(deadline),
                        ),
                        &mut probe::Budget::new(48, 12),
                    ))? {
                        return Ok(text);
                    }
                }
                if let Some(scope) = point_scope.take() {
                    if let Some(text) = failures.observe(search_subtree(
                        target,
                        session,
                        &session.control,
                        scope,
                        SearchOptions::capture(
                            Some(point),
                            gesture,
                            (Instant::now() + Duration::from_millis(200)).min(deadline),
                        ),
                        &mut probe::Budget::new(32, 10),
                    ))? {
                        return Ok(text);
                    }
                }
            }
        }
        if Instant::now() < deadline {
            if let Ok(element) = session
                .automation
                .GetFocusedElementBuildCache(&session.cache)
            {
                let chain = scoped_chain(
                    target,
                    UiaNode::cached(element),
                    session,
                    &session.control,
                    (Instant::now() + Duration::from_millis(160)).min(deadline),
                );
                if let Some(text) = failures.observe(chain_text(
                    &chain,
                    anchor,
                    gesture,
                    (Instant::now() + Duration::from_millis(140)).min(deadline),
                ))? {
                    return Ok(text);
                }
                if let Some(scope) = subtree_scope(&chain) {
                    if let Some(text) = failures.observe(search_subtree(
                        target,
                        session,
                        &session.control,
                        scope,
                        SearchOptions::capture(anchor, gesture, deadline),
                        &mut probe::Budget::new(32, 10),
                    ))? {
                        return Ok(text);
                    }
                }
            }
        }
        // Raw view includes plugin bridges omitted from Control view. Conversely
        // Control view can reach the Document without hundreds of raw glyphs.
        // Use only the verified foreground HWND as a final scope; never use the
        // desktop automation root. The point scope was already tried in both.
        if Instant::now() < deadline {
            let scope = {
                let point_is_owned = point.is_none() || anchor.is_some();
                point_is_owned
                    .then(|| {
                        session
                            .automation
                            .ElementFromHandleBuildCache(
                                windows::Win32::Foundation::HWND(target.root),
                                &session.cache,
                            )
                            .ok()
                    })
                    .flatten()
                    .map(|element| UiaNode::cached(element))
            };
            if let Some(scope) = scope {
                if let Some(text) = failures.observe(search_subtree(
                    target,
                    session,
                    &session.control,
                    scope,
                    SearchOptions::capture(anchor, gesture, deadline),
                    &mut probe::Budget::new(32, 10),
                ))? {
                    return Ok(text);
                }
            }
        }
        if failures.unverified {
            Err(UiaFailure::Unverified)
        } else if let Some(reason) = failures.unavailable {
            Err(UiaFailure::Unavailable(reason))
        } else {
            Err(UiaFailure::Unavailable(
                "未读取到选中文字；PDF 文字接口可能仍在加载，请重新划选或手动复制后翻译。".into(),
            ))
        }
    })
}

unsafe fn chain_text(
    chain: &[UiaNode],
    anchor: Option<POINT>,
    gesture: Option<probe::Gesture>,
    deadline: Instant,
) -> Result<Option<String>, UiaFailure> {
    let mut verified_leaf = false;
    let mut failures = ProbeFailures::default();
    for node in chain {
        if Instant::now() >= deadline {
            break;
        }
        if probe::priority(node.metadata, anchor.map(|point| (point.x, point.y)), 0, 0).is_none() {
            continue;
        }
        match node.password {
            Some(true) => return Err(UiaFailure::Protected),
            Some(false) => {}
            None if verified_leaf => continue,
            None => return Err(UiaFailure::Unverified),
        }
        match unsafe { node.element.CurrentIsPassword() } {
            Ok(value) if value.as_bool() => return Err(UiaFailure::Protected),
            Ok(_) => verified_leaf = true,
            Err(_) if verified_leaf => continue,
            Err(_) => return Err(UiaFailure::Unverified),
        }
        if !node.may_have_text() {
            continue;
        }
        if let Some(pattern) = unsafe { text_pattern(node, false) } {
            if let Some(text) =
                failures.observe(unsafe { selected_text(&pattern, gesture, deadline) })?
            {
                return Ok(Some(text));
            }
        }
    }
    failures
        .unavailable
        .map_or(Ok(None), |reason| Err(UiaFailure::Unavailable(reason)))
}

unsafe fn scoped_chain(
    target: &Target,
    first: UiaNode,
    session: &UiaSession,
    walker: &IUIAutomationTreeWalker,
    deadline: Instant,
) -> Vec<UiaNode> {
    let initial_pid = first.pid;
    let mut chain = Vec::new();
    let mut scoped = false;
    let mut node = first;
    for _ in 0..MAX_ANCESTORS {
        if Instant::now() >= deadline {
            break;
        }
        let native_scope = if node.native.is_null() {
            probe::NativeScope::None
        } else if target.owns_window(node.native) {
            probe::NativeScope::Foreground
        } else {
            probe::NativeScope::Other
        };
        let step = probe::ancestor_step(native_scope, node.window && node.native == target.root);
        if step == probe::AncestorStep::StopBefore {
            if !scoped {
                return Vec::new();
            }
            break;
        }
        scoped |= native_scope == probe::NativeScope::Foreground;
        chain.push(node.clone());
        // A child HWND proves ownership, but its UIA parent can still be the
        // TextPattern document. Stop at the actual foreground Window element.
        if step == probe::AncestorStep::StopAfter {
            break;
        }
        let Ok(parent) =
            (unsafe { walker.GetParentElementBuildCache(&node.element, &session.cache) })
        else {
            break;
        };
        node = unsafe { UiaNode::cached(parent) };
    }
    if scoped {
        chain
    } else if initial_pid == target.pid as i32 {
        chain
            .into_iter()
            .take_while(|node| node.pid == target.pid as i32)
            .collect()
    } else {
        // A renderer with another PID is accepted only with the HWND ancestry
        // proof above, not just because it is an accessibility descendant.
        Vec::new()
    }
}

fn subtree_scope(chain: &[UiaNode]) -> Option<UiaNode> {
    chain
        .iter()
        .find(|node| node.metadata.document)
        .or_else(|| chain.last())
        .cloned()
}

struct PendingNode {
    node: UiaNode,
    priority: probe::Priority,
    depth: usize,
    unknown_bounds: usize,
}

struct SearchOptions {
    point: Option<POINT>,
    deadline: Instant,
    read_text: bool,
    gesture: Option<probe::Gesture>,
}

impl SearchOptions {
    fn capture(point: Option<POINT>, gesture: Option<probe::Gesture>, deadline: Instant) -> Self {
        Self {
            point,
            deadline,
            read_text: true,
            gesture,
        }
    }
}

unsafe fn search_subtree(
    target: &Target,
    session: &UiaSession,
    walker: &IUIAutomationTreeWalker,
    root: UiaNode,
    options: SearchOptions,
    budget: &mut probe::Budget,
) -> Result<Option<String>, UiaFailure> {
    let SearchOptions {
        point,
        deadline,
        read_text,
        gesture,
    } = options;
    let point = point.map(|point| (point.x, point.y));
    let Some((priority, unknown_bounds)) = probe::priority(root.metadata, point, 0, 0) else {
        return Ok(None);
    };
    let mut pending = vec![PendingNode {
        node: root,
        priority,
        depth: 0,
        unknown_bounds,
    }];
    let mut failures = ProbeFailures::default();
    while !pending.is_empty() && Instant::now() < deadline {
        if unsafe { GetForegroundWindow() } != target.foreground {
            return Ok(None);
        }
        let index = pending
            .iter()
            .enumerate()
            .min_by_key(|(_, node)| node.priority)
            .map(|(index, _)| index)
            .expect("nonempty frontier");
        let current = pending.swap_remove(index);
        let node = current.node;
        if node.password != Some(false)
            || (!node.native.is_null() && !target.owns_window(node.native))
        {
            // An unknown/password/cross-root branch is never queried for text.
            continue;
        }
        let probe_pattern = if read_text {
            node.may_have_text()
        } else {
            node.metadata.document || node.metadata.edit
        };
        if probe_pattern && budget.pattern() {
            if read_text
                && !matches!(unsafe { node.element.CurrentIsPassword() }, Ok(value) if !value.as_bool())
            {
                continue;
            }
            if let Some(pattern) = unsafe { text_pattern(&node, !read_text) } {
                if read_text {
                    if let Some(text) =
                        failures.observe(unsafe { selected_text(&pattern, gesture, deadline) })?
                    {
                        return Ok(Some(text));
                    }
                }
            }
        }
        if current.depth >= probe::MAX_DEPTH || !budget.has_nodes() || Instant::now() >= deadline {
            continue;
        }
        let mut child =
            unsafe { walker.GetFirstChildElementBuildCache(&node.element, &session.cache) }.ok();
        for _ in 0..probe::MAX_SIBLINGS {
            if Instant::now() >= deadline || !budget.scan() {
                break;
            }
            let Some(element) = child else { break };
            let child_node = unsafe { UiaNode::cached(element) };
            if let Some((priority, unknown_bounds)) = probe::priority(
                child_node.metadata,
                point,
                current.depth + 1,
                current.unknown_bounds,
            ) {
                if budget.node() {
                    pending.push(PendingNode {
                        node: child_node.clone(),
                        priority,
                        depth: current.depth + 1,
                        unknown_bounds,
                    });
                }
            }
            if Instant::now() >= deadline || !budget.has_nodes() {
                break;
            }
            child = unsafe {
                walker.GetNextSiblingElementBuildCache(&child_node.element, &session.cache)
            }
            .ok();
        }
    }
    failures
        .unavailable
        .map_or(Ok(None), |reason| Err(UiaFailure::Unavailable(reason)))
}

unsafe fn text_pattern(node: &UiaNode, warming: bool) -> Option<IUIAutomationTextPattern> {
    unsafe {
        // TextPattern2 inherits the same selection contract. Accept the newer
        // pattern ID too; never substitute caret/whole-document extraction or
        // LegacyIAccessible.Value for an unavailable selection.
        (node.text_available != Some(false) || node.text_candidate || warming)
            .then(|| {
                node.element
                    .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                    .ok()
            })
            .flatten()
            .or_else(|| {
                (node.text2_available != Some(false))
                    .then(|| {
                        node.element
                            .GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
                            .ok()
                            .map(|pattern| IUIAutomationTextPattern::clone(&pattern))
                    })
                    .flatten()
            })
    }
}

unsafe fn selected_text(
    pattern: &IUIAutomationTextPattern,
    gesture: Option<probe::Gesture>,
    deadline: Instant,
) -> Result<Option<String>, UiaFailure> {
    if Instant::now() >= deadline {
        return Ok(None);
    }
    // Unsupported selection must not be interpreted as an entire document or
    // as an instruction to read LegacyIAccessible.Value/Name.
    if unsafe { pattern.SupportedTextSelection() }.ok() == Some(SupportedTextSelection_None) {
        return Ok(None);
    }
    if Instant::now() >= deadline {
        return Ok(None);
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
    let mut selected_ranges = Vec::new();
    let mut geometry_matches = false;
    let mut geometry_unavailable = false;
    for index in 0..count.max(0) {
        if Instant::now() >= deadline {
            return Ok(None);
        }
        let range = unsafe { ranges.GetElement(index) }
            .map_err(|_| UiaFailure::Unavailable("选区已失效，请重新选择文字。".into()))?;
        if let Some(gesture) = gesture {
            match unsafe { range_matches_gesture(&range, gesture) } {
                Some(matches) => geometry_matches |= matches,
                None => geometry_unavailable = true,
            }
        }
        selected_ranges.push(range);
    }
    if gesture.is_some() && !geometry_matches && !geometry_unavailable {
        return Ok(None);
    }
    for range in selected_ranges {
        if Instant::now() >= deadline {
            return Ok(None);
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
    if !has_visible_text(&text) {
        // An explicitly selected run of spaces/formatting is not a provider
        // failure or a request to translate an ancestor's document contents.
        return if units == 0 {
            Ok(None)
        } else {
            Err(UiaFailure::Unavailable(no_selection()))
        };
    }
    Ok(Some(text))
}

unsafe fn range_matches_gesture(
    range: &IUIAutomationTextRange,
    gesture: probe::Gesture,
) -> Option<bool> {
    // The caller owns the returned SAFEARRAY. Validate its shape and element
    // width before creating any Rust slice, and bound multi-line rectangles.
    let array = unsafe { range.GetBoundingRectangles() }.ok()?;
    if array.is_null() {
        return None;
    }
    struct OwnedArray(*mut windows::Win32::System::Com::SAFEARRAY);
    impl Drop for OwnedArray {
        fn drop(&mut self) {
            unsafe {
                let _ = SafeArrayDestroy(self.0);
            }
        }
    }
    let array = OwnedArray(array);
    unsafe {
        if SafeArrayGetDim(array.0) != 1 || SafeArrayGetElemsize(array.0) != size_of::<f64>() as u32
        {
            return None;
        }
        let lower = SafeArrayGetLBound(array.0, 1).ok()?;
        let upper = SafeArrayGetUBound(array.0, 1).ok()?;
        let count = i64::from(upper) - i64::from(lower) + 1;
        if !(4..=4096).contains(&count) || count % 4 != 0 {
            return None;
        }
        let mut data = ptr::null_mut();
        SafeArrayAccessData(array.0, &mut data).ok()?;
        let result = if data.is_null() {
            None
        } else {
            gesture.intersects_rectangles(std::slice::from_raw_parts(
                data.cast::<f64>(),
                count as usize,
            ))
        };
        let _ = SafeArrayUnaccessData(array.0);
        result
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
    if !has_visible_text(&text) {
        return Err("剪贴板中没有可翻译的文字。".into());
    }
    Ok(text)
}

unsafe fn read_unicode_text() -> Result<String, String> {
    decode_unicode(&unsafe { clipboard_bytes()? })
}

fn capture_by_copy(foreground: HWND) -> Result<CapturedText, String> {
    super::clipboard_copy::capture(
        foreground as isize,
        unsafe { GetClipboardSequenceNumber() },
        || unsafe { GetForegroundWindow() } != foreground,
    )
    .and_then(|text| {
        if has_visible_text(&text) {
            Ok(CapturedText {
                text,
                source: "模拟复制",
            })
        } else {
            Err(no_selection())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        choose_source_point, decode_unicode, focused_native_fallback_allowed,
        is_no_selection_error, is_protected_error, native_edit_selection, no_selection,
        protected_message, NativeSelection, Target, UiaFailure, MAX_SELECTION_UNITS,
    };
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{HWND, POINT},
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
    fn popup_covered_release_uses_source_origin_and_never_foreign_points() {
        let origin = POINT { x: 100, y: 250 };
        let end = POINT { x: 300, y: 280 };
        let point = choose_source_point(Some(end), Some(origin), |point| point.x == 100).unwrap();
        assert_eq!((point.x, point.y), (100, 250));
        let point = choose_source_point(Some(end), Some(origin), |_| true).unwrap();
        assert_eq!((point.x, point.y), (300, 280));
        assert!(choose_source_point(Some(end), Some(origin), |_| false).is_none());
        assert!(choose_source_point(Some(end), None, |_| false).is_none());
    }

    #[test]
    fn no_selection_and_protected_classification_is_exact() {
        assert!(is_no_selection_error(&no_selection()));
        assert!(!is_no_selection_error("UIA PDF provider is still loading"));
        assert!(is_protected_error(&protected_message()));
        assert!(!is_protected_error(&format!(
            "{} other provider details",
            protected_message()
        )));
        assert!(!is_protected_error(&no_selection()));
    }

    #[test]
    fn automatic_native_fallback_rejects_same_root_toolbar_selection_without_a_gesture_hit() {
        let (root, toolbar, target) = edit("old toolbar text", false);
        select(toolbar, 0, 3);
        // An ancestor/sibling hit proves only the browser window, not which
        // control the user's current PDF gesture selected.
        assert!(!focused_native_fallback_allowed(
            &target,
            toolbar,
            true,
            [root.0]
        ));
        assert!(focused_native_fallback_allowed(
            &target,
            toolbar,
            true,
            [toolbar]
        ));
        assert!(focused_native_fallback_allowed(&target, toolbar, false, []));

        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let nested = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                WS_CHILD,
                0,
                0,
                10,
                10,
                toolbar,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        assert!(!nested.is_null());
        assert!(focused_native_fallback_allowed(
            &target,
            toolbar,
            true,
            [nested]
        ));
        let (_popup, foreign, _) = edit("translation popup", false);
        // Release over our popup still permits the source origin; a foreign
        // release alone must never revive the focused toolbar's old selection.
        assert!(focused_native_fallback_allowed(
            &target,
            toolbar,
            true,
            [foreign, toolbar]
        ));
        assert!(!focused_native_fallback_allowed(
            &target,
            toolbar,
            true,
            [foreign]
        ));
        assert!(!focused_native_fallback_allowed(
            &target,
            foreign,
            false,
            []
        ));
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
    fn native_edit_filters_blank_runs_but_preserves_joiners_inside_visible_text() {
        for selected in [" \t\r\n\u{00a0}", "\u{200b}\u{200d}\u{feff}"] {
            let (_root, window, target) = edit(selected, false);
            select(window, 0, selected.encode_utf16().count());
            assert!(matches!(
                native_edit_selection(&target, window),
                Ok(NativeSelection::Empty)
            ));
        }
        let selected = "👩\u{200d}💻 می\u{200c}روم 0 .";
        let (_root, window, target) = edit(selected, false);
        select(window, 0, selected.encode_utf16().count());
        match native_edit_selection(&target, window).unwrap() {
            NativeSelection::Text(actual) => assert_eq!(actual, selected),
            _ => panic!("visible text and its joiners were not preserved"),
        }
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

    #[test]
    fn clipboard_decode_rejects_invisible_only_text() {
        let bytes: Vec<u8> = " \u{200b}\u{200c}\u{feff}\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(decode_unicode(&bytes).is_err());
    }
}
