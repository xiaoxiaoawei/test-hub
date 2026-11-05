//! Warm native palette and owner drawing. These helpers never borrow App state,
//! so synchronous WM_CTLCOLOR / WM_DRAWITEM callbacks can use them during layout.

use crate::config::ThemeMode;
use std::{mem::size_of, ptr::null};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, POINT, RECT},
    Graphics::{Dwm::*, Gdi::*},
    UI::{
        Controls::{
            SetWindowTheme, DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_HOTLIGHT, ODS_SELECTED,
        },
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            GetWindowTextLengthW, GetWindowTextW, SendMessageW, CB_GETLBTEXT, CB_GETLBTEXTLEN,
        },
    },
};

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
}

pub(crate) const BACKGROUND: COLORREF = rgb(0x21, 0x1d, 0x1b);
pub(crate) const SURFACE: COLORREF = rgb(0x30, 0x29, 0x25);
pub(crate) const TEXT: COLORREF = rgb(0xf1, 0xe5, 0xd8);
pub(crate) const MUTED: COLORREF = rgb(0xbb, 0xaa, 0x99);
pub(crate) const ACCENT: COLORREF = rgb(0xd6, 0xa5, 0x75);
pub(crate) const BORDER: COLORREF = rgb(0x59, 0x49, 0x3e);
const SURFACE_ACTIVE: COLORREF = rgb(0x40, 0x34, 0x2c);
const ACCENT_ACTIVE: COLORREF = rgb(0xbd, 0x8e, 0x62);

#[derive(Clone, Copy)]
struct Colors {
    background: COLORREF,
    surface: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    accent: COLORREF,
    border: COLORREF,
    surface_active: COLORREF,
    accent_active: COLORREF,
    combo_selected_text: COLORREF,
}

impl Colors {
    const fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self {
                background: BACKGROUND,
                surface: SURFACE,
                text: TEXT,
                muted: MUTED,
                accent: ACCENT,
                border: BORDER,
                surface_active: SURFACE_ACTIVE,
                accent_active: ACCENT_ACTIVE,
                combo_selected_text: ACCENT,
            },
            ThemeMode::Light => Self {
                background: rgb(0xf4, 0xea, 0xd8),
                surface: rgb(0xff, 0xf8, 0xea),
                text: rgb(0x33, 0x27, 0x1e),
                muted: rgb(0x75, 0x62, 0x4d),
                accent: rgb(0x8d, 0x5a, 0x39),
                border: rgb(0xa1, 0x8a, 0x6d),
                surface_active: rgb(0xe6, 0xd6, 0xbc),
                accent_active: rgb(0x73, 0x43, 0x29),
                combo_selected_text: rgb(0x33, 0x27, 0x1e),
            },
        }
    }
}

/// Keep this value on the UI thread until all windows using its brushes close.
pub(crate) struct Theme {
    pub background_color: COLORREF,
    pub surface_color: COLORREF,
    pub text_color: COLORREF,
    pub muted_color: COLORREF,
    pub accent_color: COLORREF,
    pub border_color: COLORREF,
    pub background: HBRUSH,
    pub surface: HBRUSH,
    pub accent: HBRUSH,
    surface_active: HBRUSH,
    accent_active: HBRUSH,
    border_pen: HPEN,
    accent_pen: HPEN,
    primary_pen: HPEN,
    check_pen: HPEN,
    combo_selected_text: COLORREF,
}

impl Theme {
    pub(crate) fn new(mode: ThemeMode) -> Result<Self, String> {
        let colors = Colors::for_mode(mode);
        let mut theme = Self {
            background_color: colors.background,
            surface_color: colors.surface,
            text_color: colors.text,
            muted_color: colors.muted,
            accent_color: colors.accent,
            border_color: colors.border,
            background: std::ptr::null_mut(),
            surface: std::ptr::null_mut(),
            accent: std::ptr::null_mut(),
            surface_active: std::ptr::null_mut(),
            accent_active: std::ptr::null_mut(),
            border_pen: std::ptr::null_mut(),
            accent_pen: std::ptr::null_mut(),
            primary_pen: std::ptr::null_mut(),
            check_pen: std::ptr::null_mut(),
            combo_selected_text: colors.combo_selected_text,
        };
        // Assign each resource immediately, so Drop also cleans partial failure.
        unsafe {
            theme.background = checked(CreateSolidBrush(theme.background_color))?;
            theme.surface = checked(CreateSolidBrush(theme.surface_color))?;
            theme.accent = checked(CreateSolidBrush(theme.accent_color))?;
            theme.surface_active = checked(CreateSolidBrush(colors.surface_active))?;
            theme.accent_active = checked(CreateSolidBrush(colors.accent_active))?;
            theme.border_pen = checked(CreatePen(PS_SOLID, 1, theme.border_color))?;
            theme.accent_pen = checked(CreatePen(PS_SOLID, 1, theme.accent_color))?;
            theme.primary_pen = checked(CreatePen(PS_SOLID, 1, colors.accent_active))?;
            theme.check_pen = checked(CreatePen(PS_SOLID, 2, colors.background))?;
        }
        Ok(theme)
    }

