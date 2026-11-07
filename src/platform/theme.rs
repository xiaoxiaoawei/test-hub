//! Native palette families and rounded owner drawing. These helpers never borrow App state,
//! so synchronous WM_CTLCOLOR / WM_DRAWITEM callbacks can use them during layout.

use crate::{
    config::{ThemeMode, ThemePreset},
    visual::Palette,
};
use std::{mem::size_of, ptr::null};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, POINT, RECT},
    Graphics::{Dwm::*, Gdi::*},
    UI::{
        Controls::{
            SetWindowTheme, DRAWITEMSTRUCT, ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_FOCUS,
            ODS_HOTLIGHT, ODS_SELECTED,
        },
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            GetWindowTextLengthW, GetWindowTextW, SendMessageW, CB_GETLBTEXT, CB_GETLBTEXTLEN,
        },
    },
};

pub(crate) const fn color(rgb: u32) -> COLORREF {
    ((rgb >> 16) & 255) | (rgb & 0xFF00) | ((rgb & 255) << 16)
}

/// Keep this value on the UI thread until all windows using its brushes close.
pub(crate) struct Theme {
    pub background_color: COLORREF,
    pub surface_color: COLORREF,
    pub card_color: COLORREF,
    pub raised_color: COLORREF,
    pub on_accent_color: COLORREF,
    mode: ThemeMode,
    pub text_color: COLORREF,
    pub muted_color: COLORREF,
    pub accent_color: COLORREF,
    pub border_color: COLORREF,
    pub background: HBRUSH,
    pub surface: HBRUSH,
    pub card: HBRUSH,
    pub raised: HBRUSH,
    pub accent: HBRUSH,
    surface_active: HBRUSH,
    selection: HBRUSH,
    accent_active: HBRUSH,
    border_pen: HPEN,
    accent_pen: HPEN,
    primary_pen: HPEN,
    check_pen: HPEN,
    combo_selected_text: COLORREF,
}

impl Theme {
    pub(crate) fn new(mode: ThemeMode, preset: ThemePreset) -> Result<Self, String> {
        let colors = Palette::for_theme(mode, preset);
        let mut theme = Self {
            mode,
            card_color: color(colors.card),
            raised_color: color(colors.raised),
            on_accent_color: color(colors.on_accent),
            background_color: color(colors.background),
            surface_color: color(colors.surface),
            text_color: color(colors.text),
            muted_color: color(colors.muted),
            accent_color: color(colors.accent),
            border_color: color(colors.border),
            background: std::ptr::null_mut(),
            surface: std::ptr::null_mut(),
            card: std::ptr::null_mut(),
            raised: std::ptr::null_mut(),
            accent: std::ptr::null_mut(),
            surface_active: std::ptr::null_mut(),
            selection: std::ptr::null_mut(),
            accent_active: std::ptr::null_mut(),
            border_pen: std::ptr::null_mut(),
            accent_pen: std::ptr::null_mut(),
            primary_pen: std::ptr::null_mut(),
            check_pen: std::ptr::null_mut(),
            combo_selected_text: color(colors.selection_text),
        };
        // Assign each resource immediately, so Drop also cleans partial failure.
        unsafe {
            theme.background = checked(CreateSolidBrush(theme.background_color))?;
            theme.surface = checked(CreateSolidBrush(theme.surface_color))?;
            theme.accent = checked(CreateSolidBrush(theme.accent_color))?;
            theme.card = checked(CreateSolidBrush(theme.card_color))?;
            theme.raised = checked(CreateSolidBrush(theme.raised_color))?;
            theme.surface_active = checked(CreateSolidBrush(theme.raised_color))?;
            theme.selection = checked(CreateSolidBrush(color(colors.selection)))?;
            theme.accent_active = checked(CreateSolidBrush(color(colors.accent_active)))?;
            theme.border_pen = checked(CreatePen(PS_SOLID, 1, theme.border_color))?;
            theme.accent_pen = checked(CreatePen(PS_SOLID, 1, theme.accent_color))?;
            theme.primary_pen = checked(CreatePen(PS_SOLID, 1, color(colors.accent_active)))?;
            theme.check_pen = checked(CreatePen(PS_SOLID, 2, theme.on_accent_color))?;
        }
        Ok(theme)
    }

