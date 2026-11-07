//! Rounded chrome around real EDIT and COMBOBOX controls. Native text editing,
//! selection, keyboard navigation and accessibility stay with the system class.
use super::theme::Theme;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::*,
    UI::{
        Controls::*,
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{
            GetFocus, IsWindowEnabled, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
        },
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const SUBCLASS: usize = 0x4c4c_0080;
const EDIT: usize = 1;
const COMBO: usize = 2;
const LIST: usize = 3;
const CARD: &[u16] = &[76, 76, 46, 67, 97, 114, 100, 0];
const HOT: &[u16] = &[76, 76, 46, 72, 111, 116, 0];
const ROUND: &[u16] = &[76, 76, 46, 82, 111, 117, 110, 100, 0];

#[derive(Clone, Copy)]
struct Colors {
    surface: COLORREF,
    border: COLORREF,
    accent: COLORREF,
    text: COLORREF,
    raised: COLORREF,
}

type Geometry = (i32, i32, i32, u32);
type Layout = (i32, i32, u32, usize);

thread_local! {
    static REGIONS: RefCell<HashMap<usize, Geometry>> = RefCell::new(HashMap::new());
    static LAYOUTS: RefCell<HashMap<usize, Layout>> = RefCell::new(HashMap::new());
    static WINDOWS: RefCell<HashMap<usize, (i32, i32, u32, bool)>> = RefCell::new(HashMap::new());
    static COLORS: Cell<Colors> = const { Cell::new(Colors { surface: 0x252930, border: 0x3e4959, accent: 0x75a5d6, text: 0xd8e5f1, raised: 0x2c3440 }) };
}

/// Commit the control chrome together with the owning Theme. It stores colors,
/// never brush handles that could outlive a replaced palette.
pub(crate) fn set_palette(theme: &Theme) {
    COLORS.set(Colors {
        surface: theme.surface_color,
        border: theme.border_color,
        accent: theme.accent_color,
        text: theme.text_color,
        raised: theme.raised_color,
    });
}

pub(crate) unsafe fn set_card_control(window: HWND, card: bool) {
    if card {
        SetPropW(window, CARD.as_ptr(), std::ptr::without_provenance_mut(1));
    } else {
        RemovePropW(window, CARD.as_ptr());
    }
}

pub(crate) unsafe fn is_card_control(window: HWND) -> bool {
    !GetPropW(window, CARD.as_ptr()).is_null()
}

pub(crate) unsafe fn install_edit(window: HWND) {
    if window.is_null() {
        return;
    }
    SetWindowSubclass(window, Some(control_proc), SUBCLASS, EDIT);
    remove_native_edge(window);
    layout_control(window);
}

pub(crate) unsafe fn install_combo(window: HWND) {
    if window.is_null() {
        return;
    }
    SetWindowSubclass(window, Some(control_proc), SUBCLASS, COMBO);
    // A full native combo still handles clicks, F4, Alt+Down, arrows and MSAA.
    // Disable only its system visual skin; the selection item remains owner drawn.
    let empty = [0u16];
    SetWindowTheme(window, empty.as_ptr(), empty.as_ptr());
    remove_native_edge(window);
    layout_control(window);
}

unsafe fn remove_native_edge(window: HWND) {
    let style = GetWindowLongPtrW(window, GWL_STYLE);
    SetWindowLongPtrW(window, GWL_STYLE, style & !(WS_BORDER as isize));
    let extended = GetWindowLongPtrW(window, GWL_EXSTYLE);
    SetWindowLongPtrW(window, GWL_EXSTYLE, extended & !(WS_EX_CLIENTEDGE as isize));
}

/// Call after moving/font changes. No timers or animation are involved.
pub(crate) unsafe fn layout_control(window: HWND) {
    if window.is_null() {
        return;
    }
    let mut installed_kind = 0usize;
    if GetWindowSubclass(window, Some(control_proc), SUBCLASS, &mut installed_kind) == 0 {
        return;
    }
    let mut bounds: RECT = zeroed();
    if GetWindowRect(window, &mut bounds) == 0 {
        return;
    }
    let key = (
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
        GetDpiForWindow(window),
        SendMessageW(window, WM_GETFONT, 0, 0) as usize,
    );
    let unchanged =
        LAYOUTS.with(|cache| cache.borrow().get(&(window as usize)).copied() == Some(key));
    if unchanged {
        return;
    }
    // Commit the cache before native frame changes can reenter layout.
    LAYOUTS.with(|cache| {
        cache.borrow_mut().insert(window as usize, key);
    });
    let mut class = [0u16; 24];
    let len = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
    let combo = class[..len]
        .iter()
        .map(|value| *value as u8)
        .eq(b"ComboBox".iter().copied())
        || class[..len]
            .iter()
            .map(|value| *value as u8)
            .eq(b"COMBOBOX".iter().copied());
    if combo {
        SendMessageW(
            window,
            CB_SETITEMHEIGHT,
            usize::MAX,
            scaled(window, 34) as isize,
        );
        SendMessageW(window, CB_SETITEMHEIGHT, 0, scaled(window, 32) as isize);
        let mut info: COMBOBOXINFO = zeroed();
        info.cbSize = size_of::<COMBOBOXINFO>() as u32;
        if GetComboBoxInfo(window, &mut info) != 0 && !info.hwndList.is_null() {
            SetWindowSubclass(info.hwndList, Some(control_proc), SUBCLASS, LIST);
        }
    }
    SetWindowPos(
        window,
        null_mut(),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
    rounded_region(window, scaled(window, 11));
    if GetWindowRect(window, &mut bounds) != 0 {
        let actual = (
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            GetDpiForWindow(window),
            SendMessageW(window, WM_GETFONT, 0, 0) as usize,
        );
        LAYOUTS.with(|cache| {
            cache.borrow_mut().insert(window as usize, actual);
        });
    }
    RedrawWindow(window, null(), null_mut(), RDW_INVALIDATE | RDW_FRAME);
}

/// Native DWM rounding is preferred for decorated Windows 11 windows. A region
/// is the fallback for popups and systems without the documented corner hint.
pub(crate) unsafe fn round_window(window: HWND) {
    if window.is_null() {
        return;
    }
    let mut bounds: RECT = zeroed();
    if GetWindowRect(window, &mut bounds) == 0 {
        return;
    }
    let zoomed = IsZoomed(window) != 0;
    let key = (
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
        GetDpiForWindow(window),
        zoomed,
    );
    let unchanged =
        WINDOWS.with(|cache| cache.borrow().get(&(window as usize)).copied() == Some(key));
    if unchanged {
        return;
    }
    WINDOWS.with(|cache| {
        cache.borrow_mut().insert(window as usize, key);
    });
    if zoomed {
        REGIONS.with(|cache| {
            cache.borrow_mut().remove(&(window as usize));
        });
        SetWindowRgn(window, null_mut(), 0);
        return;
    }
    let popup = GetWindowLongPtrW(window, GWL_STYLE) & WS_POPUP as isize != 0;
    let preference = windows_sys::Win32::Graphics::Dwm::DWMWCP_ROUND;
    let result = windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
        window,
        windows_sys::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        (&preference as *const i32).cast(),
        size_of::<i32>() as u32,
    );
    let mut actual = windows_sys::Win32::Graphics::Dwm::DWMWCP_DEFAULT;
    let readable = windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
        window,
        windows_sys::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        (&mut actual as *mut i32).cast(),
        size_of::<i32>() as u32,
    );
    // Do not mistake a successful but unimplemented compatibility-layer hint
    // for a real rounded frame. Native Windows 11 reports the applied preference.
    if popup
        || result < 0
        || readable < 0
        || actual != preference
        || !GetPropW(window, ROUND.as_ptr()).is_null()
    {
        rounded_region(window, scaled(window, if popup { 18 } else { 14 }));
        SetPropW(window, ROUND.as_ptr(), std::ptr::without_provenance_mut(1));
    }
}

