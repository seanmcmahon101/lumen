//! Global Win+Space via a low-level keyboard hook. The combo is swallowed so Windows'
//! own Win+Space (input-language switch) never sees it.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_LWIN, VK_RWIN,
    VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, SetWindowsHookExW, TranslateMessage,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

static SPACE_HELD: AtomicBool = AtomicBool::new(false);
static ON_PRESS: OnceLock<fn()> = OnceLock::new();

/// Installs the hook on its own thread (low-level hooks need a message loop).
pub fn spawn(on_press: fn()) {
    let _ = ON_PRESS.set(on_press);
    std::thread::spawn(|| unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        if SetWindowsHookExW(WH_KEYBOARD_LL, Some(callback), Some(module.into()), 0).is_err() {
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

unsafe extern "system" fn callback(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let injected = k.flags.0 & LLKHF_INJECTED.0 != 0;

        if k.vkCode == VK_SPACE.0 as u32 && !injected {
            // Ask Windows rather than tracking Win ourselves: a missed Win key-up
            // (lock screen, admin window focused) would make plain Space trigger.
            let held = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
            let win = held(VK_LWIN) || held(VK_RWIN);
            if down && win {
                if !SPACE_HELD.swap(true, Ordering::Relaxed) {
                    send_mask_key();
                    if let Some(f) = ON_PRESS.get() {
                        f();
                    }
                }
                return LRESULT(1);
            }
            if !down && SPACE_HELD.swap(false, Ordering::Relaxed) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Taps an unassigned key so releasing Win afterwards doesn't open the Start menu.
fn send_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}