    unsafe fn control_backdrop(&self, control: HWND) -> HBRUSH {
        if super::surfaces::is_card_control(control) {
            self.card
        } else {
            self.background
        }
    }

    /// Paint a rounded grouping surface after the window background is filled.
    pub(crate) unsafe fn draw_card(&self, dc: HDC, rect: RECT, radius: i32) {
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        SelectObject(dc, self.card);
        // Surface contrast groups content without drawing a second frame around it.
        SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius * 2,
            radius * 2,
        );
        RestoreDC(dc, saved);
    }

    /// Paint a native sidebar button while retaining its label and accessibility.
    ///
    /// # Safety
    /// `info` and `font` must be valid for the current UI thread's WM_DRAWITEM.
    pub(crate) unsafe fn draw_navigation(
        &self,
        info: &DRAWITEMSTRUCT,
        font: HFONT,
        selected: bool,
    ) {
        let dc = info.hDC;
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let disabled = info.itemState & ODS_DISABLED != 0;
        let pressed = info.itemState & ODS_SELECTED != 0;
        let hot = info.itemState & ODS_HOTLIGHT != 0;
        let focused = info.itemState & ODS_FOCUS != 0;
        let bounds = info.rcItem;
        FillRect(dc, &bounds, self.card);
        let diameter = scale(info.hwndItem, 8);
        SelectObject(dc, GetStockObject(NULL_PEN));
        if !disabled && (selected || pressed || hot) {
            SelectObject(dc, if pressed { self.surface } else { self.raised });
            RoundRect(
                dc,
                bounds.left,
                bounds.top,
                bounds.right,
                bounds.bottom,
                diameter,
                diameter,
            );
        }
        if selected {
            let indicator_height = scale(info.hwndItem, 16);
            let top = bounds.top + (bounds.bottom - bounds.top - indicator_height) / 2;
            SelectObject(dc, if disabled { self.surface } else { self.accent });
            let left = bounds.left + scale(info.hwndItem, 2);
            RoundRect(
                dc,
                left,
                top,
                left + scale(info.hwndItem, 3),
                top + indicator_height,
                scale(info.hwndItem, 3),
                scale(info.hwndItem, 3),
            );
        }
        if focused && !disabled {
            let inset = scale(info.hwndItem, 2);
            SelectObject(dc, self.accent_pen);
            SelectObject(dc, GetStockObject(NULL_BRUSH));
            RoundRect(
                dc,
                bounds.left + inset,
                bounds.top + inset,
                bounds.right - inset,
                bounds.bottom - inset,
                diameter,
                diameter,
            );
        }
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(
            dc,
            if disabled {
                self.muted_color
            } else {
                self.text_color
            },
        );
        SelectObject(dc, font);
        let mut text_rect = RECT {
            left: bounds.left + scale(info.hwndItem, 20),
            right: bounds.right - scale(info.hwndItem, 10),
            ..bounds
        };
        if pressed && !disabled {
            text_rect.top += scale(info.hwndItem, 1);
        }
        let text = label(info.hwndItem);
        DrawTextW(
            dc,
            text.as_ptr(),
            -1,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        RestoreDC(dc, saved);
    }

    /// A real BUTTON remains underneath this preview for keyboard and MSAA.
    pub(crate) unsafe fn draw_preset(
        &self,
        info: &DRAWITEMSTRUCT,
        font: HFONT,
        preset: ThemePreset,
        selected: bool,
    ) {
        let dc = info.hDC;
        let colors = Palette::for_theme(self.mode, preset);
        let disabled = info.itemState & ODS_DISABLED != 0;
        let pressed = info.itemState & ODS_SELECTED != 0;
        let hot = info.itemState & ODS_HOTLIGHT != 0;
        let focused = info.itemState & ODS_FOCUS != 0;
        let card = GdiObject(CreateSolidBrush(color(if (hot || pressed) && !disabled {
            colors.raised
        } else {
            colors.card
        })));
        let outline = GdiObject(CreatePen(
            PS_SOLID,
            scale(info.hwndItem, 1),
            color(if selected && !disabled {
                colors.accent
            } else {
                colors.border
            }),
        ));
        let title_font = sized_font(font, info.hwndItem, 16, FW_SEMIBOLD as i32);
        let detail_font = sized_font(font, info.hwndItem, 13, FW_NORMAL as i32);
        if card.0.is_null() || outline.0.is_null() {
            return;
        }
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let bounds = info.rcItem;
        FillRect(dc, &bounds, self.background);
        SelectObject(dc, card.0);
        SelectObject(dc, outline.0);
        let gap = scale(info.hwndItem, 2);
        let rect = RECT {
            left: bounds.left + gap,
            top: bounds.top + gap,
            right: bounds.right - gap,
            bottom: bounds.bottom - gap,
        };
        let diameter = scale(info.hwndItem, 24);
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            diameter,
            diameter,
        );
        SelectObject(dc, GetStockObject(NULL_PEN));
        for (index, swatch) in [colors.accent, colors.raised, colors.background]
            .into_iter()
            .enumerate()
        {
            let brush = GdiObject(CreateSolidBrush(color(swatch)));
            if !brush.0.is_null() {
                let previous = SelectObject(dc, brush.0);
                let x = rect.left + scale(info.hwndItem, 16 + index as i32 * 16);
                let y = rect.top + scale(info.hwndItem, 15);
                let side = scale(info.hwndItem, 12);
                let corner = scale(info.hwndItem, 8);
                RoundRect(dc, x, y, x + side, y + side, corner, corner);
                SelectObject(dc, previous);
            }
        }
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color(if disabled { colors.muted } else { colors.text }));
        SelectObject(
            dc,
            if title_font.0.is_null() {
                font
            } else {
                title_font.0
            },
        );
        let mut title = RECT {
            left: rect.left + scale(info.hwndItem, 16),
            top: rect.top + scale(info.hwndItem, 34),
            right: rect.right - scale(info.hwndItem, 12),
            bottom: rect.top + scale(info.hwndItem, 60),
        };
        DrawTextW(
            dc,
            wide(preset.label()).as_ptr(),
            -1,
            &mut title,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(
            dc,
            if detail_font.0.is_null() {
                font
            } else {
                detail_font.0
            },
        );
        SetTextColor(
            dc,
            color(if !disabled && (hot || pressed) {
                colors.text
            } else {
                colors.muted
            }),
        );
        let mut detail = RECT {
            top: rect.top + scale(info.hwndItem, 64),
            bottom: rect.bottom - scale(info.hwndItem, 8),
            ..title
        };
        DrawTextW(
            dc,
            wide(preset.subtitle()).as_ptr(),
            -1,
            &mut detail,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        if selected {
            SelectObject(dc, outline.0);
            let x = rect.right - scale(info.hwndItem, 24);
            let y = rect.top + scale(info.hwndItem, 21);
            let points = [
                POINT {
                    x: x - scale(info.hwndItem, 4),
                    y,
                },
                POINT {
                    x: x - scale(info.hwndItem, 1),
                    y: y + scale(info.hwndItem, 3),
                },
                POINT {
                    x: x + scale(info.hwndItem, 5),
                    y: y - scale(info.hwndItem, 4),
                },
            ];
            Polyline(dc, points.as_ptr(), points.len() as i32);
        }
        if focused && !disabled {
            SelectObject(dc, self.accent_pen);
            SelectObject(dc, GetStockObject(NULL_BRUSH));
            let inset = scale(info.hwndItem, 4);
            RoundRect(
                dc,
                rect.left + inset,
                rect.top + inset,
                rect.right - inset,
                rect.bottom - inset,
                diameter - inset,
                diameter - inset,
            );
        }
        // Restore selected objects before RAII releases the temporary GDI handles.
        RestoreDC(dc, saved);
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
        FillRect(dc, &rect, self.control_backdrop(info.hwndItem));
        SelectObject(dc, brush);
        SelectObject(dc, pen);
        let radius = scale(info.hwndItem, 16);
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
                self.on_accent_color
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
        let collapsed = info.itemState & ODS_COMBOBOXEDIT != 0;
        if collapsed {
            let mut combo: windows_sys::Win32::UI::Controls::COMBOBOXINFO = std::mem::zeroed();
            combo.cbSize = size_of::<windows_sys::Win32::UI::Controls::COMBOBOXINFO>() as u32;
            if windows_sys::Win32::UI::Controls::GetComboBoxInfo(info.hwndItem, &mut combo) != 0 {
                let native_arrow = (combo.rcButton.right - combo.rcButton.left).max(0);
                rect.right -= (scale(info.hwndItem, 34) - native_arrow).max(0);
            }
        }
        FillRect(
            dc,
            &rect,
            if selected && !disabled {
                self.selection
            } else {
                self.surface
            },
        );
        if focused && !disabled && !collapsed {
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
        FillRect(dc, &rect, self.control_backdrop(info.hwndItem));
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
        let radius = scale(info.hwndItem, 8);
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
            self.card,
            self.raised,
            self.selection,
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

/// Temporary preview resources are always released, including allocation failure.
struct GdiObject(HGDIOBJ);

impl Drop for GdiObject {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

unsafe fn sized_font(font: HFONT, hwnd: HWND, height: i32, weight: i32) -> GdiObject {
    let mut face: LOGFONTW = std::mem::zeroed();
    if GetObjectW(
        font,
        size_of::<LOGFONTW>() as i32,
        (&mut face as *mut LOGFONTW).cast(),
    ) == 0
    {
        return GdiObject(std::ptr::null_mut());
    }
    face.lfHeight = -scale(hwnd, height);
    face.lfWeight = weight;
    GdiObject(CreateFontIndirectW(&face))
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
pub(crate) unsafe fn apply_window(hwnd: HWND, mode: ThemeMode, preset: ThemePreset) {
    let colors = Palette::for_theme(mode, preset);
    attribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &i32::from(mode == ThemeMode::Dark),
    );
    attribute(hwnd, DWMWA_CAPTION_COLOR, &color(colors.background));
    attribute(hwnd, DWMWA_TEXT_COLOR, &color(colors.text));
    attribute(hwnd, DWMWA_BORDER_COLOR, &color(colors.border));
    attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND);
}

/// Hint to a native child control that its border/scrollbar should use the
/// matching Explorer theme. WM_CTLCOLOR and owner drawing supply the actual palette.
///
/// # Safety
/// `hwnd` must refer to a live native child control on the current UI thread.
pub(crate) unsafe fn style_edit(hwnd: HWND, mode: ThemeMode) {
    let mut class = [0u16; 24];
    let length = windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(
        hwnd,
        class.as_mut_ptr(),
        class.len() as i32,
    )
    .max(0) as usize;
    let combo = String::from_utf16_lossy(&class[..length]).eq_ignore_ascii_case("COMBOBOX");
    let theme: Vec<u16> = match mode {
        ThemeMode::Dark => "DarkMode_Explorer\0",
        ThemeMode::Light => "Explorer\0",
    }
    .encode_utf16()
    .collect();
    if combo {
        let empty = [0u16];
        let _ = SetWindowTheme(hwnd, empty.as_ptr(), empty.as_ptr());
    } else {
        let _ = SetWindowTheme(hwnd, theme.as_ptr(), null());
    }
}

unsafe fn attribute<T>(hwnd: HWND, id: DWMWINDOWATTRIBUTE, value: &T) {
    let _ = DwmSetWindowAttribute(
        hwnd,
        id as u32,
        (value as *const T).cast(),
        size_of::<T>() as u32,
    );
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
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
