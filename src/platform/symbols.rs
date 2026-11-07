//! Small owner-drawn controls. Labels stay on the native BUTTON for accessibility;
//! the visible mark is drawn with GDI and does not depend on a symbol font.

use super::theme::Theme;
use std::ptr;
use windows_sys::Win32::{
    Foundation::{COLORREF, POINT},
    Graphics::Gdi::*,
    UI::{
        Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_HOTLIGHT, ODS_SELECTED},
        HiDpi::GetDpiForWindow,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Symbol {
    Sun,
    Moon,
    Copy,
    Connection,
    Send,
    Expand,
    Settings,
    Close,
    Stop,
    Retry,
    Pin,
    PinActive,
    Swap,
    Eye,
    EyeClosed,
}

/// Draw a quiet icon button; its native text remains available to assistive tools.
///
/// # Safety
/// `item` must describe the current, same-thread `WM_DRAWITEM` operation. Its DC
/// and rectangle must be valid. The theme must remain alive throughout this call.
pub(crate) unsafe fn draw_button(
    item: &DRAWITEMSTRUCT,
    _font: HFONT,
    palette: &Theme,
    symbol: Symbol,
) {
    let Some(mut paint) = (unsafe { Paint::begin(item.hDC) }) else {
        return;
    };
    let rect = item.rcItem;
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    let dpi = unsafe { GetDpiForWindow(item.hwndItem) }.max(96);
    let scale = |value: i32| ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32;
    let disabled = item.itemState & ODS_DISABLED != 0;
    let pressed = item.itemState & ODS_SELECTED != 0 && !disabled;
    let hot = item.itemState & ODS_HOTLIGHT != 0 && !disabled;
    let focused = item.itemState & ODS_FOCUS != 0 && !disabled;
    let latched = matches!(symbol, Symbol::PinActive) && !disabled;
    let on_card = unsafe { super::surfaces::is_card_control(item.hwndItem) };
    let backdrop = if on_card {
        palette.card_color
    } else {
        palette.background_color
    };
    let background = if pressed {
        blend(palette.surface_color, palette.accent_color, 24)
    } else if latched {
        blend(backdrop, palette.accent_color, 18)
    } else if hot {
        palette.surface_color
    } else {
        backdrop
    };
    let foreground = if disabled {
        blend(backdrop, palette.muted_color, 58)
    } else if hot
        || pressed
        || latched
        || matches!(symbol, Symbol::Sun | Symbol::Moon | Symbol::Send)
    {
        palette.accent_color
    } else {
        palette.text_color
    };

    unsafe {
        IntersectClipRect(item.hDC, rect.left, rect.top, rect.right, rect.bottom);
        FillRect(
            item.hDC,
            &rect,
            if on_card {
                palette.card
            } else {
                palette.background
            },
        );
        if hot || pressed || latched {
            if let Some(brush) = paint.brush(background) {
                SelectObject(item.hDC, brush);
                SelectObject(item.hDC, GetStockObject(NULL_PEN));
                let radius = scale(9);
                RoundRect(
                    item.hDC,
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    radius,
                    radius,
                );
            }
        }
        if focused {
            if let Some(pen) = paint.pen(scale(1).max(1), palette.accent_color) {
                SelectObject(item.hDC, pen);
                SelectObject(item.hDC, GetStockObject(NULL_BRUSH));
                let inset = scale(2);
                let radius = scale(7);
                RoundRect(
                    item.hDC,
                    rect.left + inset,
                    rect.top + inset,
                    rect.right - inset,
                    rect.bottom - inset,
                    radius,
                    radius,
                );
            }
        }

        let Some(pen) = paint.pen(scale(2).max(1), foreground) else {
            return;
        };
        SelectObject(item.hDC, pen);
        SelectObject(item.hDC, GetStockObject(NULL_BRUSH));
        let side = (rect.right - rect.left).min(rect.bottom - rect.top);
        let size = scale(20).min((side - scale(8)).max(0));
        if size < 8 {
            return;
        }
        let offset = if pressed { scale(1) } else { 0 };
        let canvas = Canvas {
            dc: item.hDC,
            left: rect.left + (rect.right - rect.left - size) / 2 + offset,
            top: rect.top + (rect.bottom - rect.top - size) / 2 + offset,
            size,
        };
        canvas.draw(symbol);
    }
}

// SaveDC does not own selected objects. Restore first, then delete our pens and
// brush, including on an allocation failure or an early return above.
struct Paint {
    dc: HDC,
    saved: i32,
    owned: [HGDIOBJ; 3],
    count: usize,
}

impl Paint {
    unsafe fn begin(dc: HDC) -> Option<Self> {
        let saved = unsafe { SaveDC(dc) };
        (saved != 0).then_some(Self {
            dc,
            saved,
            owned: [ptr::null_mut(); 3],
            count: 0,
        })
    }

    fn own(&mut self, object: HGDIOBJ) -> Option<HGDIOBJ> {
        if object.is_null() {
            return None;
        }
        // The button uses at most a background brush, focus pen and glyph pen.
        debug_assert!(self.count < self.owned.len());
        self.owned[self.count] = object;
        self.count += 1;
        Some(object)
    }

    unsafe fn pen(&mut self, width: i32, color: COLORREF) -> Option<HPEN> {
        self.own(unsafe { CreatePen(PS_SOLID, width, color) })
    }

    unsafe fn brush(&mut self, color: COLORREF) -> Option<HBRUSH> {
        self.own(unsafe { CreateSolidBrush(color) })
    }
}

impl Drop for Paint {
    fn drop(&mut self) {
        unsafe {
            RestoreDC(self.dc, self.saved);
            for object in &self.owned[..self.count] {
                DeleteObject(*object);
            }
        }
    }
}

struct Canvas {
    dc: HDC,
    left: i32,
    top: i32,
    size: i32,
}

impl Canvas {
    fn point(&self, x: i32, y: i32) -> POINT {
        POINT {
            x: self.left + (x * self.size + 12) / 24,
            y: self.top + (y * self.size + 12) / 24,
        }
    }

    unsafe fn line<const N: usize>(&self, points: &[(i32, i32); N]) {
        let points = points.map(|(x, y)| self.point(x, y));
        unsafe { Polyline(self.dc, points.as_ptr(), points.len() as i32) };
    }

    unsafe fn curve<const N: usize>(&self, points: &[(i32, i32); N]) {
        let points = points.map(|(x, y)| self.point(x, y));
        unsafe { PolyBezier(self.dc, points.as_ptr(), points.len() as u32) };
    }

    unsafe fn circle(&self, left: i32, top: i32, right: i32, bottom: i32) {
        let a = self.point(left, top);
        let b = self.point(right, bottom);
        unsafe { Ellipse(self.dc, a.x, a.y, b.x, b.y) };
    }

    unsafe fn draw(&self, symbol: Symbol) {
        unsafe {
            match symbol {
                Symbol::Sun => {
                    self.circle(7, 7, 17, 17);
                    for ray in [
                        [(12, 1), (12, 4)],
                        [(12, 20), (12, 23)],
                        [(1, 12), (4, 12)],
                        [(20, 12), (23, 12)],
                        [(4, 4), (6, 6)],
                        [(18, 18), (20, 20)],
                        [(4, 20), (6, 18)],
                        [(18, 6), (20, 4)],
                    ] {
                        self.line(&ray);
                    }
                }
                Symbol::Moon => self.curve(&[
                    (18, 3),
                    (12, 1),
                    (5, 4),
                    (4, 11),
                    (3, 18),
                    (9, 23),
                    (16, 21),
                    (19, 20),
                    (21, 18),
                    (22, 15),
                    (16, 17),
                    (10, 13),
                    (10, 8),
                    (10, 5),
                    (13, 2),
                    (18, 3),
                ]),
                Symbol::Copy => {
                    self.line(&[(6, 16), (4, 16), (4, 3), (16, 3), (16, 5)]);
                    let a = self.point(8, 7);
                    let b = self.point(20, 21);
                    let radius = (self.size / 7).max(2);
                    RoundRect(self.dc, a.x, a.y, b.x, b.y, radius, radius);
                }
                Symbol::Connection => {
                    // A small plug: connection testing remains recognizable at
                    // 16–20 pixels without relying on a checkmark font glyph.
                    self.line(&[
                        (7, 8),
                        (7, 12),
                        (9, 15),
                        (15, 15),
                        (17, 12),
                        (17, 8),
                        (7, 8),
                    ]);
                    self.line(&[(9, 3), (9, 8)]);
                    self.line(&[(15, 3), (15, 8)]);
                    self.line(&[(12, 15), (12, 20), (18, 20)]);
                }
                Symbol::Send => {
                    self.line(&[(3, 10), (22, 2), (14, 21), (11, 13), (3, 10)]);
                    self.line(&[(11, 13), (22, 2)]);
                }
                Symbol::Expand => {
                    for corner in [
                        [(4, 10), (4, 4), (10, 4)],
                        [(14, 4), (20, 4), (20, 10)],
                        [(20, 14), (20, 20), (14, 20)],
                        [(10, 20), (4, 20), (4, 14)],
                    ] {
                        self.line(&corner);
                    }
                }
                Symbol::Settings => {
                    self.line(&[
                        (10, 2),
                        (14, 2),
                        (15, 5),
                        (17, 6),
                        (20, 5),
                        (22, 9),
                        (20, 11),
                        (20, 13),
                        (22, 15),
                        (20, 19),
                        (17, 18),
                        (15, 19),
                        (14, 22),
                        (10, 22),
                        (9, 19),
                        (7, 18),
                        (4, 19),
                        (2, 15),
                        (4, 13),
                        (4, 11),
                        (2, 9),
                        (4, 5),
                        (7, 6),
                        (9, 5),
                        (10, 2),
                    ]);
                    self.circle(8, 8, 16, 16);
                }
                Symbol::Close => {
                    self.line(&[(6, 6), (18, 18)]);
                    self.line(&[(18, 6), (6, 18)]);
                }
                Symbol::Stop => {
                    self.line(&[(6, 6), (18, 6), (18, 18), (6, 18), (6, 6)]);
                }
                Symbol::Retry => {
                    self.curve(&[
                        (20, 9),
                        (16, 3),
                        (10, 2),
                        (6, 6),
                        (1, 11),
                        (4, 20),
                        (11, 21),
                        (17, 23),
                        (22, 18),
                        (21, 13),
                    ]);
                    self.line(&[(14, 9), (20, 9), (20, 3)]);
                }
                Symbol::Pin | Symbol::PinActive => {
                    self.line(&[
                        (8, 3),
                        (18, 3),
                        (16, 7),
                        (16, 12),
                        (20, 16),
                        (6, 16),
                        (10, 12),
                        (10, 7),
                        (8, 3),
                    ]);
                    self.line(&[(13, 16), (13, 23)]);
                    if symbol == Symbol::PinActive {
                        self.line(&[(12, 6), (14, 6)]);
                        self.line(&[(12, 9), (14, 9)]);
                        self.line(&[(11, 13), (15, 13)]);
                    }
                }
                Symbol::Swap => {
                    self.line(&[(3, 7), (20, 7)]);
                    self.line(&[(16, 3), (20, 7), (16, 11)]);
                    self.line(&[(21, 17), (4, 17)]);
                    self.line(&[(8, 13), (4, 17), (8, 21)]);
                }
                Symbol::Eye | Symbol::EyeClosed => {
                    self.curve(&[
                        (2, 12),
                        (6, 4),
                        (18, 4),
                        (22, 12),
                        (18, 20),
                        (6, 20),
                        (2, 12),
                    ]);
                    self.circle(9, 9, 15, 15);
                    if symbol == Symbol::EyeClosed {
                        self.line(&[(4, 3), (20, 21)]);
                    }
                }
            }
        }
    }
}

fn blend(first: COLORREF, second: COLORREF, amount: u32) -> COLORREF {
    let mut color = 0;
    for shift in [0, 8, 16] {
        let a = (first >> shift) & 255;
        let b = (second >> shift) & 255;
        color |= ((a * (100 - amount) + b * amount + 50) / 100) << shift;
    }
    color
}
