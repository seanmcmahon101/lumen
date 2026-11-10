//! Windows Settings pages and system commands, found by name or by the words
//! people use for them.

use std::os::windows::process::CommandExt;

pub enum Target {
    Open(&'static str),              // a settings URI, console or shell folder
    Run(&'static str, &'static str), // a program and its arguments
    Lock,
    EmptyRecycleBin,
}

pub struct Command {
    pub title: &'static str,
    words: &'static str, // what else people type for it, comma separated
    pub note: &'static str,
    pub glyph: char,
    pub target: Target,
    /// Asks for a second Enter first: "Press Enter again to restart".
    pub confirm: Option<&'static str>,
}

const SETTINGS: char = '\u{E713}';

const fn page(title: &'static str, words: &'static str, uri: &'static str) -> Command {
    Command { title, words, note: "Windows Settings", glyph: SETTINGS, target: Target::Open(uri), confirm: None }
}

const fn tool(title: &'static str, words: &'static str, target: Target) -> Command {
    Command { title, words, note: "Windows tool", glyph: SETTINGS, target, confirm: None }
}

const fn power(title: &'static str, words: &'static str, glyph: char, target: Target, confirm: Option<&'static str>) -> Command {
    Command { title, words, note: "Windows", glyph, target, confirm }
}

const COMMANDS: &[Command] = &[
    page("Bluetooth & devices", "bluetooth, devices, pair, headphones, earbuds", "ms-settings:bluetooth"),
    page("Display", "display, monitor, monitors, screen, resolution, brightness, scale, scaling, refresh rate", "ms-settings:display"),
    page("Night light", "night light, blue light", "ms-settings:nightlight"),
    page("Wi-Fi", "wifi, wi-fi, wireless, wlan", "ms-settings:network-wifi"),
    page("Network & internet", "network, internet, ethernet, ip address", "ms-settings:network-status"),
    page("VPN", "vpn", "ms-settings:network-vpn"),
    page("Proxy", "proxy", "ms-settings:network-proxy"),
    page("Mobile hotspot", "hotspot, tethering", "ms-settings:network-mobilehotspot"),
    page("Airplane mode", "airplane mode, flight mode", "ms-settings:network-airplanemode"),
    page("Sound", "sound, audio, volume, speakers, microphone, mic", "ms-settings:sound"),
    page("Notifications", "notifications, do not disturb", "ms-settings:notifications"),
    page("Power & battery", "power, battery, battery saver, energy, sleep settings", "ms-settings:powersleep"),
    page("Storage", "storage, disk space, storage sense, free space", "ms-settings:storagesense"),
    page("Multitasking", "multitasking, snap, virtual desktops", "ms-settings:multitasking"),
    page("Clipboard", "clipboard, clipboard history", "ms-settings:clipboard"),
    page("About this PC", "about, pc name, rename pc, computer name, specs, system info, windows version", "ms-settings:about"),
    page("Background", "background, wallpaper, desktop background", "ms-settings:personalization-background"),
    page("Colors", "colors, colours, dark mode, light mode, accent color, theme", "ms-settings:colors"),
    page("Lock screen", "lock screen, screen saver", "ms-settings:lockscreen"),
    page("Taskbar", "taskbar, tray icons", "ms-settings:taskbar"),
    page("Fonts", "fonts", "ms-settings:fonts"),
    page("Installed apps", "installed apps, uninstall, programs, add or remove programs", "ms-settings:appsfeatures"),
    page("Default apps", "default apps, default browser, open with", "ms-settings:defaultapps"),
    page("Startup apps", "startup apps, startup programs", "ms-settings:startupapps"),
    page("Optional features", "optional features", "ms-settings:optionalfeatures"),
    page("Your info", "your info, microsoft account, profile picture", "ms-settings:yourinfo"),
    page("Sign-in options", "sign-in options, password, pin, windows hello, fingerprint", "ms-settings:signinoptions"),
    page("Access work or school", "work or school, work account, school account", "ms-settings:workplace"),
    page("Date & time", "date, time, clock, time zone, timezone", "ms-settings:dateandtime"),
    page("Language & region", "language, region, keyboard layout, input language", "ms-settings:regionlanguage"),
    page("Typing", "typing, autocorrect, touch keyboard", "ms-settings:typing"),
    page("Mouse", "mouse, pointer speed, cursor", "ms-settings:mousetouchpad"),
    page("Touchpad", "touchpad, trackpad, gestures", "ms-settings:devices-touchpad"),
    page("Printers & scanners", "printers, printer, scanners, scanner", "ms-settings:printers"),
    page("Accessibility", "accessibility, ease of access, narrator, magnifier, text size, high contrast", "ms-settings:easeofaccess"),
    page("Privacy & security", "privacy, permissions", "ms-settings:privacy"),
    page("Camera privacy", "camera, webcam, camera access", "ms-settings:privacy-webcam"),
    page("Microphone privacy", "microphone access, mic access", "ms-settings:privacy-microphone"),
    page("Location", "location, gps", "ms-settings:privacy-location"),
    page("Windows Update", "windows update, update, updates", "ms-settings:windowsupdate"),
    page("Recovery", "recovery, reset pc, reset this pc, advanced startup", "ms-settings:recovery"),
    tool("Device Manager", "device manager, drivers", Target::Open("devmgmt.msc")),
    tool("Disk Management", "disk management, partitions, format drive", Target::Open("diskmgmt.msc")),
    tool("Environment variables", "environment variables, env vars, path variable", Target::Run("rundll32.exe", "sysdm.cpl,EditEnvironmentVariables")),
    tool("Recycle Bin", "recycle bin, trash, deleted files", Target::Open("shell:RecycleBinFolder")),
    power("Lock", "lock, lock pc, lock computer", '\u{E72E}', Target::Lock, None),
    power("Restart", "restart, reboot", '\u{E72C}', Target::Run("shutdown.exe", "/r /t 0"), Some("restart")),
    power("Shut down", "shut down, shutdown, power off, turn off", '\u{E7E8}', Target::Run("shutdown.exe", "/s /t 0"), Some("shut down")),
    power("Sign out", "sign out, log out, logout, log off, logoff", '\u{E77B}', Target::Run("shutdown.exe", "/l"), Some("sign out")),
    power("Empty Recycle Bin", "empty recycle bin, empty trash", '\u{E74D}', Target::EmptyRecycleBin, None),
];

/// Commands whose title or words equal the query, then, from four characters,
/// those starting with it: (exact, prefix), at most three in all.
pub fn find(query: &str) -> (Vec<&'static Command>, Vec<&'static Command>) {
    let q = query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let (mut exact, mut prefix) = (Vec::new(), Vec::new());
    if q.chars().count() < 3 {
        return (exact, prefix);
    }
    for command in COMMANDS {
        let title = command.title.to_lowercase();
        let names = || std::iter::once(title.as_str()).chain(command.words.split(", "));
        if names().any(|name| name == q) {
            exact.push(command);
        } else if q.chars().count() >= 4 && names().any(|name| name.starts_with(&q)) {
            prefix.push(command);
        }
    }
    exact.truncate(3);
    prefix.truncate(3 - exact.len());
    (exact, prefix)
}

pub fn run(target: &'static Target) {
    match target {
        Target::Open(target) => {
            crate::win::shell_open(target, "open", None);
        }
        Target::Run(program, arguments) => {
            let _ = std::process::Command::new(program).raw_arg(arguments).creation_flags(crate::win::NO_WINDOW).spawn();
        }
        Target::Lock => unsafe {
            let _ = windows::Win32::System::Shutdown::LockWorkStation();
        },
        // Windows asks for confirmation itself; its dialog must not hold up the launcher.
        Target::EmptyRecycleBin => {
            std::thread::spawn(|| unsafe {
                let _ = windows::Win32::UI::Shell::SHEmptyRecycleBinW(None, windows::core::PCWSTR::null(), 0);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(found: Vec<&Command>) -> Vec<&str> {
        found.iter().map(|c| c.title).collect()
    }

    #[test]
    fn finds_settings_by_name_and_common_words() {
        assert_eq!(titles(find("bluetooth").0), ["Bluetooth & devices"]);
        assert_eq!(titles(find("WiFi").0), ["Wi-Fi"]);
        assert_eq!(titles(find("dark  mode").0), ["Colors"]);
        assert_eq!(titles(find("reboot").0), ["Restart"]);
        assert_eq!(titles(find("blue").1), ["Bluetooth & devices", "Night light"]);
        assert_eq!(titles(find("lock").0), ["Lock"]);
        assert_eq!(titles(find("lock").1), ["Lock screen"]);
    }

    #[test]
    fn short_or_unrelated_queries_find_nothing() {
        for q in ["bl", "pro", "budget 2026", "quarterly report"] {
            let (exact, prefix) = find(q);
            assert!(exact.is_empty() && prefix.is_empty(), "{q}");
        }
    }

    #[test]
    fn only_power_commands_that_close_apps_ask_first() {
        let asks: Vec<&str> = COMMANDS.iter().filter(|c| c.confirm.is_some()).map(|c| c.title).collect();
        assert_eq!(asks, ["Restart", "Shut down", "Sign out"]);
    }
}
