//! Window show/hide, launching, shell icons, history, run-at-login.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, GetObjectW, GetStockObject, HBRUSH, HDC, HGDIOBJ,
    HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
    GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, LWA_ALPHA, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetForegroundWindow, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, ShowWindowAsync, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

static HWND_VAL: AtomicIsize = AtomicIsize::new(0);
static TARGET_MONITOR: AtomicIsize = AtomicIsize::new(0);
pub static VISIBLE: AtomicBool = AtomicBool::new(false);
pub static JUST_SHOWN: AtomicBool = AtomicBool::new(false);
pub static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();
pub const BACKDROP_BRIGHTNESS: u8 = 76;

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
    // Resolve the active app before Lumen takes focus. Leave capture and window
    // creation to the UI thread so they cannot delay the keyboard hook.
    let monitor = unsafe { active_monitor() };
    TARGET_MONITOR.store(monitor.0 as isize, Ordering::Relaxed);
    VISIBLE.store(true, Ordering::Relaxed);
    JUST_SHOWN.store(true, Ordering::Release);
    if let Some(ctx) = CTX.get() {
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

unsafe fn active_monitor() -> HMONITOR {
    unsafe {
        let foreground = GetForegroundWindow();
        if !foreground.is_invalid() && foreground != GetDesktopWindow() && foreground != GetShellWindow() {
            return MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
    }
}

fn monitor_rect(monitor: HMONITOR) -> Option<RECT> {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool().then_some(info.rcMonitor)
}

/// Owned by the UI thread, which creates and destroys the native dimmers.
#[derive(Default)]
pub struct DisplayOverlay {
    monitor: Option<RECT>,
    dimmers: Vec<HWND>,
}

impl DisplayOverlay {
    pub fn open(&mut self) -> Option<eframe::egui::ColorImage> {
        self.close();
        let selected = HMONITOR(TARGET_MONITOR.load(Ordering::Relaxed) as *mut c_void);
        let monitor = monitor_rect(selected).or_else(|| monitor_rect(unsafe { active_monitor() }))?;
        let backdrop = unsafe { capture(monitor) };
        self.monitor = Some(monitor);
        self.fit();

        let mut others = Vec::<RECT>::new();
        let _ = unsafe { EnumDisplayMonitors(None, None, Some(collect_monitor), LPARAM(&mut others as *mut Vec<RECT> as isize)) };
        for rect in others.into_iter().filter(|rect| *rect != monitor) {
            if let Some(window) = dim_monitor(rect) {
                self.dimmers.push(window);
            }
        }
        backdrop
    }

    pub fn fit(&self) {
        let Some(r) = self.monitor else { return };
        unsafe {
            let mut current = RECT::default();
            if GetWindowRect(hwnd(), &mut current).is_err() || current != r {
                let _ = SetWindowPos(hwnd(), Some(HWND_TOPMOST), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE);
            }
        }
    }

    pub fn close(&mut self) {
        self.monitor = None;
        for window in self.dimmers.drain(..) {
            let _ = unsafe { DestroyWindow(window) };
        }
    }
}

impl Drop for DisplayOverlay {
    fn drop(&mut self) {
        self.close();
    }
}

unsafe extern "system" fn collect_monitor(_: HMONITOR, _: HDC, rect: *mut RECT, data: LPARAM) -> BOOL {
    // EnumDisplayMonitors calls back synchronously with the live vector from open().
    let monitors = unsafe { &mut *(data.0 as *mut Vec<RECT>) };
    let rect = unsafe { *rect };
    if !monitors.contains(&rect) {
        monitors.push(rect);
    }
    true.into()
}

unsafe extern "system" fn dimmer_window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn dim_monitor(r: RECT) -> Option<HWND> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let class = w!("Lumen.MonitorDimmer");
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
        if !REGISTERED.get_or_init(|| {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(dimmer_window_proc),
                hInstance: instance,
                lpszClassName: class,
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            }) != 0
        }) {
            return None;
        }

        let window = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class,
            PCWSTR::null(),
            WS_POPUP,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            Some(instance),
            None,
        )
        .ok()?;
        if SetLayeredWindowAttributes(window, COLORREF(0), 255 - BACKDROP_BRIGHTNESS, LWA_ALPHA).is_err()
            || SetWindowPos(window, Some(HWND_TOPMOST), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE | SWP_SHOWWINDOW)
                .is_err()
        {
            let _ = DestroyWindow(window);
            return None;
        }
        Some(window)
    }
}

