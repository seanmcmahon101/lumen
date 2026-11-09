//! Window show/hide, launching, shell icons, history, run-at-login.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{
    CloseHandle, COLORREF, ERROR_SUCCESS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, GetObjectW, GetStockObject, HBRUSH, HDC, HGDIOBJ,
    HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateEventW, EVENT_MODIFY_STATE, GetCurrentThreadId, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
    GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, LWA_ALPHA, RegisterClassW, SPI_GETCLIENTAREAANIMATION, SW_HIDE, SW_SHOWNOACTIVATE,
    SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetForegroundWindow, SetLayeredWindowAttributes,
    SetWindowPos, ShowWindow, ShowWindowAsync, SystemParametersInfoW, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

static HWND_VAL: AtomicIsize = AtomicIsize::new(0);
static TARGET_MONITOR: AtomicIsize = AtomicIsize::new(0);
pub static VISIBLE: AtomicBool = AtomicBool::new(false);
pub static JUST_SHOWN: AtomicBool = AtomicBool::new(false);
pub static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

/// Whether Windows apps are in light mode (Settings › Personalization › Colors).
pub fn apps_use_light_theme() -> bool {
    let mut value = 1u32;
    let mut bytes = std::mem::size_of::<u32>() as u32;
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut bytes),
        );
    }
    value != 0
}

/// The accent color ramp Windows derives from the user's accent, lightest first:
/// Light 3, 2, 1, the accent, Dark 1, 2, 3.
pub fn accent_ramp() -> Option<[[u8; 3]; 7]> {
    let mut palette = [0u8; 32];
    let mut bytes = palette.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(palette.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    (status == ERROR_SUCCESS && bytes >= 28).then(|| std::array::from_fn(|i| [palette[i * 4], palette[i * 4 + 1], palette[i * 4 + 2]]))
}

/// Windows' "Animation effects" setting, which also asks apps for less motion.
pub fn animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut BOOL as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    enabled.as_bool()
}

pub fn set_hwnd(h: isize) {
    HWND_VAL.store(h, Ordering::Relaxed);
}

fn hwnd() -> HWND {
    HWND(HWND_VAL.load(Ordering::Relaxed) as *mut c_void)
}

pub fn toggle() {
    if VISIBLE.load(Ordering::Relaxed) { hide() } else { show() }
}

/// Shows the search bar unless it is already open.
pub fn open() {
    if !VISIBLE.load(Ordering::Relaxed) {
        show();
    }
}

const SHOW_EVENT: PCWSTR = w!("Local\\Lumen.Show");

/// Opens the search bar whenever another `lumen.exe` starts, such as from the Start menu.
pub fn listen_for_relaunch() {
    let Ok(event) = (unsafe { CreateEventW(None, false, false, SHOW_EVENT) }) else { return };
    let event = event.0 as isize; // a raw HANDLE cannot cross threads
    std::thread::spawn(move || {
        while unsafe { WaitForSingleObject(HANDLE(event as *mut c_void), INFINITE) } == WAIT_OBJECT_0 {
            open();
        }
    });
}

/// Asks the Lumen already running to open, and lets it take the foreground from us.
pub fn signal_running() {
    unsafe {
        if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, SHOW_EVENT) {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = SetEvent(event);
            let _ = CloseHandle(event);
        }
    }
}

