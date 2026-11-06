//! Window show/hide, launching, shell icons, history, run-at-login.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, GetObjectW, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, HWND_TOPMOST, SW_HIDE, SW_SHOWNORMAL,
    SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos, ShowWindowAsync,
};
use windows::core::{HSTRING, PCWSTR};

static HWND_VAL: AtomicIsize = AtomicIsize::new(0);
pub static VISIBLE: AtomicBool = AtomicBool::new(false);
pub static JUST_SHOWN: AtomicBool = AtomicBool::new(false);
pub static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

pub fn set_hwnd(h: isize) {
    HWND_VAL.store(h, Ordering::Relaxed);
}

fn hwnd() -> HWND {
    HWND(HWND_VAL.load(Ordering::Relaxed) as *mut c_void)
}

pub fn toggle() {
    if VISIBLE.load(Ordering::Relaxed) { hide() } else { show() }
}

/// Safe to call from any thread (the hook calls it).
pub fn show() {
    // Snapshot what's under the overlay first; drawn darkened as the backdrop.
    *BACKDROP.lock().unwrap() = unsafe { cursor_monitor() }.and_then(|r| unsafe { capture(r) });
    unsafe { fit_to_cursor_monitor(true) };
    VISIBLE.store(true, Ordering::Relaxed);
    JUST_SHOWN.store(true, Ordering::Relaxed);
    // Go through eframe so its own idea of visibility stays in sync.
    if let Some(ctx) = CTX.get() {
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
        ctx.request_repaint();
    }
}

pub fn hide() {
    VISIBLE.store(false, Ordering::Relaxed);
    if let Some(ctx) = CTX.get() {
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(false));
        ctx.request_repaint();
    }
    unsafe {
        let _ = ShowWindowAsync(hwnd(), SW_HIDE);
    }
}

/// Covers the whole monitor under the mouse. Called again on the UI thread in case a
/// DPI change between monitors resized the window.
unsafe fn cursor_monitor() -> Option<RECT> {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(mon, &mut mi).as_bool().then_some(mi.rcMonitor)
    }
}

pub unsafe fn fit_to_cursor_monitor(asynchronous: bool) -> Option<RECT> {
    unsafe {
        let r = cursor_monitor()?;
        let flags = if asynchronous {
            windows::Win32::UI::WindowsAndMessaging::SWP_ASYNCWINDOWPOS
        } else {
            SWP_SHOWWINDOW
        };
        let _ = SetWindowPos(hwnd(), Some(HWND_TOPMOST), r.left, r.top, r.right - r.left, r.bottom - r.top, flags);
        Some(r)
    }
}

pub static BACKDROP: Mutex<Option<eframe::egui::ColorImage>> = Mutex::new(None);

unsafe fn capture(r: RECT) -> Option<eframe::egui::ColorImage> {
    unsafe {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY).is_ok();
        SelectObject(mem, old);
        let mut bits = vec![0u8; (w * h * 4) as usize];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = if ok {
            GetDIBits(mem, bmp, 0, h as u32, Some(bits.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS)
        } else {
            0
        };
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if lines == 0 {
            return None;
        }
        for p in bits.chunks_exact_mut(4) {
            p.swap(0, 2);
            p[3] = 255;
        }
        Some(eframe::egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &bits))
    }
}

/// Windows only lets the foreground app hand over focus; borrowing its input queue gets around that.
pub fn force_foreground() {
    unsafe {
        let me = GetCurrentThreadId();
        let fg = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let attach = fg != 0 && fg != me;
        if attach {
            let _ = AttachThreadInput(me, fg, true);
        }
        let _ = SetForegroundWindow(hwnd());
        let _ = BringWindowToTop(hwnd());
        if attach {
            let _ = AttachThreadInput(me, fg, false);
        }
    }
}

pub fn shell_open(target: &str, verb: &str, params: Option<&str>) -> bool {
    let verb = HSTRING::from(verb);
    let target = HSTRING::from(target);
    let params = params.map(HSTRING::from);
    let params = params.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr()));
    let r = unsafe { ShellExecuteW(None, &verb, &target, params, PCWSTR::null(), SW_SHOWNORMAL) };
    r.0 as isize > 32
}

pub fn reveal(path: &str) {
    shell_open("explorer.exe", "open", Some(&format!("/select,\"{path}\"")));
}

pub fn copy(text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

/// Shell icon for a file, folder or shell:AppsFolder item, as straight RGBA.
pub fn load_icon(path: &str) -> Option<eframe::egui::ColorImage> {
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>).ok()?;
        let hbm = factory.GetImage(SIZE { cx: 64, cy: 64 }, SIIGBF_ICONONLY).ok()?;
        let mut bm = BITMAP::default();
        let got = GetObjectW(HGDIOBJ(hbm.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void));
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut bits = vec![0u8; (w.max(0) * h.max(0) * 4) as usize];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let hdc = GetDC(None);
        let lines = if got != 0 && !bits.is_empty() {
            GetDIBits(hdc, hbm, 0, h as u32, Some(bits.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS)
        } else {
            0
        };
        ReleaseDC(None, hdc);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        if lines == 0 {
            return None;
        }
        // Old-style icons have no alpha at all: treat as opaque.
        let has_alpha = bits.chunks_exact(4).any(|p| p[3] != 0);
        let mut rgba = Vec::with_capacity(bits.len());
        for p in bits.chunks_exact(4) {
            rgba.extend_from_slice(&[p[2], p[1], p[0], if has_alpha { p[3] } else { 255 }]);
        }
        Some(eframe::egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba))
    }
}

/// Launch counts, so things you open often rank higher.
#[derive(Clone)]
pub struct History {
    counts: HashMap<String, u32>,
}

impl History {
    fn file() -> Option<std::path::PathBuf> {
        std::env::var("APPDATA").ok().map(|p| std::path::Path::new(&p).join("Lumen").join("history.json"))
    }

    pub fn load() -> Self {
        let counts = Self::file()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { counts }
    }

    pub fn uses(&self, key: &str) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
    }

    pub fn bump(&mut self, key: &str) {
        *self.counts.entry(key.to_string()).or_default() += 1;
        if let Some(f) = Self::file() {
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            if let Ok(json) = serde_json::to_vec(&self.counts) {
                let _ = std::fs::write(f, json);
            }
        }
    }
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg.exe")
        .args(args)
        .creation_flags(0x0800_0000)
        .output()
        .is_ok_and(|o| o.status.success())
}

pub fn startup_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", "Lumen"])
}

pub fn set_startup(on: bool) {
    if on {
        let exe = std::env::current_exe().unwrap_or_default();
        let value = format!("\"{}\"", exe.display());
        reg(&["add", RUN_KEY, "/v", "Lumen", "/t", "REG_SZ", "/d", &value, "/f"]);
    } else {
        reg(&["delete", RUN_KEY, "/v", "Lumen", "/f"]);
    }
}
