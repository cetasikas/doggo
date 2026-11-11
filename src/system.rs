//! Clipboard access, Start with Windows and Windows theme lookup.

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::*;
use windows::core::{Error, Result, w};

pub fn set_clipboard(hwnd: HWND, text: &str) -> Result<()> {
    let data: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let result = (|| {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2)?;
            let ptr = GlobalLock(mem) as *mut u16;
            if ptr.is_null() {
                let _ = GlobalFree(Some(mem));
                return Err(Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            let _ = GlobalUnlock(mem);
            // SetClipboardData transfers memory ownership only on success.
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e);
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

pub fn get_clipboard(hwnd: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let text = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let mem = HGLOBAL(handle.0);
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // Other apps may omit the terminating NUL. Limit reads to the allocated block.
            let units = std::slice::from_raw_parts(ptr, GlobalSize(mem) / 2);
            let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..len]);
            let _ = GlobalUnlock(mem);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
    }
}

pub fn autostart_enabled() -> bool {
    crate::startup::enabled()
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    if !enabled {
        return crate::startup::set(None);
    }
    let exe = std::env::current_exe().map_err(|e| Error::new(windows::Win32::Foundation::E_FAIL, e.to_string()))?;
    let scoop = std::env::var_os("SCOOP").map(std::path::PathBuf::from);
    let exe = crate::install::autostart_path(&exe, scoop.as_deref());
    crate::startup::set(Some(&exe))
}

pub fn system_uses_light_theme() -> bool {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    result.is_ok() && value != 0
}
