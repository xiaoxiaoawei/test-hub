//! A native, read-only Markdown reader. RichEdit supplies text shaping,
//! selection, scrolling and accessibility; no browser or external content runs.
use crate::{
    config::{ThemeMode, ThemePreset},
    markdown::{self, RenderStyle},
    visual::Palette,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{RedrawWindow, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE},
    System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32},
    UI::{
        Controls::{
            EM_EMPTYUNDOBUFFER, EM_GETFIRSTVISIBLELINE, EM_GETRECT, EM_LINESCROLL, EM_SCROLLCARET,
            EM_SETRECTNP,
        },
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

pub(crate) const CLASS: &str = "RICHEDIT50W";
const SUBCLASS: usize = 0x4c4c_0090;
const MAX_TEXT: usize = 2 * 1024 * 1024;
const EM_EXGETSEL: u32 = WM_USER + 52;
const EM_EXLIMITTEXT: u32 = WM_USER + 53;
const EM_EXSETSEL: u32 = WM_USER + 55;
const EM_SETBKGNDCOLOR: u32 = WM_USER + 67;
const EM_SETCHARFORMAT: u32 = WM_USER + 68;
const EM_STREAMIN: u32 = WM_USER + 73;
const EM_SETUNDOLIMIT: u32 = WM_USER + 82;
const EM_AUTOURLDETECT: u32 = WM_USER + 91;
const EM_GETTEXTEX: u32 = WM_USER + 94;
const EM_GETTEXTLENGTHEX: u32 = WM_USER + 95;
const EM_GETSCROLLPOS: u32 = WM_USER + 221;
const EM_SETSCROLLPOS: u32 = WM_USER + 222;
const SF_RTF: usize = 2;
const SCF_ALL: usize = 4;

#[repr(C)]
#[derive(Default)]
struct CharRange {
    start: i32,
    end: i32,
}

// The SDK uses pack(4) for EDITSTREAM on 64-bit Windows. Its callback is at
// offset 12, not the offset 16 produced by an ordinary repr(C) structure.
#[repr(C, packed(4))]
struct EditStream {
    cookie: usize,
    error: u32,
    callback: Option<unsafe extern "system" fn(usize, *mut u8, i32, *mut i32) -> u32>,
}

#[repr(C)]
struct CharFormat {
    size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    color: u32,
    charset: u8,
    pitch: u8,
    face: [u16; 32],
}

#[repr(C)]
struct TextLength {
    flags: u32,
    codepage: u32,
}

#[repr(C, packed(4))]
struct GetText {
    bytes: u32,
    flags: u32,
    codepage: u32,
    default_character: *const u8,
    used_default: *mut i32,
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

#[derive(Clone)]
struct View {
    source: Option<String>,
    text: String,
    mode: ThemeMode,
    preset: ThemePreset,
    width: i32,
    dpi: u32,
}

thread_local! {
    // Keep Msftedit loaded while USER32 can call its window procedures.
    static LIBRARY: Cell<isize> = const { Cell::new(0) };
    static VIEWS: RefCell<HashMap<usize, View>> = RefCell::new(HashMap::new());
    static RENDERING: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

pub(crate) fn load() -> Result<(), String> {
    if LIBRARY.get() != 0 {
        return Ok(());
    }
    let name: Vec<u16> = "Msftedit.dll\0".encode_utf16().collect();
    let library =
        unsafe { LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if library.is_null() {
        return Err("无法载入 Windows 富文本组件 Msftedit.dll。".into());
    }
    LIBRARY.set(library as isize);
    Ok(())
}

pub(crate) unsafe fn is_rich(editor: HWND) -> bool {
    let mut class = [0u16; 32];
    let count = GetClassNameW(editor, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
    String::from_utf16_lossy(&class[..count]).eq_ignore_ascii_case(CLASS)
}

pub(crate) unsafe fn install(editor: HWND, mode: ThemeMode, preset: ThemePreset) {
    if editor.is_null() || !is_rich(editor) {
        return;
    }
    VIEWS.with(|views| {
        views.borrow_mut().entry(editor as usize).or_insert(View {
            source: None,
            text: String::new(),
            mode,
            preset,
            width: 0,
            dpi: 0,
        });
    });
    SetWindowSubclass(editor, Some(view_proc), SUBCLASS, 0);
    SendMessageW(editor, EM_EXLIMITTEXT, 0, MAX_TEXT as isize);
    SendMessageW(editor, EM_AUTOURLDETECT, 0, 0);
    SendMessageW(editor, EM_SETUNDOLIMIT, 0, 0);
    apply_padding(editor);
    set_background(editor, mode, preset);
    plain_format(editor, mode, preset, false);
}

/// Render Markdown on the UI thread. The synchronous stream callback borrows
/// its bytes only for the duration of EM_STREAMIN.
pub(crate) unsafe fn render(
    editor: HWND,
    _previous: &str,
    current: &str,
    fresh: bool,
    mode: ThemeMode,
    preset: ThemePreset,
) {
    render_inner(editor, current, fresh, mode, preset, false);
}

pub(crate) unsafe fn restyle(editor: HWND, mode: ThemeMode, preset: ThemePreset) {
    let source = VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        let view = views.get_mut(&(editor as usize))?;
        view.mode = mode;
        view.preset = preset;
        Some(view.source.clone())
    });
    set_background(editor, mode, preset);
    if let Some(Some(source)) = source {
        render_inner(editor, &source, false, mode, preset, true);
    } else {
        apply_padding(editor);
        plain_format(editor, mode, preset, true);
    }
}

pub(crate) unsafe fn cleanup(editor: HWND) {
    VIEWS.with(|views| {
        views.borrow_mut().remove(&(editor as usize));
    });
    RENDERING.with(|views| {
        views.borrow_mut().remove(&(editor as usize));
    });
    RemoveWindowSubclass(editor, Some(view_proc), SUBCLASS);
}

unsafe fn render_inner(
    editor: HWND,
    current: &str,
    fresh: bool,
    mode: ThemeMode,
    preset: ThemePreset,
    force: bool,
) {
    if editor.is_null() || RENDERING.with(|set| set.borrow().contains(&(editor as usize))) {
        return;
    }
    let Some(content) = apply_padding(editor) else {
        return;
    };
    let mut bounds: RECT = zeroed();
    if GetClientRect(editor, &mut bounds) == 0 {
        return;
    }
    let width = (bounds.right - bounds.left).max(1);
    let dpi = GetDpiForWindow(editor).max(96);
    let current = normalize(current);
    let cached = VIEWS.with(|views| views.borrow().get(&(editor as usize)).cloned());
    if !force
        && cached.as_ref().is_some_and(|view| {
            view.source.as_deref() == Some(current.as_str())
                && view.mode == mode
                && view.preset == preset
                && view.width == width
                && view.dpi == dpi
        })
    {
        return;
    }
    let colors = Palette::for_theme(mode, preset);
    let available = (i64::from((content.right - content.left).max(1)) * 1440 / i64::from(dpi))
        .clamp(720, 28_800);
    let style = RenderStyle {
        text: colors.text,
        background: colors.surface,
        muted: colors.muted,
        accent: colors.accent,
        code_background: colors.raised,
        body_font: "Segoe UI",
        mono_font: "Consolas",
        body_size_half_points: 24,
        width_twips: available as i32,
    };
    let rendered = markdown::render(&current, &style);
    let old_text = cached
        .as_ref()
        .filter(|view| view.source.is_some())
        .map(|view| view.text.clone())
        .unwrap_or_else(|| document_text(editor));
    let mut selection = CharRange::default();
    SendMessageW(editor, EM_EXGETSEL, 0, &mut selection as *mut _ as isize);
    let mut scroll: SCROLLINFO = zeroed();
    scroll.cbSize = size_of::<SCROLLINFO>() as u32;
    scroll.fMask = SIF_RANGE | SIF_PAGE | SIF_POS;
    let at_bottom = GetScrollInfo(editor, SB_VERT, &mut scroll) == 0
        || i64::from(scroll.nPos) + i64::from(scroll.nPage) >= i64::from(scroll.nMax);
    let follow = fresh || (selection.start == selection.end && at_bottom);
    let mut point = POINT { x: 0, y: 0 };
    let pixel_scroll = SendMessageW(editor, EM_GETSCROLLPOS, 0, &mut point as *mut _ as isize) != 0;
    let first_line = SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0);

    let _rendering = Rendering::begin(editor);
    // Insert before native calls can synchronously ask this subclass to reflow.
    VIEWS.with(|views| {
        views.borrow_mut().insert(
            editor as usize,
            View {
                source: Some(current),
                text: String::new(),
                mode,
                preset,
                width,
                dpi,
            },
        );
    });
    let _redraw = Redraw::pause(editor);
    set_background(editor, mode, preset);
    if !stream_rtf(editor, &rendered.rtf) {
        // A component parse failure must still leave readable text, never raw
        // RTF or an empty output. No unsupported external RTF enters this path.
        let plain: Vec<u16> = rendered.text.encode_utf16().chain(Some(0)).collect();
        SetWindowTextW(editor, plain.as_ptr());
        plain_format(editor, mode, preset, true);
    }
    // Some RichEdit versions reset the formatting rectangle while importing
    // document margins. Restore the native content inset before scroll/selection.
    apply_padding(editor);
    SendMessageW(editor, EM_EMPTYUNDOBUFFER, 0, 0);
    let new_text = document_text(editor);
    if follow {
        let end = CharRange { start: -1, end: -1 };
        SendMessageW(editor, EM_EXSETSEL, 0, &end as *const _ as isize);
        SendMessageW(editor, EM_SCROLLCARET, 0, 0);
    } else {
        let mut selection = mapped_selection(&old_text, &new_text, &selection);
        SendMessageW(editor, EM_EXSETSEL, 0, &mut selection as *mut _ as isize);
        if pixel_scroll {
            SendMessageW(editor, EM_SETSCROLLPOS, 0, &point as *const _ as isize);
        } else {
            let now = SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0);
            SendMessageW(editor, EM_LINESCROLL, 0, first_line - now);
        }
    }
    VIEWS.with(|views| {
        if let Some(view) = views.borrow_mut().get_mut(&(editor as usize)) {
            view.text = new_text;
        }
    });
}

unsafe fn stream_rtf(editor: HWND, bytes: &[u8]) -> bool {
    let mut reader = Reader { bytes, offset: 0 };
    let mut stream = EditStream {
        cookie: &mut reader as *mut Reader<'_> as usize,
        error: 0,
        callback: Some(read_stream),
    };
    SendMessageW(editor, EM_STREAMIN, SF_RTF, &mut stream as *mut _ as isize);
    stream.error == 0 && reader.offset == bytes.len()
}

unsafe extern "system" fn read_stream(
    cookie: usize,
    buffer: *mut u8,
    capacity: i32,
    read: *mut i32,
) -> u32 {
    if read.is_null() || cookie == 0 || capacity < 0 || (capacity != 0 && buffer.is_null()) {
        return 1;
    }
    *read = 0;
    let reader = &mut *(cookie as *mut Reader<'_>);
    let count = (reader.bytes.len() - reader.offset).min(capacity as usize);
    if count != 0 {
        std::ptr::copy_nonoverlapping(reader.bytes.as_ptr().add(reader.offset), buffer, count);
        reader.offset += count;
    }
    *read = count as i32;
    0
}

unsafe fn document_text(editor: HWND) -> String {
    let length_request = TextLength {
        flags: 2 | 8,
        codepage: 1200,
    };
    let length = SendMessageW(
        editor,
        EM_GETTEXTLENGTHEX,
        &length_request as *const _ as usize,
        0,
    );
    if !(0..=MAX_TEXT as isize).contains(&length) {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let request = GetText {
        bytes: (buffer.len() * size_of::<u16>()) as u32,
        flags: 0,
        codepage: 1200,
        default_character: null(),
        used_default: null_mut(),
    };
    let read = SendMessageW(
        editor,
        EM_GETTEXTEX,
        &request as *const _ as usize,
        buffer.as_mut_ptr() as isize,
    );
    let read = (read.max(0) as usize).min(buffer.len() - 1);
    normalize(&String::from_utf16_lossy(&buffer[..read]))
}

unsafe fn set_background(editor: HWND, mode: ThemeMode, preset: ThemePreset) {
    let colors = Palette::for_theme(mode, preset);
    SendMessageW(editor, EM_SETBKGNDCOLOR, 0, rgb(colors.surface) as isize);
}

/// Keep the real editor's text rectangle inset from its rounded shell. Read the
/// native rectangle first so stream updates do not repeatedly force a reflow.
/// A temporary rendering guard prevents a scrollbar/layout notification from
/// synchronously reentering Markdown rendering; no RefCell borrow crosses it.
unsafe fn apply_padding(editor: HWND) -> Option<RECT> {
    let mut client: RECT = zeroed();
    if GetClientRect(editor, &mut client) == 0 {
        return None;
    }
    let dpi = GetDpiForWindow(editor).max(96);
    let width = (client.right - client.left).max(1);
    let height = (client.bottom - client.top).max(1);
    let horizontal = ((12 * i64::from(dpi) + 48) / 96) as i32;
    let vertical = ((8 * i64::from(dpi) + 48) / 96) as i32;
    let horizontal = horizontal.min((width - 1) / 2);
    let vertical = vertical.min((height - 1) / 2);
    let content = RECT {
        left: client.left + horizontal,
        top: client.top + vertical,
        right: client.right - horizontal,
        bottom: client.bottom - vertical,
    };
    let mut native: RECT = zeroed();
    SendMessageW(editor, EM_GETRECT, 0, &mut native as *mut _ as isize);
    if (native.left, native.top, native.right, native.bottom)
        != (content.left, content.top, content.right, content.bottom)
    {
        let rendering = RENDERING.with(|set| set.borrow().contains(&(editor as usize)));
        let _rendering = (!rendering).then(|| Rendering::begin(editor));
        SendMessageW(editor, EM_SETRECTNP, 0, &content as *const _ as isize);
    }
    Some(content)
}

unsafe fn plain_format(editor: HWND, mode: ThemeMode, preset: ThemePreset, all: bool) {
    let colors = Palette::for_theme(mode, preset);
    let mut face = [0u16; 32];
    for (target, value) in face.iter_mut().zip("Segoe UI".encode_utf16()) {
        *target = value;
    }
    let format = CharFormat {
        size: size_of::<CharFormat>() as u32,
        mask: 0x4000_0000 | 0x8000_0000 | 0x2000_0000 | 1 | 2 | 4 | 8 | 32 | 256,
        effects: 0,
        height: 240,
        offset: 0,
        color: rgb(colors.text),
        charset: 1,
        pitch: 0,
        face,
    };
    SendMessageW(
        editor,
        EM_SETCHARFORMAT,
        if all { SCF_ALL } else { 0 },
        &format as *const _ as isize,
    );
}

unsafe extern "system" fn view_proc(
    editor: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        cleanup(editor);
        return DefSubclassProc(editor, message, wparam, lparam);
    }
    let rendering = RENDERING.with(|set| set.borrow().contains(&(editor as usize)));
    if message == WM_SETTEXT && !rendering {
        VIEWS.with(|views| {
            if let Some(view) = views.borrow_mut().get_mut(&(editor as usize)) {
                view.source = None;
                view.text.clear();
            }
        });
    }
    let result = DefSubclassProc(editor, message, wparam, lparam);
    if !rendering {
        let state = VIEWS.with(|views| views.borrow().get(&(editor as usize)).cloned());
        if let Some(view) = state {
            if message == WM_SETTEXT {
                apply_padding(editor);
                plain_format(editor, view.mode, view.preset, true);
            } else if matches!(message, WM_SIZE | WM_DPICHANGED_AFTERPARENT | WM_SETFONT) {
                if let Some(source) = view.source {
                    render_inner(
                        editor,
                        &source,
                        false,
                        view.mode,
                        view.preset,
                        message != WM_SIZE,
                    );
                } else {
                    apply_padding(editor);
                }
            }
        }
    }
    // This subclass does not handle WM_CONTEXTMENU: the application's existing
    // translated-output subclass and the native keyboard chain retain it.
    result
}

struct Rendering(HWND);

impl Rendering {
    fn begin(editor: HWND) -> Self {
        RENDERING.with(|set| {
            set.borrow_mut().insert(editor as usize);
        });
        Self(editor)
    }
}

impl Drop for Rendering {
    fn drop(&mut self) {
        RENDERING.with(|set| {
            set.borrow_mut().remove(&(self.0 as usize));
        });
    }
}

struct Redraw(HWND, bool);

impl Redraw {
    unsafe fn pause(editor: HWND) -> Self {
        let visible = GetWindowLongPtrW(editor, GWL_STYLE) & WS_VISIBLE as isize != 0;
        if visible {
            SendMessageW(editor, WM_SETREDRAW, 0, 0);
        }
        Self(editor, visible)
    }
}

impl Drop for Redraw {
    fn drop(&mut self) {
        unsafe {
            if self.1 {
                SendMessageW(self.0, WM_SETREDRAW, 1, 0);
            }
            RedrawWindow(
                self.0,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME,
            );
        }
    }
}

fn normalize(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

const fn rgb(value: u32) -> u32 {
    ((value >> 16) & 255) | (value & 0xff00) | ((value & 255) << 16)
}

fn mapped_selection(previous: &str, current: &str, range: &CharRange) -> CharRange {
    let previous: Vec<u16> = previous.encode_utf16().collect();
    let current: Vec<u16> = current.encode_utf16().collect();
    let prefix = previous
        .iter()
        .zip(&current)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = previous[prefix..]
        .iter()
        .rev()
        .zip(current[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let map = |index: i32| {
        let index = (index.max(0) as usize).min(previous.len());
        let mapped = if index <= prefix {
            index
        } else if index >= previous.len() - suffix {
            current.len() - (previous.len() - index)
        } else {
            prefix + (index - prefix).min(current.len() - prefix - suffix)
        };
        // A closing delimiter may move a selection across an emoji. Never leave
        // an endpoint in the middle of its UTF-16 surrogate pair.
        let mapped = mapped.min(current.len());
        let mapped = if mapped > 0
            && mapped < current.len()
            && (0xdc00..=0xdfff).contains(&current[mapped])
            && (0xd800..=0xdbff).contains(&current[mapped - 1])
        {
            mapped - 1
        } else {
            mapped
        };
        mapped.min(i32::MAX as usize) as i32
    };
    CharRange {
        start: map(range.start),
        end: map(range.end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_markup_moves_selected_unicode_text_without_splitting_surrogates() {
        let selected = CharRange { start: 2, end: 7 };
        let moved = mapped_selection("**中😀文", "中😀文", &selected);
        assert_eq!((moved.start, moved.end), (0, 4));
        let expanded = mapped_selection("中😀文", "前缀中😀文", &CharRange { start: 1, end: 3 });
        assert_eq!((expanded.start, expanded.end), (3, 5));
    }

    #[test]
    fn appended_stream_keeps_selection_and_changed_region_clamps_it() {
        let unchanged = mapped_selection(
            "select this",
            "select this and more",
            &CharRange { start: 0, end: 6 },
        );
        assert_eq!((unchanged.start, unchanged.end), (0, 6));
        let clamped = mapped_selection("old long answer", "new", &CharRange { start: 4, end: 15 });
        assert_eq!((clamped.start, clamped.end), (3, 3));
        let empty = mapped_selection("old", "", &CharRange { start: 0, end: 3 });
        assert_eq!((empty.start, empty.end), (0, 0));
    }

    #[test]
    fn stream_callback_is_bounded_and_completes_at_eof() {
        let mut reader = Reader {
            bytes: b"abcdef",
            offset: 0,
        };
        let mut buffer = [0u8; 4];
        let mut read = 0;
        unsafe {
            let cookie = &mut reader as *mut Reader<'_> as usize;
            assert_eq!(read_stream(cookie, buffer.as_mut_ptr(), 4, &mut read), 0);
            assert_eq!((read, buffer), (4, *b"abcd"));
            assert_eq!(read_stream(cookie, buffer.as_mut_ptr(), 4, &mut read), 0);
            assert_eq!((read, &buffer[..2]), (2, &b"ef"[..]));
            assert_eq!(read_stream(cookie, buffer.as_mut_ptr(), 4, &mut read), 0);
            assert_eq!(read, 0);
            assert_eq!(read_stream(cookie, null_mut(), 1, &mut read), 1);
        }
    }

    #[test]
    fn native_layout_matches_the_64_bit_richedit_abi() {
        assert_eq!(size_of::<EditStream>(), 20);
        assert_eq!(size_of::<CharRange>(), 8);
        assert_eq!(size_of::<CharFormat>(), 92);
    }

    #[test]
    fn unicode_gettextex_matches_the_sdk_pack4_layout() {
        assert_eq!(size_of::<GetText>(), 28);
        assert_eq!(std::mem::offset_of!(GetText, default_character), 12);
        assert_eq!(std::mem::offset_of!(GetText, used_default), 20);
    }
}
