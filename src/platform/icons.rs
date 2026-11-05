//! Small runtime icons rendered with native GDI and a premultiplied alpha mask.
//! The executable's on-disk resource icon is deliberately left untouched.

use crate::config::IconStyle;
use std::{ffi::c_void, mem::zeroed, ptr};
use windows_sys::Win32::{
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO},
};

const SUPERSAMPLE: usize = 4;

/// Owns the handles used by tray and window icons. Keep this value alive until
/// those windows have received their replacement handles.
pub(crate) struct IconSet {
    small: OwnedIcon,
    large: OwnedIcon,
}

impl IconSet {
    pub(crate) fn new(style: IconStyle, color: u32, dpi: u32) -> Result<Self, String> {
        if color > 0xFF_FF_FF {
            return Err("图标颜色必须是有效的 RGB 颜色。".into());
        }
        // Bound work and memory even if a caller supplies a malformed DPI.
        let dpi = dpi.clamp(96, 384);
        let small = render_icon(style, color, scale(16, dpi))?;
        let large = render_icon(style, color, scale(32, dpi))?;
        Ok(Self { small, large })
    }

    pub(crate) fn small(&self) -> HICON {
        self.small.0
    }

    pub(crate) fn large(&self) -> HICON {
        self.large.0
    }
}

struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe { DestroyIcon(self.0) };
    }
}

struct Bitmap {
    handle: HBITMAP,
    pixels: *mut u8,
    side: usize,
}

impl Bitmap {
    fn new(side: usize) -> Result<Self, String> {
        let mut info: BITMAPINFO = unsafe { zeroed() };
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = side as i32;
        info.bmiHeader.biHeight = -(side as i32); // top-down BGRA
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels: *mut c_void = ptr::null_mut();
        let handle = unsafe {
            CreateDIBSection(
                ptr::null_mut(),
                &info,
                DIB_RGB_COLORS,
                &mut pixels,
                ptr::null_mut(),
                0,
            )
        };
        if handle.is_null() || pixels.is_null() {
            if !handle.is_null() {
                unsafe { DeleteObject(handle) };
            }
            return Err("无法创建图标位图，请稍后重试。".into());
        }
        unsafe { ptr::write_bytes(pixels.cast::<u8>(), 0, side * side * 4) };
        Ok(Self {
            handle,
            pixels: pixels.cast(),
            side,
        })
    }

    fn bytes(&self) -> &[u8] {
        // The exclusively owned DIB remains alive and no GDI command is pending
        // whenever this view is requested.
        unsafe { std::slice::from_raw_parts(self.pixels, self.side * self.side * 4) }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.handle) };
    }
}

struct MemoryDc {
    handle: HDC,
    previous: HGDIOBJ,
}

impl MemoryDc {
    fn selecting(bitmap: &Bitmap) -> Result<Self, String> {
        let handle = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        if handle.is_null() {
            return Err("无法创建图标绘制上下文。".into());
        }
        let previous = unsafe { SelectObject(handle, bitmap.handle) };
        if previous.is_null() || previous as usize == usize::MAX {
            unsafe { DeleteDC(handle) };
            return Err("无法准备图标绘制。".into());
        }
        Ok(Self { handle, previous })
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.handle, self.previous);
            DeleteDC(self.handle);
        }
    }
}

fn scale(logical: u32, dpi: u32) -> usize {
    ((logical * dpi + 48) / 96) as usize
}

