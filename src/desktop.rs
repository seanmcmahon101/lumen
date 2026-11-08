//! On-demand window discovery and switching through Win32.
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

#[derive(Clone, Debug)]
pub struct Window {
    pub handle: isize,
    pub title: String,
    pub process: u32,
    pub thread: u32,
}

pub fn windows() -> Vec<Window> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool()
                || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0
            {
                return true.into();
            }
            if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
                return true.into();
            }
            let mut text = [0u16; 1024];
            let n = GetWindowTextW(hwnd, &mut text);
            if n == 0 {
                return true.into();
            }
            let title = String::from_utf16_lossy(&text[..n as usize]);
            if title == "Lumen" || title == "Program Manager" {
                return true.into();
            }
            let mut process = 0;
            let thread = GetWindowThreadProcessId(hwnd, Some(&mut process));
            let result = &mut *(data.0 as *mut Vec<Window>);
            if result.len() < 256 {
                result.push(Window {
                    handle: hwnd.0 as isize,
                    title,
                    process,
                    thread,
                });
            }
            true.into()
        }
    }
    let mut result = Vec::new();
    let _ = unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut result as *mut Vec<Window> as isize),
        )
    };
    result
}

pub fn activate(window: &Window) -> bool {
    let hwnd = HWND(window.handle as *mut _);
    let mut process = 0;
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool()
            || GetWindowThreadProcessId(hwnd, Some(&mut process)) != window.thread
            || process != window.process
        {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd).as_bool()
    }
}

pub fn terminal(target: &str) {
    let target = target.to_string();
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        let path = std::path::Path::new(&target);
        let folder = if path.is_dir() {
            Some(path)
        } else {
            path.parent()
        };
        let Some(folder) = folder else { return };
        if std::process::Command::new("wt.exe")
            .arg("-d")
            .arg(folder)
            .spawn()
            .is_ok()
        {
            return;
        }
        // The path is a working directory, never executable shell text.
        let _ = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NoExit"])
            .current_dir(folder)
            .creation_flags(0x00000010)
            .spawn();
    });
}
