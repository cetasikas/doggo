//! Shell icon loading for the desk, on a worker thread.
//!
//! `IShellItemImageFactory` takes a few milliseconds per item, so the desk
//! asks for missing icons and repaints when batches arrive. This mirrors the
//! file search worker, and reuses its bitmap conversion.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, Result};

use crate::indexer::{self, IconPixels};

/// Posted to the desk window after each icon batch arrives.
pub const WM_DESK_ICONS: u32 = WM_APP + 41;

/// Sizes of the batches; small enough to keep the desk responsive.
const BATCH: usize = 8;

pub struct IconLoader {
    requests: Sender<Vec<Request>>,
    pub events: Receiver<Vec<(String, bool, Arc<IconPixels>)>>,
}

#[derive(Clone)]
struct Request {
    path: String,
    thumbnail: bool,
}

impl IconLoader {
    pub fn spawn(hwnd: HWND, size: i32) -> Result<Self> {
        let (requests, rx) = channel();
        let (events, event_rx) = channel();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("desk-icons".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
                worker(rx, events, size, move || unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), WM_DESK_ICONS, WPARAM(0), LPARAM(0));
                });
            })
            .map_err(|e| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string()))?;
        Ok(Self {
            requests,
            events: event_rx,
        })
    }

    /// Asks for any paths the desk does not have yet. Duplicates are dropped
    /// here so a refresh does not queue the same icon twice.
    pub fn request(&self, items: Vec<(String, bool)>) {
        if items.is_empty() {
            return;
        }
        let _ = self.requests.send(
            items
                .into_iter()
                .map(|(path, thumbnail)| Request { path, thumbnail })
                .collect(),
        );
    }
}

fn worker(
    requests: Receiver<Vec<Request>>,
    events: Sender<Vec<(String, bool, Arc<IconPixels>)>>,
    size: i32,
    notify: impl Fn(),
) {
    while let Ok(paths) = requests.recv() {
        // One request covers everything queued while the previous batch ran.
        let mut wanted: Vec<Request> = Vec::with_capacity(paths.len());
        let mut seen = HashSet::new();
        for request in paths {
            if seen.insert((request.path.to_ascii_lowercase(), request.thumbnail)) {
                wanted.push(request);
            }
        }
        while let Ok(more) = requests.try_recv() {
            for request in more {
                if seen.insert((request.path.to_ascii_lowercase(), request.thumbnail)) {
                    wanted.push(request);
                }
            }
        }

        let mut batch = Vec::with_capacity(BATCH);
        for request in wanted {
            if let Some(pixels) = unsafe { load_mode(&request.path, size, request.thumbnail) } {
                batch.push((request.path, request.thumbnail, Arc::new(pixels)));
            }
            if batch.len() >= BATCH {
                if events.send(std::mem::take(&mut batch)).is_err() {
                    return;
                }
                notify();
            }
        }
        if !batch.is_empty() {
            let _ = events.send(batch);
            notify();
        }
    }
}

pub(crate) unsafe fn load(path: &str, size: i32) -> Option<IconPixels> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        indexer::load_icon(&item, size)
    }
}

unsafe fn load_mode(path: &str, size: i32, thumbnail: bool) -> Option<IconPixels> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        if thumbnail {
            indexer::load_thumbnail(&item, size).or_else(|| indexer::load_icon(&item, size))
        } else {
            indexer::load_icon(&item, size)
        }
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    /// Like the worker thread, so COM is initialized the same way.
    #[test]
    #[ignore = "needs the real desktop and shell"]
    fn desktop_icons_load() {
        let items = crate::desk::collect(&crate::desk::desktop_dirs());
        println!("items={}", items.len());
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            let mut ok = 0;
            for item in &items {
                match unsafe { load(&item.path, 66) } {
                    Some(px) => {
                        ok += 1;
                        let opaque = px.bgra.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
                        println!("ok   {} {}x{} alpha>0 {}", item.name, px.width, px.height, opaque);
                    }
                    None => println!("FAIL {} ({})", item.name, item.path),
                }
            }
            println!("loaded {ok}/{}", items.len());
            assert!(ok > 0, "no desktop icon could be loaded");
        })
        .join()
        .unwrap();
    }
}