    /// Paint a BS_OWNERDRAW push button using the caller's current DPI font.
    ///
    /// # Safety
    /// `info` must come from the active WM_DRAWITEM call. Its window and device
    /// context, and `font`, must be valid on the current UI thread.
    pub(crate) unsafe fn draw_button(&self, info: &DRAWITEMSTRUCT, font: HFONT, primary: bool) {
        let dc = info.hDC;
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let disabled = info.itemState & ODS_DISABLED != 0;
        let pressed = info.itemState & ODS_SELECTED != 0;
        let hot = info.itemState & ODS_HOTLIGHT != 0;
        let focused = info.itemState & ODS_FOCUS != 0;
        let brush = if disabled {
            self.surface
        } else if primary {
            if pressed {
                self.accent_active
            } else {
                self.accent
            }
        } else if pressed || hot {
            self.surface_active
        } else {
            self.surface
        };
        let pen = if focused && !disabled {
            self.accent_pen
        } else if primary && !disabled {
            self.primary_pen
        } else {
            self.border_pen
        };
        let mut rect = info.rcItem;
        FillRect(dc, &rect, self.background);
        SelectObject(dc, brush);
        SelectObject(dc, pen);
        let radius = scale(info.hwndItem, 9);
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right - 1,
            rect.bottom - 1,
            radius,
            radius,
        );
        if focused && !disabled {
            let inset = scale(info.hwndItem, 3);
            let focus_rect = RECT {
                left: rect.left + inset,
                top: rect.top + inset,
                right: rect.right - inset,
                bottom: rect.bottom - inset,
            };
            SelectObject(dc, GetStockObject(NULL_BRUSH));
            SelectObject(
                dc,
                if primary {
                    self.check_pen
                } else {
                    self.accent_pen
                },
            );
            RoundRect(
                dc,
                focus_rect.left,
                focus_rect.top,
                focus_rect.right - 1,
                focus_rect.bottom - 1,
                radius,
                radius,
            );
        }
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(
            dc,
            if disabled {
                self.muted_color
            } else if primary {
                self.background_color
            } else {
                self.text_color
            },
        );
        SelectObject(dc, font);
        let label = label(info.hwndItem);
        let padding = scale(info.hwndItem, 8);
        rect.left += padding;
        rect.right -= padding;
        if pressed {
            rect.top += scale(info.hwndItem, 1);
        }
        DrawTextW(
            dc,
            label.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        RestoreDC(dc, saved);
    }

