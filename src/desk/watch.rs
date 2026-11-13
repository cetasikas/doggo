//! Watches the desktop folders and asks the desk to rescan when they change.
//!
//! Explorer updates its own icons; the desk keeps a copy of the folder
//! listing, so a saved download or a new shortcut must trigger a refresh.
//! A change notification costs nothing while the desktop is idle.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_FAILED, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::HSTRING;

/// Posted to the desk window after a desktop folder changed.
pub const WM_DESK_CHANGED: u32 = WM_APP + 42;

/// How long to wait after a change before re-arming, so copying a folder does
/// not repaint once per file.
const SETTLE: Duration = Duration::from_millis(250);

pub struct Watch {
    stop: HANDLE,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Starts watching `dirs`. Returns `None` if the watch could not start.
    pub fn spawn(hwnd: HWND, dirs: Vec<PathBuf>) -> Option<Self> {
        let stop = unsafe { CreateEventW(None, true, false, None) }.ok()?;
        // Handles are not `Send`; the thread rebuilds them from their values.
        let stop_raw = stop.0 as isize;
        let target = hwnd.0 as isize;
        let thread = std::thread::Builder::new()
            .name("desk-watch".into())
            .spawn(move || watch(HANDLE(stop_raw as _), HWND(target as _), dirs))
            .ok()?;
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            let _ = CloseHandle(self.stop);
        }
    }
}

fn watch(stop: HANDLE, hwnd: HWND, dirs: Vec<PathBuf>) {
    let filter = FILE_NOTIFY_CHANGE(
        FILE_NOTIFY_CHANGE_FILE_NAME.0
            | FILE_NOTIFY_CHANGE_DIR_NAME.0
            | FILE_NOTIFY_CHANGE_LAST_WRITE.0
            | FILE_NOTIFY_CHANGE_SIZE.0,
    );
    let mut changes: Vec<HANDLE> = Vec::new();
    for dir in dirs {
        let path = HSTRING::from(dir.to_string_lossy().into_owned());
        if let Ok(handle) = unsafe { FindFirstChangeNotificationW(&path, false, filter) }
            && !handle.is_invalid()
        {
            changes.push(handle);
        }
    }
    if changes.is_empty() {
        return;
    }

    let mut handles = Vec::with_capacity(changes.len() + 1);
    handles.push(stop);
    handles.extend(changes.iter().copied());
    loop {
        let result = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if result == WAIT_FAILED || result == WAIT_OBJECT_0 {
            break;
        }
        let index = (result.0 - WAIT_OBJECT_0.0) as usize;
        if index == 0 || index > changes.len() {
            break;
        }
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_DESK_CHANGED, WPARAM(0), LPARAM(0));
        }
        // Explorer bounces entries while it finishes a copy; one repaint is
        // enough once the batch settles.
        std::thread::sleep(SETTLE);
        if unsafe { FindNextChangeNotification(changes[index - 1]) }.is_err() {
            break;
        }
    }
    for handle in changes {
        unsafe {
            let _ = FindCloseChangeNotification(handle);
        }
    }
}
