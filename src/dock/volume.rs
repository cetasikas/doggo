//! Compact dock volume popup, using the existing Direct2D renderer and Core Audio.
use super::render::Renderer;
use crate::{
    commands::audio,
    config::Backdrop,
    theme::{self, Palette},
};
use std::cell::RefCell;
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
use windows::core::{Result, w};

pub struct Popup {
    hwnd: HWND,
    // The stable allocation outlives every window callback.
    state: Box<RefCell<State>>,
}
struct State {
    renderer: Renderer,
    palette: Palette,
    dpi: u32,
    level: Option<(u32, bool)>,
    dragging: bool,
    mute_focus: bool,
}
pub const WIDTH: f32 = 64.0;
pub const HEIGHT: f32 = 232.0;
pub const TRACK_TOP: f32 = 36.0;
pub const TRACK_BOTTOM: f32 = 180.0;
pub fn percent_at(y: f32) -> u32 {
    (((TRACK_BOTTOM - y) / (TRACK_BOTTOM - TRACK_TOP)).clamp(0.0, 1.0) * 100.0).round() as u32
}

impl Popup {
    pub fn new(owner: HWND) -> Result<Self> {
        let instance = unsafe { GetModuleHandleW(None)? };
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
            lpszClassName: w!("Doggo.Volume"),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&class) } == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
            return Err(windows::core::Error::from_thread());
        }
        let mut state = Box::new(RefCell::new(State {
            renderer: Renderer::new()?,
            palette: Palette::new(true, true),
            dpi: 96,
            level: None,
            dragging: false,
            mute_focus: false,
        }));
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class.lpszClassName,
                w!("Doggo Volume"),
                WS_POPUP | WS_CAPTION,
                0,
                0,
                WIDTH as i32,
                HEIGHT as i32,
                Some(owner),
                None,
                Some(instance.into()),
                Some((&raw mut *state).cast()),
            )?
        };
        Ok(Self { hwnd, state })
    }
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
    pub fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }
    pub fn hide(&self) {
        hide(self.hwnd);
    }
    pub fn open(&mut self, anchor: RECT, dark: bool, backdrop: Backdrop) {
        let mut s = self.state.borrow_mut();
        s.dpi = unsafe { GetDpiForWindow(GetWindow(self.hwnd, GW_OWNER).unwrap_or(self.hwnd)) }.max(96);
        let scale = s.dpi as f32 / 96.0;
        let (width, height) = ((WIDTH * scale).round() as i32, (HEIGHT * scale).round() as i32);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = GetMonitorInfoW(MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST), &mut info);
        }
        let margin = (8.0 * scale).round() as i32;
        let x = ((anchor.left + anchor.right - width) / 2).clamp(
            info.rcWork.left + margin,
            (info.rcWork.right - width - margin).max(info.rcWork.left + margin),
        );
        let y = (anchor.top - height - margin).max(info.rcWork.top + margin);
        s.palette = theme::apply_backdrop(self.hwnd, dark, backdrop, true);
        s.level = audio::level();
        s.dragging = false;
        s.mute_focus = false;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            theme::shape(self.hwnd, width, height, 8.0, s.dpi);
            let _ = ShowWindow(
                self.hwnd,
                if crate::util::perf::hook_on("WISP_CAPTURE") {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_SHOW
                },
            );
        }
        drop(s);
        unsafe {
            if !crate::util::perf::hook_on("WISP_CAPTURE") {
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(Some(self.hwnd));
            }
            SetTimer(Some(self.hwnd), 1, 250, None);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    #[cfg(feature = "perf-trace")]
    pub fn capture_verify(&mut self) {
        assert!(self.visible());
        {
            let mut s = self.state.borrow_mut();
            let mut client = RECT::default();
            unsafe {
                GetClientRect(self.hwnd, &mut client).unwrap();
            }
            let scale = s.dpi as f32 / 96.0;
            assert_eq!(client.right, (WIDTH * scale).round() as i32);
            assert_eq!(client.bottom, (HEIGHT * scale).round() as i32);
            let State {
                renderer,
                palette,
                dpi,
                level,
                mute_focus,
                ..
            } = &mut *s;
            renderer
                .draw_volume(self.hwnd, *dpi, palette, *level, *mute_focus)
                .unwrap();
        }
        crate::util::perf::event("volume_popup_frame");
        unsafe {
            SendMessageW(self.hwnd, WM_KEYDOWN, Some(WPARAM(VK_TAB.0 as usize)), None);
        }
        assert!(self.state.borrow().mute_focus);
        unsafe {
            SendMessageW(self.hwnd, WM_KEYDOWN, Some(WPARAM(VK_ESCAPE.0 as usize)), None);
        }
        assert!(!self.visible());
    }
}
impl Drop for Popup {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
fn hide(hwnd: HWND) {
    unsafe {
        if GetCapture() == hwnd {
            let _ = ReleaseCapture();
        }
        let _ = KillTimer(Some(hwnd), 1);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}
impl State {
    fn volume(&mut self, percent: u32) {
        if self.level.is_some() {
            if let Err(error) = audio::set_volume(percent) {
                eprintln!("Dock volume: {error}");
            }
            self.level = audio::level();
        }
    }
    fn mute(&mut self) {
        if let Some((_, muted)) = self.level {
            if let Err(error) = audio::set_mute(!muted) {
                eprintln!("Dock mute: {error}");
            }
            self.level = audio::level();
        }
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        if msg == WM_NCCALCSIZE && wp.0 != 0 {
            return LRESULT(0);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        if msg == WM_NCHITTEST {
            return LRESULT(HTCLIENT as isize);
        }
        let cell = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>).as_ref();
        if let Some(cell) = cell
            && let Ok(mut s) = cell.try_borrow_mut()
        {
            let scale = s.dpi as f32 / 96.0;
            let (x, y) = (lp.0 as i16 as f32 / scale, (lp.0 >> 16) as i16 as f32 / scale);
            match msg {
                WM_ACTIVATE if wp.0 as u16 == WA_INACTIVE as u16 => {
                    s.dragging = false;
                    hide(hwnd);
                    return LRESULT(0);
                }
                WM_CLOSE | WM_DPICHANGED | WM_DISPLAYCHANGE => {
                    hide(hwnd);
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    if (28.0..188.0).contains(&y) && (18.0..46.0).contains(&x) {
                        s.mute_focus = false;
                        s.dragging = true;
                        SetCapture(hwnd);
                        s.volume(percent_at(y));
                    } else if (192.0..224.0).contains(&y) && (12.0..52.0).contains(&x) {
                        s.mute_focus = true;
                        s.mute();
                    }
                }
                WM_MOUSEMOVE if s.dragging => {
                    s.volume(percent_at(y));
                }
                WM_LBUTTONUP => {
                    if s.dragging {
                        s.volume(percent_at(y));
                        s.dragging = false;
                        let _ = ReleaseCapture();
                    }
                }
                WM_CAPTURECHANGED => {
                    s.dragging = false;
                }
                WM_MOUSEWHEEL => {
                    if let Some((level, _)) = s.level {
                        let delta = (wp.0 >> 16) as i16;
                        s.volume((level as i32 + if delta > 0 { 2 } else { -2 }).clamp(0, 100) as u32);
                    }
                }
                WM_KEYDOWN => {
                    let (level, _) = s.level.unwrap_or((0, false));
                    match VIRTUAL_KEY(wp.0 as u16) {
                        VK_ESCAPE => {
                            hide(hwnd);
                            return LRESULT(0);
                        }
                        VK_TAB => s.mute_focus = !s.mute_focus,
                        VK_SPACE | VK_RETURN if s.mute_focus => s.mute(),
                        VK_M => s.mute(),
                        VK_LEFT | VK_DOWN => s.volume(level.saturating_sub(1)),
                        VK_RIGHT | VK_UP => s.volume((level + 1).min(100)),
                        VK_PRIOR => s.volume((level + 10).min(100)),
                        VK_NEXT => s.volume(level.saturating_sub(10)),
                        VK_HOME => s.volume(0),
                        VK_END => s.volume(100),
                        _ => return DefWindowProcW(hwnd, msg, wp, lp),
                    }
                }
                WM_TIMER => {
                    let level = audio::level();
                    if level == s.level {
                        return LRESULT(0);
                    }
                    s.level = level;
                }
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    let State {
                        renderer,
                        palette,
                        dpi,
                        level,
                        mute_focus,
                        ..
                    } = &mut *s;
                    if let Err(error) = renderer.draw_volume(hwnd, *dpi, palette, *level, *mute_focus) {
                        eprintln!("Volume popup: {error}");
                    }
                    let _ = EndPaint(hwnd, &ps);
                    return LRESULT(0);
                }
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn slider_maps_and_clamps_endpoints() {
        assert_eq!(super::percent_at(-200.0), 100);
        assert_eq!(super::percent_at(36.0), 100);
        assert_eq!(super::percent_at(108.0), 50);
        assert_eq!(super::percent_at(180.0), 0);
        assert_eq!(super::percent_at(800.0), 0);
    }
}
