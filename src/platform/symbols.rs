//! Small owner-drawn controls. Labels stay on the native BUTTON for accessibility;
//! the visible mark is drawn with GDI and does not depend on a symbol font.

use super::theme::Theme;
use std::{mem::zeroed, ptr};
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
    Speak,
    SpeakStop,
    Polish,
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
    let latched = matches!(symbol, Symbol::PinActive | Symbol::SpeakStop) && !disabled;
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

        let side = (rect.right - rect.left).min(rect.bottom - rect.top);
        let size = scale(20).min((side - scale(8)).max(0));
        if size < 8 {
            return;
        }
        let offset = if pressed { scale(1) } else { 0 };
        let left = rect.left + (rect.right - rect.left - size) / 2 + offset;
        let top = rect.top + (rect.bottom - rect.top - size) / 2 + offset;
        // Draw vectors at the actual destination DPI, with subpixel coverage
        // for their curves and diagonals. Only this tiny glyph is sampled; text
        // remains native ClearType, and no complete UI bitmap is enlarged.
        let stroke = ((u64::from(dpi) * 2 * GLYPH_SAMPLES as u64 + 48) / 96) as i32;
        if let Some(coverage) = glyph_coverage(symbol, size, stroke) {
            if draw_coverage(item.hDC, left, top, size, &coverage, foreground, background) {
                return;
            }
        }
        // Allocation failure keeps the original vector mark functional.
        let Some(pen) = paint.pen(scale(2).max(1), foreground) else {
            return;
        };
        SelectObject(item.hDC, pen);
        SelectObject(item.hDC, GetStockObject(NULL_BRUSH));
        Canvas {
            dc: item.hDC,
            left,
            top,
            size,
        }
        .draw(symbol);
    }
}

// Coverage sampling improves the original un-antialiased GDI strokes without
// introducing an image decoder, a font glyph dependency, or a renderer runtime.
// The resulting bitmap is always copied 1:1 into device pixels.
const GLYPH_SAMPLES: usize = 4;

struct GlyphBitmap {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u8,
}

impl GlyphBitmap {
    unsafe fn new(side: usize) -> Option<Self> {
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = side as i32;
        info.bmiHeader.biHeight = -(side as i32);
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels = ptr::null_mut();
        let bitmap = CreateDIBSection(
            ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut pixels,
            ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || pixels.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            return None;
        }
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            DeleteObject(bitmap);
            return None;
        }
        let previous = SelectObject(dc, bitmap);
        if previous.is_null() || previous as usize == usize::MAX {
            DeleteDC(dc);
            DeleteObject(bitmap);
            return None;
        }
        ptr::write_bytes(pixels.cast::<u8>(), 0, side * side * 4);
        Some(Self {
            dc,
            bitmap,
            previous,
            pixels: pixels.cast(),
        })
    }
}

impl Drop for GlyphBitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteDC(self.dc);
            DeleteObject(self.bitmap);
        }
    }
}

unsafe fn glyph_coverage(symbol: Symbol, size: i32, stroke: i32) -> Option<Vec<u8>> {
    if !(8..=256).contains(&size) {
        return None;
    }
    let side = size as usize * GLYPH_SAMPLES;
    let bitmap = GlyphBitmap::new(side)?;
    let mut paint = Paint::begin(bitmap.dc)?;
    let pen = paint.pen(stroke.max(1), 0x00ff_ffff)?;
    SelectObject(bitmap.dc, pen);
    SelectObject(bitmap.dc, GetStockObject(NULL_BRUSH));
    Canvas {
        dc: bitmap.dc,
        left: 0,
        top: 0,
        size: side as i32,
    }
    .draw(symbol);
    // GDI may batch DIB writes. Flush before reading its exclusively owned pixels.
    if GdiFlush() == 0 {
        return None;
    }
    let pixels = std::slice::from_raw_parts(bitmap.pixels, side * side * 4);
    let mut coverage = vec![0; size as usize * size as usize];
    for y in 0..size as usize {
        for x in 0..size as usize {
            let mut total = 0_u32;
            for sy in 0..GLYPH_SAMPLES {
                for sx in 0..GLYPH_SAMPLES {
                    total += u32::from(
                        pixels[((y * GLYPH_SAMPLES + sy) * side + x * GLYPH_SAMPLES + sx) * 4],
                    );
                }
            }
            coverage[y * size as usize + x] = ((total + 8) / 16) as u8;
        }
    }
    Some(coverage)
}

