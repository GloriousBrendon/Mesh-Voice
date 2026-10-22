//! Windows: paints the native title bar (and the window border) in the theme's
//! colors through DWM, so the minimize/maximize/close strip matches Mesh
//! instead of the system's white. Snap layouts and the native buttons keep
//! working. Windows 10 only takes the dark-mode switch; elsewhere a no-op.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::theme::Palette;

/// The main window's HWND, once iced has told us.
static WINDOW: AtomicU64 = AtomicU64::new(0);

pub fn set_window(raw_id: u64) {
    WINDOW.store(raw_id, Ordering::Relaxed);
}

/// `0x00BBGGRR`, DWM's color layout.
fn colorref(c: iced::Color) -> u32 {
    let [r, g, b, _] = c.into_rgba8();
    u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16
}

pub fn paint(p: &Palette) {
    let hwnd = WINDOW.load(Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    let bg = p.background();
    let dark = u32::from(0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b < 0.5);
    let attrs = [(20, dark), (35, colorref(bg)), (36, colorref(p.text_bright())), (34, colorref(p.border()))];
    #[cfg(windows)]
    for (attr, value) in attrs {
        // DWMWA_USE_IMMERSIVE_DARK_MODE, CAPTION_COLOR, TEXT_COLOR, BORDER_COLOR.
        unsafe {
            windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd as _,
                attr,
                std::ptr::from_ref(&value).cast(),
                size_of::<u32>() as u32,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = attrs;
}

#[cfg(test)]
mod tests {
    #[test]
    fn colorref_is_bgr() {
        assert_eq!(super::colorref(iced::Color::from_rgb8(0x11, 0x22, 0x33)), 0x0033_2211);
    }
}