pub fn show_native() {
    unsafe {
        let _ = ShowWindow(hwnd(), SW_SHOWNOACTIVATE);
    }
}

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
        let lines =
            if ok { GetDIBits(mem, bmp, 0, h as u32, Some(bits.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS) } else { 0 };
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

/// Launch counts, so things you open often rank higher, and the result last
/// opened for each typed query, so that query brings it back first.
#[derive(Clone)]
pub struct History {
    counts: HashMap<String, u32>,
    picks: HashMap<String, String>, // normalized typed query -> path
}

impl History {
    fn file(name: &str) -> Option<std::path::PathBuf> {
        std::env::var("APPDATA").ok().map(|p| std::path::Path::new(&p).join("Lumen").join(name))
    }

    fn write(name: &str, json: serde_json::Result<Vec<u8>>) {
        if let (Some(f), Ok(json)) = (Self::file(name), json) {
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            let _ = std::fs::write(f, json);
        }
    }

    pub fn load() -> Self {
        let read = |name: &str| Self::file(name).and_then(|f| std::fs::read(f).ok());
        let counts = read("history.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let picks = read("picks.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { counts, picks }
    }

    pub fn uses(&self, key: &str) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
    }

    pub fn bump(&mut self, key: &str) {
        *self.counts.entry(key.to_string()).or_default() += 1;
        Self::write("history.json", serde_json::to_vec(&self.counts));
    }

    /// The path last opened after typing `query`.
    pub fn pick(&self, query: &str) -> Option<&str> {
        self.picks.get(&normalize(query)).map(String::as_str)
    }

    /// Remember `path` for `query` and for each shorter prefix of it.
    pub fn learn(&mut self, query: &str, path: &str) {
        for key in pick_keys(query) {
            self.picks.insert(key, path.to_string());
        }
        Self::write("picks.json", serde_json::to_vec(&self.picks));
    }
}

fn normalize(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Every prefix of the search term from two characters up, keeping any filter
/// words in front: "excel bud" gives "excel bu" and "excel bud".
fn pick_keys(query: &str) -> Vec<String> {
    let (filter, term) = crate::index::split_filter(query);
    let ends: Vec<usize> = term.char_indices().map(|(i, c)| i + c.len_utf8()).skip(1).take(39).collect();
    let mut keys: Vec<String> = ends.into_iter().map(|end| normalize(&format!("{filter}{}", &term[..end]))).collect();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_learn_prefixes_behind_filters() {
        assert_eq!(pick_keys("jd"), ["jd"]);
        assert_eq!(pick_keys("Excel  Bud"), ["excel bu", "excel bud"]);
        assert_eq!(pick_keys("j"), Vec::<String>::new());
        assert_eq!(pick_keys("mail "), Vec::<String>::new());
        let mut history = History { counts: HashMap::new(), picks: HashMap::new() };
        for key in pick_keys("john d") {
            history.picks.insert(key, "C:\\jd.xlsx".into());
        }
        assert_eq!(history.pick("JO"), Some("C:\\jd.xlsx"));
        assert_eq!(history.pick("john  d"), Some("C:\\jd.xlsx"));
        assert_eq!(history.pick("excel jo"), None);
    }
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg.exe").args(args).creation_flags(0x0800_0000).output().is_ok_and(|o| o.status.success())
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