unsafe fn rounded_region(window: HWND, radius: i32) {
    let mut rect: RECT = zeroed();
    if GetWindowRect(window, &mut rect) == 0 {
        return;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 1 || height <= 1 {
        return;
    }
    let key = (width, height, radius, GetDpiForWindow(window));
    let unchanged =
        REGIONS.with(|cache| cache.borrow().get(&(window as usize)).copied() == Some(key));
    if unchanged {
        return;
    }
    REGIONS.with(|cache| {
        cache.borrow_mut().insert(window as usize, key);
    });
    let diameter = (radius * 2).min(width).min(height);
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
    if !region.is_null() && SetWindowRgn(window, region, 0) == 0 {
        // A successful SetWindowRgn transfers ownership to USER32.
        DeleteObject(region);
        REGIONS.with(|cache| {
            cache.borrow_mut().remove(&(window as usize));
        });
    }
}

unsafe extern "system" fn control_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass: usize,
    kind: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        REGIONS.with(|cache| {
            cache.borrow_mut().remove(&(window as usize));
        });
        LAYOUTS.with(|cache| {
            cache.borrow_mut().remove(&(window as usize));
        });
        RemoveWindowSubclass(window, Some(control_proc), subclass);
        return DefSubclassProc(window, message, wparam, lparam);
    }
    if message == WM_NCCALCSIZE && kind == EDIT && lparam != 0 {
        let result = DefSubclassProc(window, message, wparam, lparam);
        let rect = if wparam != 0 {
            &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0]
        } else {
            &mut *(lparam as *mut RECT)
        };
        let height = (rect.bottom - rect.top).max(0);
        let width = (rect.right - rect.left).max(0);
        let multiline = GetWindowLongPtrW(window, GWL_STYLE) & ES_MULTILINE as isize != 0;
        let vertical = if multiline {
            scaled(window, 5).min(height / 4)
        } else {
            ((height - scaled(window, 20)).max(0) / 2).min(scaled(window, 10))
        };
        let horizontal = scaled(window, 1).min(width / 4);
        rect.top += vertical;
        rect.bottom -= vertical;
        rect.left += horizontal;
        rect.right -= horizontal;
        return result;
    }
    let result = DefSubclassProc(window, message, wparam, lparam);
    match message {
        WM_SIZE => {
            rounded_region(window, scaled(window, if kind == LIST { 10 } else { 11 }));
        }
        WM_PAINT | WM_NCPAINT => paint_chrome(window, kind),
        WM_THEMECHANGED => {
            LAYOUTS.with(|cache| {
                cache.borrow_mut().remove(&(window as usize));
            });
            RedrawWindow(window, null(), null_mut(), RDW_INVALIDATE | RDW_FRAME);
        }
        WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => {
            RedrawWindow(window, null(), null_mut(), RDW_INVALIDATE | RDW_FRAME);
        }
        WM_MOUSEMOVE if kind != LIST => {
            if GetPropW(window, HOT.as_ptr()).is_null() {
                SetPropW(window, HOT.as_ptr(), std::ptr::without_provenance_mut(1));
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: window,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
                RedrawWindow(window, null(), null_mut(), RDW_INVALIDATE | RDW_FRAME);
            }
        }
        WM_MOUSELEAVE => {
            RemovePropW(window, HOT.as_ptr());
            RedrawWindow(window, null(), null_mut(), RDW_INVALIDATE | RDW_FRAME);
        }
        CB_SHOWDROPDOWN | WM_LBUTTONUP if kind == COMBO => {
            let mut info: COMBOBOXINFO = zeroed();
            info.cbSize = size_of::<COMBOBOXINFO>() as u32;
            if GetComboBoxInfo(window, &mut info) != 0 && !info.hwndList.is_null() {
                rounded_region(info.hwndList, scaled(window, 10));
            }
            paint_chrome(window, kind);
        }
        _ => {}
    }
    result
}

