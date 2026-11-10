//! Taskbar diagnostics, with an optional config-reload notification for integration checks.
use windows::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};
use windows::core::w;

#[derive(Default)]
struct Probe {
    dock: Option<RECT>,
    visible_bars: Vec<RECT>,
    real_bars: usize,
    work: Option<RECT>,
}

unsafe extern "system" fn inspect(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
    let mut class = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut class) };
    let class = String::from_utf16_lossy(&class[..len as usize]);
    if matches!(
        class.as_str(),
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Doggo.Launcher"
    ) {
        let mut region = RECT::default();
        let region_type = unsafe { GetWindowRgnBox(hwnd, &mut region) };
        let mut rect = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
        let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
        let proxy = !unsafe { GetPropW(hwnd, w!("Wisp.TrayProxy")) }.is_invalid();
        let probe = unsafe { &mut *(data.0 as *mut Probe) };
        if class == "Doggo.Launcher" {
            if visible {
                probe.dock = Some(rect);
                let mut info = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() } {
                    println!("Dock monitor work area: {:?}", info.rcWork);
                    probe.work = Some(info.rcWork);
                }
            }
        } else if !proxy {
            probe.real_bars += 1;
            if visible && region_type != NULLREGION {
                probe.visible_bars.push(rect);
            }
        }
        println!(
            "{class}: visible={} empty_region={} proxy={} rect={rect:?}",
            unsafe { IsWindowVisible(hwnd).as_bool() },
            region_type == NULLREGION,
            !unsafe { GetPropW(hwnd, w!("Wisp.TrayProxy")) }.is_invalid()
        );
    }
    true.into()
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--reload-config") {
        unsafe {
            let hwnd = FindWindowW(w!("Doggo.Launcher"), None).expect("Doggo must be running");
            PostMessageW(Some(hwnd), WM_APP + 6, WPARAM(0), LPARAM(0)).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let mut probe = Probe::default();
    unsafe {
        let _ = EnumWindows(Some(inspect), LPARAM((&raw mut probe) as isize));
        let mut primary = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        assert!(
            GetMonitorInfoW(
                MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
                &mut primary
            )
            .as_bool()
        );
        println!("Primary monitor work area: {:?}", primary.rcWork);
    }
    if args.iter().any(|arg| arg == "--assert-overlay") {
        let dock = probe.dock.expect("The always-visible dock must be shown");
        let work = probe.work.unwrap();
        assert!(
            work.bottom >= dock.bottom && work.top <= dock.top,
            "Windows must be able to use the area behind the dock"
        );
    }
    if args.iter().any(|arg| arg == "--expect-hidden") {
        assert!(
            probe.real_bars > 0 && probe.visible_bars.is_empty(),
            "Explorer's taskbar must be suppressed"
        );
    }
    if args.iter().any(|arg| arg == "--expect-visible") {
        assert!(!probe.visible_bars.is_empty(), "Explorer's taskbar must be restored");
    }
    if args.iter().any(|arg| arg == "--assert-clear") {
        let dock = probe
            .dock
            .expect("Doggo's dock must be visible for the placement check");
        assert!(!probe.visible_bars.is_empty());
        for bar in probe.visible_bars {
            assert!(
                dock.bottom <= bar.top || dock.top >= bar.bottom || dock.right <= bar.left || dock.left >= bar.right,
                "Doggo overlaps Explorer's taskbar: dock={dock:?}, taskbar={bar:?}"
            );
        }
    }
}
