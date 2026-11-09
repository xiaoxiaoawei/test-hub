//! Per-pixel alpha presentation for the selection icon. No frame or shadow is
//! painted into this surface; its bounds are exactly the icon's pixel bounds.

use super::icons::IconPixels;
use std::{ffi::c_void, mem::size_of, mem::zeroed, ptr};
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, POINT, SIZE},
    Graphics::{Dwm::*, Gdi::*},
    UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA},
};

struct ScreenDc(HDC);

impl ScreenDc {
    unsafe fn new() -> Result<Self, String> {
        let dc = GetDC(ptr::null_mut());
        if dc.is_null() {
            Err("无法取得显示设备，请稍后重试。".into())
        } else {
            Ok(Self(dc))
        }
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(ptr::null_mut(), self.0) };
    }
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Surface {
    unsafe fn new(screen: HDC, pixels: &IconPixels) -> Result<Self, String> {
        checked_length(pixels.width, pixels.height, pixels.pixels.len())?;
        let dc = CreateCompatibleDC(screen);
        if dc.is_null() {
            return Err("无法创建图标绘制设备。".into());
        }
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = pixels.width;
        info.bmiHeader.biHeight = -pixels.height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut data: *mut c_void = ptr::null_mut();
        let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut data, ptr::null_mut(), 0);
        if bitmap.is_null() || data.is_null() {
            DeleteDC(dc);
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            return Err("无法创建透明图标位图。".into());
        }
        let previous = SelectObject(dc, bitmap);
        if previous.is_null() || previous as isize == -1 {
            DeleteDC(dc);
            DeleteObject(bitmap);
            return Err("无法准备透明图标绘制。".into());
        }
        ptr::copy_nonoverlapping(
            pixels.pixels.as_ptr(),
            data.cast::<u8>(),
            pixels.pixels.len(),
        );
        Ok(Self {
            dc,
            bitmap,
            previous,
        })
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

fn checked_length(width: i32, height: i32, actual: usize) -> Result<(), String> {
    if !(1..=1024).contains(&width) || !(1..=1024).contains(&height) {
        return Err("透明图标尺寸无效。".into());
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|size| size.checked_mul(4));
    if expected != Some(actual) {
        return Err("透明图标像素格式无效。".into());
    }
    Ok(())
}

/// Atomically replace a layered popup's position, dimensions and alpha pixels.
/// This neither shows nor activates the window, and leaves no extra background.
///
/// # Safety
/// `hwnd` must be a live `WS_EX_LAYERED` window belonging to this UI thread.
/// `pixels` must contain premultiplied, top-down BGRA produced by `render_pixels`.
/// Do not combine this path with `SetLayeredWindowAttributes` on the same popup.
pub(crate) unsafe fn update(hwnd: HWND, x: i32, y: i32, pixels: &IconPixels) -> Result<(), String> {
    if hwnd.is_null() {
        return Err("划词图标窗口不可用。".into());
    }
    let screen = ScreenDc::new()?;
    let surface = Surface::new(screen.0, pixels)?;
    let destination = POINT { x, y };
    let source = POINT { x: 0, y: 0 };
    let size = SIZE {
        cx: pixels.width,
        cy: pixels.height,
    };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    if UpdateLayeredWindow(
        hwnd,
        screen.0,
        &destination,
        &size,
        surface.dc,
        &source,
        0,
        &blend,
        ULW_ALPHA,
    ) == 0
    {
        let code = GetLastError();
        crate::diagnostics::record_error(
            crate::diagnostics::Stage::Render,
            crate::diagnostics::Outcome::Failed,
            code,
        );
        return Err(format!("透明划词图标更新失败（系统错误 {code}）。"));
    }
    Ok(())
}

/// Disable non-client decorations using documented, optional DWM attributes.
/// The caller also supplies a borderless popup class without `CS_DROPSHADOW`.
///
/// # Safety
/// `hwnd` must be a live application-owned window on the current UI thread.
pub(crate) unsafe fn disable_shadow(hwnd: HWND) {
    let policy = DWMNCRP_DISABLED;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_NCRENDERING_POLICY as u32,
        (&policy as *const DWMNCRENDERINGPOLICY).cast(),
        size_of::<DWMNCRENDERINGPOLICY>() as u32,
    );
    let corners = DWMWCP_DONOTROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        (&corners as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
        size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
    );
}

#[cfg(test)]
mod tests {
    use super::checked_length;

    #[test]
    fn transparent_surface_requires_exact_bgra_rows_and_bounded_dimensions() {
        assert!(checked_length(32, 32, 32 * 32 * 4).is_ok());
        assert!(checked_length(1, 64, 64 * 4).is_ok());
        for (width, height, bytes) in [
            (0, 32, 0),
            (32, -1, 0),
            (1025, 1, 4100),
            (32, 32, 4095),
            (32, 32, 4097),
        ] {
            assert!(checked_length(width, height, bytes).is_err());
        }
    }
}