fn render_icon(style: IconStyle, color: u32, size: usize) -> Result<OwnedIcon, String> {
    let pixels = render_pixels(style, color, size)?;
    let bitmap = Bitmap::new(size)?;
    unsafe { ptr::copy_nonoverlapping(pixels.as_ptr(), bitmap.pixels, pixels.len()) };

    // A monochrome DDB scanline is WORD-aligned. The AND mask retains the
    // transparent corners for callers that consult it in addition to alpha.
    let mask_stride = size.div_ceil(16) * 2;
    let mut and_mask = vec![0_u8; mask_stride * size];
    for y in 0..size {
        for x in 0..size {
            if pixels[(y * size + x) * 4 + 3] == 0 {
                and_mask[y * mask_stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mask = unsafe { CreateBitmap(size as i32, size as i32, 1, 1, and_mask.as_ptr().cast()) };
    if mask.is_null() {
        return Err("无法创建图标透明遮罩。".into());
    }
    let info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: bitmap.handle,
    };
    // CreateIconIndirect copies both bitmaps; neither is retained by HICON.
    let icon = unsafe { CreateIconIndirect(&info) };
    unsafe { DeleteObject(mask) };
    if icon.is_null() {
        return Err("无法创建自定义图标。".into());
    }
    Ok(OwnedIcon(icon))
}

fn render_pixels(style: IconStyle, color: u32, size: usize) -> Result<Vec<u8>, String> {
    let side = size * SUPERSAMPLE;
    let glyph = match style {
        IconStyle::Leaf => leaf_mask(side),
        IconStyle::Translate => text_mask("译", side)?,
        IconStyle::Letter => text_mask("A", side)?,
    };
    let background = [
        (color & 0xff) as f64,
        ((color >> 8) & 0xff) as f64,
        ((color >> 16) & 0xff) as f64,
    ]; // BGR
    let dark_ink = [27.0, 29.0, 33.0];
    let light_ink = [232.0, 242.0, 249.0];
    let luminance = relative_luminance(background);
    let foreground = if contrast(luminance, relative_luminance(dark_ink))
        >= contrast(luminance, relative_luminance(light_ink))
    {
        dark_ink
    } else {
        light_ink
    };
    let mut output = vec![0_u8; size * size * 4];
    let count = (SUPERSAMPLE * SUPERSAMPLE) as f64;
    for y in 0..size {
        for x in 0..size {
            let mut sum = [0.0; 4];
            for dy in 0..SUPERSAMPLE {
                for dx in 0..SUPERSAMPLE {
                    let sx = x * SUPERSAMPLE + dx;
                    let sy = y * SUPERSAMPLE + dy;
                    if !rounded_background(sx, sy, side) {
                        continue;
                    }
                    let ink = f64::from(glyph[sy * side + sx]) / 255.0;
                    for channel in 0..3 {
                        sum[channel] +=
                            background[channel] * (1.0 - ink) + foreground[channel] * ink;
                    }
                    sum[3] += 255.0;
                }
            }
            let start = (y * size + x) * 4;
            for channel in 0..4 {
                output[start + channel] = (sum[channel] / count).round() as u8;
            }
        }
    }
    Ok(output)
}

fn relative_luminance(bgr: [f64; 3]) -> f64 {
    let linear = bgr.map(|channel| {
        let channel = channel / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    linear[2] * 0.2126 + linear[1] * 0.7152 + linear[0] * 0.0722
}

fn contrast(left: f64, right: f64) -> f64 {
    (left.max(right) + 0.05) / (left.min(right) + 0.05)
}

fn rounded_background(x: usize, y: usize, side: usize) -> bool {
    let x = (x as f64 + 0.5) / side as f64;
    let y = (y as f64 + 0.5) / side as f64;
    let inset = 0.045;
    let radius = 0.19;
    let nearest_x = x.clamp(inset + radius, 1.0 - inset - radius);
    let nearest_y = y.clamp(inset + radius, 1.0 - inset - radius);
    (x - nearest_x).powi(2) + (y - nearest_y).powi(2) <= radius * radius
}

fn leaf_mask(side: usize) -> Vec<u8> {
    let mut mask = vec![0; side * side];
    for y in 0..side {
        for x in 0..side {
            let dx = (x as f64 + 0.5) / side as f64 * 2.0 - 1.0;
            let dy = (y as f64 + 0.5) / side as f64 * 2.0 - 1.0;
            let u = (dx - dy) * std::f64::consts::FRAC_1_SQRT_2;
            let v = (dx + dy) * std::f64::consts::FRAC_1_SQRT_2;
            let on_leaf = (-0.55..=0.55).contains(&u)
                && v.abs() <= 0.30 * ((u + 0.55) * std::f64::consts::PI / 1.1).sin().powf(0.7);
            let vein = (-0.47..0.38).contains(&u) && v.abs() < 0.023;
            let stem = (-0.68..-0.35).contains(&u) && v.abs() < 0.027;
            if (on_leaf && !vein) || stem {
                mask[y * side + x] = 255;
            }
        }
    }
    mask
}

fn text_mask(text: &str, side: usize) -> Result<Vec<u8>, String> {
    let bitmap = Bitmap::new(side)?;
    let dc = MemoryDc::selecting(&bitmap)?;
    let font_name: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    let font = unsafe {
        CreateFontW(
            -((side as f64 * 0.72).round() as i32),
            0,
            0,
            0,
            FW_SEMIBOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            font_name.as_ptr(),
        )
    };
    if font.is_null() {
        return Err("无法创建图标字体。".into());
    }
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let previous = unsafe { SelectObject(dc.handle, font) };
    if previous.is_null() || previous as usize == usize::MAX {
        unsafe { DeleteObject(font) };
        return Err("无法选择图标字体。".into());
    }
    let mut rect = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: side as i32,
        bottom: side as i32,
    };
    let drawn = unsafe {
        SetTextColor(dc.handle, 0x00ff_ffff);
        SetBkMode(dc.handle, TRANSPARENT as i32);
        let drawn = DrawTextW(
            dc.handle,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc.handle, previous);
        DeleteObject(font);
        GdiFlush();
        drawn
    };
    if drawn == 0 {
        return Err("无法绘制图标文字。".into());
    }
    let coverage = bitmap
        .bytes()
        .chunks_exact(4)
        .map(|pixel| pixel[0].max(pixel[1]).max(pixel[2]))
        .collect();
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterized_icons_have_transparent_corners_and_premultiplied_edges() {
        for style in [IconStyle::Leaf, IconStyle::Translate, IconStyle::Letter] {
            let pixels = render_pixels(style, 0xD6_A5_75, 32).unwrap();
            assert_eq!(&pixels[..4], &[0, 0, 0, 0]);
            assert_eq!(pixels[(16 * 32 + 16) * 4 + 3], 255);
            assert!(pixels
                .chunks_exact(4)
                .any(|pixel| (1..255).contains(&pixel[3])));
            for pixel in pixels.chunks_exact(4) {
                assert!(pixel[..3].iter().all(|&channel| channel <= pixel[3]));
            }
        }
    }

    #[test]
    fn styles_colors_and_native_handles_are_distinct_and_valid() {
        assert_ne!(
            render_pixels(IconStyle::Leaf, 0xD6_A5_75, 32).unwrap(),
            render_pixels(IconStyle::Letter, 0xD6_A5_75, 32).unwrap()
        );
        assert_ne!(
            render_pixels(IconStyle::Leaf, 0xD6_A5_75, 32).unwrap(),
            render_pixels(IconStyle::Leaf, 0x35_68_90, 32).unwrap()
        );
        for style in [IconStyle::Leaf, IconStyle::Translate, IconStyle::Letter] {
            let icons = IconSet::new(style, 0x12_34_56, 144).unwrap();
            assert!(!icons.small().is_null());
            assert!(!icons.large().is_null());
            assert_ne!(icons.small(), icons.large());
        }
        assert!(IconSet::new(IconStyle::Leaf, 0x0100_0000, 96).is_err());
    }
}