/// Safe to call from any thread (the hook calls it).
pub fn show() {
    crate::metrics::opening();
    // Resolve the active app before Lumen takes focus. Leave capture and window
    // creation to the UI thread so they cannot delay the keyboard hook.
    let monitor = unsafe { active_monitor() };
    TARGET_MONITOR.store(monitor.0 as isize, Ordering::Relaxed);
    JUST_SHOWN.store(true, Ordering::Release);
    VISIBLE.store(true, Ordering::Release);
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
    /// Captures the active display and dims the others to `brightness` (255 is undimmed).
    pub fn open(&mut self, brightness: u8) -> Option<eframe::egui::ColorImage> {
        self.close();
        let selected = HMONITOR(TARGET_MONITOR.load(Ordering::Relaxed) as *mut c_void);
        let monitor = monitor_rect(selected).or_else(|| monitor_rect(unsafe { active_monitor() }))?;
        let backdrop = unsafe { capture(monitor) };
        self.monitor = Some(monitor);
        self.fit();

        let mut others = Vec::<RECT>::new();
        let _ = unsafe { EnumDisplayMonitors(None, None, Some(collect_monitor), LPARAM(&mut others as *mut Vec<RECT> as isize)) };
        for rect in others.into_iter().filter(|rect| *rect != monitor) {
            if let Some(window) = dim_monitor(rect, brightness) {
                self.dimmers.push(window);
            }
        }
        backdrop
    }

    /// Dims the other displays to `brightness`, as the room dims when Lumen opens.
    pub fn dim(&self, brightness: u8) {
        for window in &self.dimmers {
            let _ = unsafe { SetLayeredWindowAttributes(*window, COLORREF(0), 255 - brightness, LWA_ALPHA) };
        }
    }

    /// Covers the display plus one row below it. Windows presents a borderless OpenGL
    /// window that exactly covers a monitor as full-screen, and after that window is
    /// resized the screen can keep showing its first frame. The extra row is off-screen.
    pub fn fit(&self) {
        let Some(r) = self.monitor else { return };
        let target = RECT { bottom: r.bottom + 1, ..r };
        unsafe {
            let mut current = RECT::default();
            if GetWindowRect(hwnd(), &mut current).is_err() || current != target {
                let (w, h) = (target.right - target.left, target.bottom - target.top);
                let _ = SetWindowPos(hwnd(), Some(HWND_TOPMOST), target.left, target.top, w, h, SWP_NOACTIVATE);
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

fn dim_monitor(r: RECT, brightness: u8) -> Option<HWND> {
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
        if SetLayeredWindowAttributes(window, COLORREF(0), 255 - brightness, LWA_ALPHA).is_err()
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
        for p in bits.as_chunks_mut::<4>().0 {
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

/// Clipboard text, or the paths of files and folders copied in File Explorer,
/// which put no text on the clipboard.
pub fn paste_text() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if let Ok(text) = cb.get_text()
        && !text.is_empty()
    {
        return Some(text.replace("\r\n", "\n"));
    }
    let files = cb.get().file_list().ok()?;
    let paths: Vec<_> = files.iter().map(|p| p.to_string_lossy()).collect();
    Some(paths.join("\n")).filter(|p| !p.is_empty())
}

/// The Windows folder picker. Blocks until it closes, so call it off the UI thread.
pub fn pick_folder() -> Option<String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::{FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog.SetOptions(dialog.GetOptions().ok()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        dialog.SetTitle(w!("Choose a folder for Lumen to search")).ok()?;
        dialog.Show(None).ok()?; // an error when the picker is cancelled
        let name = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const c_void));
        path
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
        let has_alpha = bits.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
        let mut rgba = Vec::with_capacity(bits.len());
        for p in bits.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[p[2], p[1], p[0], if has_alpha { p[3] } else { 255 }]);
        }
        Some(eframe::egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba))
    }
}

/// Launch counts, so things you open often rank higher, and the result last
/// opened for each typed query, so that query brings it back first.
#[derive(Clone, Default)]
pub struct History {
    counts: HashMap<String, u32>,
    picks: HashMap<String, String>, // normalized typed query -> path
}

impl History {
    fn file(name: &str) -> Option<std::path::PathBuf> {
        crate::storage::directory().map(|p| p.join(name))
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
        crate::storage::save("history.json", &self.counts);
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
        crate::storage::save("picks.json", &self.picks);
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

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

/// The command Windows runs for Lumen at login, if any.
fn startup_command() -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Lumen"), RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut bytes))
    };
    // The size includes the terminating null.
    (status == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buffer[..(bytes as usize / 2).saturating_sub(1)]))
}

fn write_startup(command: Option<&str>) {
    unsafe {
        match command {
            Some(command) => {
                let wide: Vec<u16> = command.encode_utf16().chain([0]).collect();
                let _ = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    RUN_KEY,
                    w!("Lumen"),
                    REG_SZ.0,
                    Some(wide.as_ptr().cast()),
                    std::mem::size_of_val(wide.as_slice()) as u32,
                );
            }
            None => {
                let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Lumen"));
            }
        }
    }
}

pub fn startup_enabled() -> bool {
    startup_command().is_some()
}

/// At login Lumen starts with `--background`, so it waits for Win+Space.
pub fn set_startup(on: bool) {
    let exe = std::env::current_exe().unwrap_or_default();
    write_startup(on.then(|| format!("\"{}\" --background", exe.display())).as_deref());
}

/// Lumen 0.4 registered for login without `--background`; without it, Lumen now opens the search bar.
pub fn migrate_startup() {
    if let Some(command) = startup_command()
        && !command.contains("--background")
    {
        write_startup(Some(&format!("{command} --background")));
    }
}