unsafe fn paint_chrome(window: HWND, kind: usize) {
    let dc = GetWindowDC(window);
    if dc.is_null() {
        return;
    }
    let saved = SaveDC(dc);
    if saved == 0 {
        ReleaseDC(window, dc);
        return;
    }
    let colors = COLORS.get();
    let mut outer: RECT = zeroed();
    GetWindowRect(window, &mut outer);
    let width = outer.right - outer.left;
    let height = outer.bottom - outer.top;
    let mut client: RECT = zeroed();
    GetClientRect(window, &mut client);
    let mut origin = POINT { x: 0, y: 0 };
    ClientToScreen(window, &mut origin);
    let inside = RECT {
        left: origin.x - outer.left,
        top: origin.y - outer.top,
        right: origin.x - outer.left + client.right,
        bottom: origin.y - outer.top + client.bottom,
    };
    let surface = CreateSolidBrush(colors.surface);
    let focused = GetFocus() == window || IsChild(window, GetFocus()) != 0;
    let hot = !GetPropW(window, HOT.as_ptr()).is_null();
    let stroke = if focused {
        colors.accent
    } else if hot {
        blend(colors.border, colors.accent)
    } else {
        colors.border
    };
    let pen = CreatePen(
        PS_SOLID,
        if focused { scaled(window, 2).max(1) } else { 1 },
        stroke,
    );
    if surface.is_null() || pen.is_null() {
        RestoreDC(dc, saved);
        if !surface.is_null() {
            DeleteObject(surface);
        }
        if !pen.is_null() {
            DeleteObject(pen);
        }
        ReleaseDC(window, dc);
        return;
    }
    let all = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    // Fill only the non-client padding, retaining native text, selection, caret
    // and (for multiline editors) the real scrollbar's paint and hit targets.
    let frame_saved = SaveDC(dc);
    ExcludeClipRect(dc, inside.left, inside.top, inside.right, inside.bottom);
    if GetWindowLongPtrW(window, GWL_STYLE) & WS_VSCROLL as isize != 0 {
        ExcludeClipRect(dc, inside.right, inside.top, width - 1, inside.bottom);
    }
    FillRect(dc, &all, surface);
    RestoreDC(dc, frame_saved);
    if kind == COMBO {
        // The classic COMBOBOX draws a bevel inside its client area too. Cover
        // that small shell after native painting, never the text/selection body.
        let edge = scaled(window, 3).min(height / 3).min(width / 3);
        for strip in [
            RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: edge,
            },
            RECT {
                left: 0,
                top: height - edge,
                right: width,
                bottom: height,
            },
            RECT {
                left: 0,
                top: 0,
                right: edge,
                bottom: height,
            },
            RECT {
                left: width - edge,
                top: 0,
                right: width,
                bottom: height,
            },
        ] {
            FillRect(dc, &strip, surface);
        }
        let dropped = SendMessageW(window, CB_GETDROPPEDSTATE, 0, 0) != 0;
        let arrow_background = CreateSolidBrush(if dropped {
            colors.raised
        } else {
            colors.surface
        });
        let side = scaled(window, 34).min(width);
        let arrow = RECT {
            left: width - side,
            top: 1,
            right: width - 1,
            bottom: height - 1,
        };
        if !arrow_background.is_null() {
            FillRect(dc, &arrow, arrow_background);
            DeleteObject(arrow_background);
        }
        let glyph = CreatePen(
            PS_SOLID,
            scaled(window, 2).max(1),
            if IsWindowEnabled(window) != 0 {
                colors.text
            } else {
                colors.border
            },
        );
        if !glyph.is_null() {
            let previous = SelectObject(dc, glyph);
            let x = width - side / 2;
            let y = height / 2;
            let d = scaled(window, 4);
            let vertical = scaled(window, 2);
            let points = [
                POINT {
                    x: x - d,
                    y: y - vertical,
                },
                POINT { x, y: y + vertical },
                POINT {
                    x: x + d,
                    y: y - vertical,
                },
            ];
            Polyline(dc, points.as_ptr(), 3);
            SelectObject(dc, previous);
            DeleteObject(glyph);
        }
    }
    SelectObject(dc, GetStockObject(NULL_BRUSH));
    SelectObject(dc, pen);
    let diameter = scaled(window, 22).min(height);
    RoundRect(dc, 0, 0, width - 1, height - 1, diameter, diameter);
    RestoreDC(dc, saved);
    DeleteObject(pen);
    DeleteObject(surface);
    ReleaseDC(window, dc);
}

fn blend(first: COLORREF, second: COLORREF) -> COLORREF {
    let channel = |shift: u32| (((first >> shift) & 255u32) + ((second >> shift) & 255u32)) / 2u32;
    channel(0) | (channel(8) << 8) | (channel(16) << 16)
}

unsafe fn scaled(window: HWND, value: i32) -> i32 {
    ((i64::from(value) * i64::from(GetDpiForWindow(window).max(96)) + 48) / 96) as i32
}
