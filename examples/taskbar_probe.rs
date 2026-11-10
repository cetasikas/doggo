//! Read-only diagnostic for Doggo's taskbar integration checks.
use windows::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};
use windows::core::w;

#[derive(Default)]
struct Probe {
    dock: Option<RECT>,
    visible_bars: Vec<RECT>,
    real_bars: usize,
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
    let mut probe = Probe::default();
    unsafe {
        let _ = EnumWindows(Some(inspect), LPARAM((&raw mut probe) as isize));
    }
    let args: Vec<_> = std::env::args().collect();
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