    /// Paint a CBS_OWNERDRAWFIXED | CBS_HASSTRINGS combo item in the same palette.
    ///
    /// # Safety
    /// `info` must come from the active WM_DRAWITEM call for a same-thread native
    /// COMBOBOX with CBS_HASSTRINGS. Its window, device context and `font` must be
    /// valid for this call, and the combo must not change its item strings here.
    pub(crate) unsafe fn draw_combo(&self, info: &DRAWITEMSTRUCT, font: HFONT) {
        let dc = info.hDC;
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let disabled = info.itemState & ODS_DISABLED != 0;
        let selected = info.itemState & ODS_SELECTED != 0;
        let focused = info.itemState & ODS_FOCUS != 0;
        let mut rect = info.rcItem;
        FillRect(
            dc,
            &rect,
            if selected && !disabled {
                self.surface_active
            } else {
                self.surface
            },
        );
        if focused && !disabled {
            FrameRect(dc, &rect, self.accent);
        }
        let mut text: Vec<u16> = "选择服务\0".encode_utf16().collect();
        if info.itemID != u32::MAX {
            let length = SendMessageW(info.hwndItem, CB_GETLBTEXTLEN, info.itemID as usize, 0);
            // CB_GETLBTEXT has no capacity argument. The same-thread, HASSTRINGS
            // contract keeps the item stable between these two native messages.
            if (0..=4096).contains(&length) {
                text = vec![0; length as usize + 1];
                let read = SendMessageW(
                    info.hwndItem,
                    CB_GETLBTEXT,
                    info.itemID as usize,
                    text.as_mut_ptr() as isize,
                );
                if read < 0 {
                    text[0] = 0;
                }
            } else {
                text = vec![0];
            }
        }
        SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(
            dc,
            if disabled {
                self.muted_color
            } else if selected {
                self.combo_selected_text
            } else {
                self.text_color
            },
        );
        rect.left += scale(info.hwndItem, 10);
        rect.right -= scale(info.hwndItem, 8);
        DrawTextW(
            dc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        RestoreDC(dc, saved);
    }

    /// Paint an owner-drawn checkbox. The caller owns and supplies checked state.
    ///
    /// # Safety
    /// `info` must come from the active WM_DRAWITEM call. Its window and device
    /// context, and `font`, must be valid on the current UI thread.
    pub(crate) unsafe fn draw_checkbox(&self, info: &DRAWITEMSTRUCT, font: HFONT, checked: bool) {
        let dc = info.hDC;
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let disabled = info.itemState & ODS_DISABLED != 0;
        let focused = info.itemState & ODS_FOCUS != 0;
        let size = scale(info.hwndItem, 17);
        let rect = info.rcItem;
        let left = rect.left + scale(info.hwndItem, 2);
        let top = rect.top + ((rect.bottom - rect.top - size) / 2);
        FillRect(dc, &rect, self.background);
        SelectObject(
            dc,
            if checked && !disabled {
                self.accent
            } else {
                self.surface
            },
        );
        SelectObject(
            dc,
            if focused && !disabled {
                self.accent_pen
            } else {
                self.border_pen
            },
        );
        let radius = scale(info.hwndItem, 5);
        RoundRect(dc, left, top, left + size, top + size, radius, radius);
        if checked {
            SelectObject(dc, self.check_pen);
            let points = [
                POINT {
                    x: left + scale(info.hwndItem, 4),
                    y: top + scale(info.hwndItem, 9),
                },
                POINT {
                    x: left + scale(info.hwndItem, 7),
                    y: top + scale(info.hwndItem, 12),
                },
                POINT {
                    x: left + scale(info.hwndItem, 13),
                    y: top + scale(info.hwndItem, 5),
                },
            ];
            Polyline(dc, points.as_ptr(), points.len() as i32);
        }
        SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(
            dc,
            if disabled {
                self.muted_color
            } else {
                self.text_color
            },
        );
        let mut text_rect = RECT {
            left: left + size + scale(info.hwndItem, 10),
            ..rect
        };
        let label = label(info.hwndItem);
        DrawTextW(
            dc,
            label.as_ptr(),
            -1,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        RestoreDC(dc, saved);
    }
}

impl Drop for Theme {
    fn drop(&mut self) {
        for object in [
            self.background,
            self.surface,
            self.accent,
            self.surface_active,
            self.accent_active,
            self.border_pen,
            self.accent_pen,
            self.primary_pen,
            self.check_pen,
        ] {
            if !object.is_null() {
                unsafe {
                    DeleteObject(object);
                }
            }
        }
    }
}

fn checked(object: HGDIOBJ) -> Result<HGDIOBJ, String> {
    if object.is_null() {
        Err("无法创建界面资源，请关闭部分程序后重试。".into())
    } else {
        Ok(object)
    }
}

/// Apply documented DWM caption attributes for the selected palette. Unsupported attributes are
/// optional hints; the application still paints its own client area.
///
/// # Safety
/// `hwnd` must refer to a live top-level window owned by the current UI thread.
pub(crate) unsafe fn apply_window(hwnd: HWND, mode: ThemeMode) {
    let colors = Colors::for_mode(mode);
    attribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &i32::from(mode == ThemeMode::Dark),
    );
    attribute(hwnd, DWMWA_CAPTION_COLOR, &colors.background);
    attribute(hwnd, DWMWA_TEXT_COLOR, &colors.text);
    attribute(hwnd, DWMWA_BORDER_COLOR, &colors.border);
    attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND);
}

/// Hint to a native child control that its border/scrollbar should use the
/// matching Explorer theme. WM_CTLCOLOR and owner drawing supply the actual palette.
///
/// # Safety
/// `hwnd` must refer to a live native child control on the current UI thread.
pub(crate) unsafe fn style_edit(hwnd: HWND, mode: ThemeMode) {
    let theme: Vec<u16> = match mode {
        ThemeMode::Dark => "DarkMode_Explorer\0",
        ThemeMode::Light => "Explorer\0",
    }
    .encode_utf16()
    .collect();
    let _ = SetWindowTheme(hwnd, theme.as_ptr(), null());
}

unsafe fn attribute<T>(hwnd: HWND, id: DWMWINDOWATTRIBUTE, value: &T) {
    let _ = DwmSetWindowAttribute(
        hwnd,
        id as u32,
        (value as *const T).cast(),
        size_of::<T>() as u32,
    );
}

unsafe fn label(hwnd: HWND) -> Vec<u16> {
    let count = GetWindowTextLengthW(hwnd).clamp(0, 4096) as usize;
    let mut value = vec![0; count + 1];
    GetWindowTextW(hwnd, value.as_mut_ptr(), value.len() as i32);
    value
}

unsafe fn scale(hwnd: HWND, value: i32) -> i32 {
    ((value as i64 * GetDpiForWindow(hwnd).max(96) as i64 + 48) / 96) as i32
}
