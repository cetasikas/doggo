//! Explorer's desktop icons.
//!
//! Doggo never moves or deletes a desktop file. It can hide Explorer's own
//! icons through the shell's "Show desktop icons" command, and always puts
//! them back. A marker records that Doggo hid them, so an unclean exit is
//! repaired on the next start.

use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::util;

const ADVANCED: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced");
const VALUE: PCWSTR = w!("HideIcons");
/// The shell command that toggles "Show desktop icons".
const SHOW_ICONS_COMMAND: usize = 0x7402;
const TIMEOUT_MS: u32 = 1000;

fn marker() -> PathBuf {
    util::data_dir().join("desk-icons.state")
}

/// Whether Explorer is drawing its desktop icons right now. The registry value
/// alone is not enough: Explorer can rebuild its desktop and show the icons
/// again while `HideIcons` still says 1, which used to make Doggo skip hiding
/// them on the next start. The icon list's visibility is the real answer.
pub fn hidden() -> bool {
    let view = desktop_view();
    if !view.is_invalid()
        && let Ok(list) = unsafe { FindWindowExW(Some(view), None, w!("SysListView32"), None) }
        && !list.is_invalid()
    {
        return !unsafe { IsWindowVisible(list).as_bool() };
    }
    // The view is not built yet (Explorer is starting); fall back to the
    // setting Explorer keeps.
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            ADVANCED,
            VALUE,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    // A missing value means the default, which shows the icons.
    status == ERROR_SUCCESS && value != 0
}

/// Hides the icons if they are visible. Returns whether Doggo hid them, which
/// the window records so it only restores what it changed.
pub fn hide() -> bool {
    if hidden() {
        return false;
    }
    // Record the intent before touching Explorer. If Doggo dies between these
    // two steps, the next start still knows to repair the desktop.
    let _ = std::fs::write(marker(), b"hidden\n");
    if set(true) {
        true
    } else {
        let _ = std::fs::remove_file(marker());
        false
    }
}

/// Restores icons that Doggo hid. Called on exit, and at startup for a marker
/// left behind by a crash or a forced termination.
pub fn restore() {
    // Keep the marker when Explorer refuses, so the next start can try again.
    if marker().is_file() && set(false) {
        let _ = std::fs::remove_file(marker());
    }
}

/// The `SHELLDLL_DefView` window that owns the desktop icons. It can be a
/// child of `Progman`, or of a `WorkerW` when Explorer is showing a slideshow
/// wallpaper, so both places are checked.
fn desktop_view() -> HWND {
    unsafe {
        if let Ok(progman) = FindWindowW(w!("Progman"), None)
            && let Ok(view) = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None)
            && !view.is_invalid()
        {
            return view;
        }
        let mut worker = HWND::default();
        loop {
            let after = if worker.is_invalid() { None } else { Some(worker) };
            let Ok(next) = FindWindowExW(None, after, w!("WorkerW"), None) else {
                break;
            };
            if next.is_invalid() {
                break;
            }
            worker = next;
            if let Ok(view) = FindWindowExW(Some(worker), None, w!("SHELLDLL_DefView"), None)
                && !view.is_invalid()
            {
                return view;
            }
        }
        HWND::default()
    }
}

fn set(hidden: bool) -> bool {
    if self::hidden() == hidden {
        return true;
    }
    // Explorer applies the command asynchronously, so give its icon list a
    // moment to catch up before deciding that it refused.
    fn wait_for(hidden: bool) -> bool {
        for _ in 0..10 {
            if self::hidden() == hidden {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        self::hidden() == hidden
    }
    // The desktop view owns the command. When Explorer rebuilds the desktop
    // it may live under WorkerW, so try Progman and the view in turn.
    let progman = unsafe { FindWindowW(w!("Progman"), None) }.unwrap_or_default();
    let view = desktop_view();
    for target in [progman, view] {
        if target.is_invalid() {
            continue;
        }
        unsafe {
            let _ = SendMessageTimeoutW(
                target,
                WM_COMMAND,
                WPARAM(SHOW_ICONS_COMMAND),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                TIMEOUT_MS,
                None,
            );
        }
        if wait_for(hidden) {
            return true;
        }
    }

    // The shell did not take the command. Write the setting Explorer reads,
    // then ask every window to re-read it.
    let value: u32 = u32::from(hidden);
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            ADVANCED,
            VALUE,
            REG_DWORD.0,
            Some((&value as *const u32).cast()),
            4,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            None,
        );
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    wait_for(hidden)
}