unsafe fn draw_coverage(
    dc: HDC,
    left: i32,
    top: i32,
    size: i32,
    coverage: &[u8],
    foreground: COLORREF,
    background: COLORREF,
) -> bool {
    let mut pixels = Vec::with_capacity(coverage.len() * 4);
    for &alpha in coverage {
        for shift in [16, 8, 0] {
            let front = (foreground >> shift) & 255;
            let back = (background >> shift) & 255;
            pixels.push(
                ((front * u32::from(alpha) + back * (255 - u32::from(alpha)) + 127) / 255) as u8,
            );
        }
        pixels.push(0);
    }
    let mut info: BITMAPINFO = zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = size;
    info.bmiHeader.biHeight = -size;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    SetDIBitsToDevice(
        dc,
        left,
        top,
        size as u32,
        size as u32,
        0,
        0,
        0,
        size as u32,
        pixels.as_ptr().cast(),
        &info,
        DIB_RGB_COLORS,
    ) != 0
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
                Symbol::Speak | Symbol::SpeakStop => {
                    self.line(&[(3, 9), (7, 9), (12, 5), (12, 19), (7, 15), (3, 15), (3, 9)]);
                    if symbol == Symbol::SpeakStop {
                        self.line(&[(16, 9), (21, 9), (21, 15), (16, 15), (16, 9)]);
                    } else {
                        self.curve(&[(15, 8), (18, 10), (18, 14), (15, 16)]);
                        self.curve(&[(18, 5), (23, 9), (23, 15), (18, 19)]);
                    }
                }
                Symbol::Polish => {
                    self.line(&[(4, 18), (7, 21), (20, 8), (17, 5), (4, 18)]);
                    self.line(&[(13, 9), (16, 12)]);
                    self.line(&[(6, 2), (6, 6)]);
                    self.line(&[(4, 4), (8, 4)]);
                    self.line(&[(20, 16), (20, 21)]);
                    self.line(&[(18, 19), (22, 19)]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curved_glyphs_have_subpixel_coverage_at_real_destination_densities() {
        for dpi in [96, 120, 144, 192, 288, 384] {
            let size = (20 * dpi + 48) / 96;
            let stroke = (2 * dpi * GLYPH_SAMPLES as i32 + 48) / 96;
            for symbol in [Symbol::Moon, Symbol::Eye, Symbol::Speak, Symbol::Polish] {
                let coverage = unsafe { glyph_coverage(symbol, size, stroke) }.unwrap();
                assert_eq!(coverage.len(), (size * size) as usize);
                assert!(coverage.contains(&255));
                assert!(coverage.iter().any(|&alpha| alpha > 0 && alpha < 255));
                assert_eq!(coverage[0], 0);
            }
        }
    }

    #[test]
    fn toolbar_actions_keep_distinct_visible_marks_without_a_glyph_font() {
        let mut masks = std::collections::HashSet::new();
        for symbol in [
            Symbol::Sun,
            Symbol::Moon,
            Symbol::Copy,
            Symbol::Connection,
            Symbol::Send,
            Symbol::Expand,
            Symbol::Settings,
            Symbol::Close,
            Symbol::Stop,
            Symbol::Retry,
            Symbol::Pin,
            Symbol::PinActive,
            Symbol::Swap,
            Symbol::Eye,
            Symbol::EyeClosed,
            Symbol::Speak,
            Symbol::SpeakStop,
            Symbol::Polish,
        ] {
            let coverage = unsafe { glyph_coverage(symbol, 24, 8) }.unwrap();
            assert!(
                coverage.iter().filter(|&&alpha| alpha > 0).count() > 24,
                "{symbol:?}"
            );
            assert!(masks.insert(coverage), "{symbol:?}");
        }
        assert!(unsafe { glyph_coverage(Symbol::Copy, i32::MAX, 8) }.is_none());
    }
}
