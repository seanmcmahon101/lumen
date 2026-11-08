//! Notification-area icon. Clicking it opens the search bar; its menu toggles
//! Start at login and quits Lumen.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetMessageW,
    GetSystemMetrics, HICON, ICONINFO, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, PostMessageW, RegisterClassW,
    RegisterWindowMessageW, SM_CXSMICON, SM_MENUDROPALIGNMENT, SetForegroundWindow, SetMenuDefaultItem, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_NULL,
    WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const WM_TRAY: u32 = WM_APP + 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const OPEN: usize = 1;
const STARTUP: usize = 2;
const QUIT: usize = 3;
const INK: [u8; 3] = [0x16, 0x18, 0x1C];

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static ICON: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// Adds the icon from its own thread, which owns the menu and its message loop.
pub fn spawn() {
    std::thread::spawn(|| unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let instance = HINSTANCE(module.0);
        let class = w!("Lumen.Tray");
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: class, ..Default::default() });
        // Explorer broadcasts this after it restarts, when every icon must be added again.
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        // A hidden top-level window: message-only windows miss that broadcast.
        let Ok(window) = CreateWindowExW(WS_EX_TOOLWINDOW, class, w!("Lumen"), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance), None) else {
            return;
        };
        WINDOW.store(window.0 as isize, Ordering::Relaxed);
        add(window);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

/// Removes the icon first so it does not linger in the notification area.
pub fn quit() -> ! {
    crate::storage::flush();
    let window = WINDOW.load(Ordering::Relaxed);
    if window != 0 {
        let data = icon_data(HWND(window as *mut c_void));
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }
    std::process::exit(0)
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: window, uID: 1, ..Default::default() }
}

pub fn refresh_tip() {
    let window = WINDOW.load(Ordering::Relaxed);
    if window == 0 { return; }
    let mut data = icon_data(HWND(window as *mut c_void)); data.uFlags = NIF_TIP;
    let tip: Vec<u16> = format!("Lumen · {} to search", crate::settings::get().hotkey.label()).encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    let _ = unsafe { Shell_NotifyIconW(windows::Win32::UI::Shell::NIM_MODIFY, &data) };
}

unsafe fn add(window: HWND) {
    let mut data = icon_data(window);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY;
    if ICON.load(Ordering::Relaxed) == 0
        && let Some(icon) = unsafe { create_icon() }
    {
        ICON.store(icon.0 as isize, Ordering::Relaxed);
    }
    data.hIcon = HICON(ICON.load(Ordering::Relaxed) as *mut c_void);
    let tip: Vec<u16> = format!("Lumen · {} to search", crate::settings::get().hotkey.label()).encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        // Version 4 reports keyboard selection and gives the menu's anchor point.
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_TRAY => match lparam.0 as u32 & 0xFFFF {
            NIN_SELECT | NIN_KEYSELECT => crate::win::open(),
            WM_CONTEXTMENU => unsafe { menu(window, wparam) },
            _ => {}
        },
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => unsafe { add(window) },
        _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
    LRESULT(0)
}

unsafe fn menu(window: HWND, anchor: WPARAM) {
    let x = (anchor.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((anchor.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let startup = if crate::win::startup_enabled() { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING, OPEN, w!("Open Lumen\tWin+Space"));
        let _ = AppendMenuW(menu, MF_STRING | startup, STARTUP, w!("Start at login"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, QUIT, w!("Quit Lumen"));
        let _ = SetMenuDefaultItem(menu, OPEN as u32, 0);
        // Without the foreground, the menu would stay open after a click elsewhere.
        let _ = SetForegroundWindow(window);
        let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
        let flags = align | TPM_BOTTOMALIGN | TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON;
        let chosen = TrackPopupMenu(menu, flags, x, y, None, window, None);
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match chosen.0 as usize {
            OPEN => crate::win::open(),
            STARTUP => crate::win::set_startup(!crate::win::startup_enabled()),
            QUIT => quit(),
            _ => {}
        }
    }
}

unsafe fn create_icon() -> Option<HICON> {
    unsafe {
        let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16);
        let pixels = draw_icon(size as usize);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        // The alpha channel does the masking; the mask itself only has to exist.
        let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color });
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon.ok()
    }
}

/// A white magnifier on a near-black rounded square, as straight-alpha BGRA.
fn draw_icon(size: usize) -> Vec<u8> {
    const SAMPLES: usize = 4;
    let (ring, radius, stroke) = ((0.44, 0.44), 0.2, 0.065);
    let handle = ((0.44 + 0.2 * std::f32::consts::FRAC_1_SQRT_2, 0.44 + 0.2 * std::f32::consts::FRAC_1_SQRT_2), (0.75, 0.75), 0.07);
    let mut out = Vec::with_capacity(size * size * 4);
    for py in 0..size {
        for px in 0..size {
            let (mut covered, mut white) = (0usize, 0usize);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    if !in_rounded_square(x, y, 0.03, 0.24) {
                        continue;
                    }
                    covered += 1;
                    let ring_distance = ((x - ring.0).hypot(y - ring.1) - radius).abs();
                    if ring_distance <= stroke || segment_distance((x, y), handle.0, handle.1) <= handle.2 {
                        white += 1;
                    }
                }
            }
            let mix = |ink: u8| ((ink as usize * (covered - white) + 255 * white) / covered.max(1)) as u8;
            out.extend_from_slice(&[mix(INK[2]), mix(INK[1]), mix(INK[0]), (covered * 255 / (SAMPLES * SAMPLES)) as u8]);
        }
    }
    out
}

fn in_rounded_square(x: f32, y: f32, inset: f32, radius: f32) -> bool {
    let (lo, hi) = (inset + radius, 1.0 - inset - radius);
    let dx = (lo - x).max(x - hi).max(0.0);
    let dy = (lo - y).max(y - hi).max(0.0);
    x >= inset && x <= 1.0 - inset && y >= inset && y <= 1.0 - inset && dx.hypot(dy) <= radius
}

fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / (abx * abx + aby * aby)).clamp(0.0, 1.0);
    (p.0 - a.0 - t * abx).hypot(p.1 - a.1 - t * aby)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_clear_corners_dark_tile_and_white_lens() {
        let size = 32;
        let pixels = draw_icon(size);
        let at = |x: usize, y: usize| &pixels[(y * size + x) * 4..(y * size + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corner is transparent");
        assert_eq!(at(14, 14), &[INK[2], INK[1], INK[0], 255], "inside the lens is the tile");
        let rim = (0.44 * size as f32) as usize + (0.2 * size as f32) as usize;
        assert!(at(rim, 14)[0] > 200, "the ring is white");
        assert_eq!(at(27, 6), &[INK[2], INK[1], INK[0], 255], "top-right is plain tile");
    }
}
