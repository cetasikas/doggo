//! The desk window: a full-monitor, non-activating window whose region is the
//! union of the fences. Clicks between fences reach Explorer's desktop, and
//! the window sits on the wallpaper layer, directly above the shell's desktop
//! and below every normal window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CombineRgn, CreatePen, CreateRectRgn, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, EndPaint,
    FillRect, GetStockObject, HDC, InvalidateRect, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, RGN_OR, RoundRect,
    ScreenToClient, SelectObject, SetBkColor, SetTextColor, SetWindowRgn, TextOutW, UpdateWindow,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, PCWSTR, Result, w};

use crate::config::Backdrop;
use crate::desk::icons::{IconLoader, WM_DESK_ICONS};
use crate::desk::render::{self, Frame, Hit};
use crate::desk::watch::{self, Watch};
use crate::desk::{self, Desk, Item, Placed};
use crate::dock::WM_DOCK_DESK_ICONS;
use crate::indexer::IconPixels;
use crate::theme::Palette;
use crate::{launch, tray};

/// The desk window class, kept separate from the launcher and dock.
const CLASS_NAME: windows::core::PCWSTR = w!("Doggo.Desk");

/// Re-places the desk above Explorer's desktop if Explorer pushed itself over
/// it. Cheap to check, so it runs a few times a second.
const HEAL_TIMER: usize = 1;
const SCROLL_TIMER: usize = 3;
const WM_TAB_EDIT_DONE: u32 = WM_APP + 0x248;
const INLINE_TAB_CLASS: windows::core::PCWSTR = w!("Doggo.InlineTabName");

/// Ids for the fence menu, which is drawn and dismissed by Windows.
const MENU_ROLL: usize = 1;
const MENU_EXPAND: usize = 2;
const MENU_GRID: usize = 3;
const MENU_LEFT: usize = 4;
const MENU_RIGHT: usize = 5;
const MENU_DESK_FILE: usize = 6;
const MENU_SETTINGS: usize = 7;
const MENU_ICONS: usize = 8;
const MENU_CREATE_FENCE: usize = 9;
const MENU_RENAME_FENCE: usize = 10;
const MENU_DELETE_FENCE: usize = 11;
const MENU_ADD_TAB: usize = 12;
const MENU_RENAME_TAB: usize = 13;
const MENU_DELETE_TAB: usize = 14;
const MENU_TINT_NONE: usize = 20;
const MENU_TINT_BLUE: usize = 21;
const MENU_TINT_VIOLET: usize = 22;
const MENU_TINT_PINK: usize = 23;
const MENU_TINT_GREEN: usize = 24;
const MENU_TINT_AMBER: usize = 25;
const MENU_ICON_BASE: usize = 30;
const ICON_SIZES: [u32; 7] = [16, 24, 32, 40, 48, 64, 80];
const MENU_VIEW_BASE: usize = 40;
const VIEWS: [desk::FenceView; 3] = [
    desk::FenceView::Icons,
    desk::FenceView::Thumbnails,
    desk::FenceView::Details,
];
const MENU_SORT_BASE: usize = 50;
const SORTS: [desk::FenceSort; 4] = [
    desk::FenceSort::Name,
    desk::FenceSort::Type,
    desk::FenceSort::Size,
    desk::FenceSort::Modified,
];
const MENU_TRANSPARENCY_BASE: usize = 60;
const TRANSPARENCY: [u8; 6] = [0, 15, 30, 45, 60, 75];
const MENU_MOVE_BASE: usize = 100;

fn icon_key(path: &str, thumbnail: bool) -> String {
    format!("{}:{path}", if thumbnail { "thumb" } else { "icon" })
}

struct Drag {
    fence: usize,
    /// Cursor and fence origin when the drag started, in DIPs.
    start: (f32, f32),
    origin: (i32, i32),
    moved: bool,
}

struct Resize {
    fence: usize,
    start: (f32, f32),
    origin_width: i32,
    origin_height: i32,
}

struct ScrollDrag {
    fence: usize,
    start_y: f32,
    origin_scroll: i32,
}

#[derive(Clone, Copy)]
struct PromptColors {
    background: u32,
    input: u32,
    text: u32,
    muted: u32,
    accent: u32,
    button_text: u32,
    border: u32,
}

struct NamePrompt {
    edit: HWND,
    result: Option<String>,
    heading: Vec<u16>,
    description: Vec<u16>,
    accept: Vec<u16>,
    colors: PromptColors,
    scale: f32,
    background_brush: windows::Win32::Graphics::Gdi::HBRUSH,
    input_brush: windows::Win32::Graphics::Gdi::HBRUSH,
}

struct InlineEditData {
    owner: HWND,
    edit: HWND,
    colors: PromptColors,
    background_brush: windows::Win32::Graphics::Gdi::HBRUSH,
    scale: f32,
    ended: bool,
}

struct TabEdit {
    hwnd: HWND,
    edit: HWND,
    fence: usize,
    tab: usize,
    original: String,
    _data: Box<InlineEditData>,
}

impl Drop for InlineEditData {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.background_brush.into());
        }
    }
}

fn prompt_tint(base: u32, tint: Option<u32>, amount: f32) -> u32 {
    let Some(tint) = tint else { return base };
    let mix = |shift: u32| {
        let a = ((base >> shift) & 0xff) as f32;
        let b = ((tint >> shift) & 0xff) as f32;
        (a + (b - a) * amount).round() as u32
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

fn prompt_colorref(rgb: u32) -> u32 {
    ((rgb & 0xff) << 16) | (rgb & 0x00ff00) | ((rgb >> 16) & 0xff)
}

fn prompt_colors(dark: bool, tint: Option<u32>) -> PromptColors {
    if dark {
        PromptColors {
            background: prompt_tint(0x202020, tint, 0.16),
            input: prompt_tint(0x303030, tint, 0.16),
            text: 0xeaeaea,
            muted: 0x9aa3b5,
            accent: tint.unwrap_or(0x2867c7),
            button_text: 0xffffff,
            border: prompt_tint(0x3a3a3a, tint, 0.32),
        }
    } else {
        PromptColors {
            background: prompt_tint(0xf4f5f7, tint, 0.12),
            input: prompt_tint(0xffffff, tint, 0.08),
            text: 0x1a1d24,
            muted: 0x5c6577,
            accent: tint.unwrap_or(0x2867c7),
            button_text: 0xffffff,
            border: prompt_tint(0xc9cfd9, tint, 0.28),
        }
    }
}

unsafe fn finish_inline_edit(hwnd: HWND, commit: bool, data: *mut InlineEditData) {
    unsafe {
        if data.is_null() || (*data).ended {
            return;
        }
        (*data).ended = true;
        let _ = PostMessageW(
            Some((*data).owner),
            WM_TAB_EDIT_DONE,
            WPARAM(hwnd.0 as usize),
            LPARAM(commit as isize),
        );
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

unsafe extern "system" fn inline_tab_edit_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_KILLFOCUS
            && let Ok(host) = GetParent(hwnd)
        {
            let _ = PostMessageW(Some(host), WM_CLOSE, WPARAM(1), LPARAM(0));
        }
        if message == WM_KEYDOWN && (wp.0 == VK_RETURN.0 as usize || wp.0 == VK_ESCAPE.0 as usize) {
            let Ok(host) = GetParent(hwnd) else { return LRESULT(0) };
            let _ = PostMessageW(
                Some(host),
                WM_CLOSE,
                WPARAM((wp.0 == VK_RETURN.0 as usize) as usize),
                LPARAM(0),
            );
            return LRESULT(0);
        }
        let previous = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if previous != 0 {
            let proc: WNDPROC = std::mem::transmute(previous);
            CallWindowProcW(proc, hwnd, message, wp, lp)
        } else {
            DefWindowProcW(hwnd, message, wp, lp)
        }
    }
}

unsafe extern "system" fn inline_tab_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut InlineEditData;
        match message {
            WM_CREATE => {
                if !data.is_null() {
                    let scale = (*data).scale;
                    let px = |value: i32| (scale * value as f32).round() as i32;
                    let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                        .ok()
                        .map(Into::into);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let edit = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("EDIT"),
                        w!(""),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32 | ES_CENTER as u32),
                        px(3),
                        px(2),
                        (rect.right - px(6)).max(1),
                        (rect.bottom - px(2)).max(1),
                        Some(hwnd),
                        Some(HMENU(1 as _)),
                        instance,
                        None,
                    );
                    if let Ok(edit) = edit {
                        (*data).edit = edit;
                        let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, inline_tab_edit_proc as *const () as isize);
                        let _ = SetWindowLongPtrW(edit, GWLP_USERDATA, old);
                    }
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => {
                if !data.is_null() {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let _ = FillRect(HDC(wp.0 as _), &rect, (*data).background_brush);
                }
                LRESULT(1)
            }
            WM_CTLCOLOREDIT if !data.is_null() => {
                let dc = HDC(wp.0 as _);
                let _ = SetTextColor(
                    dc,
                    windows::Win32::Foundation::COLORREF(prompt_colorref((*data).colors.text)),
                );
                let _ = SetBkColor(
                    dc,
                    windows::Win32::Foundation::COLORREF(prompt_colorref((*data).colors.input)),
                );
                LRESULT((*data).background_brush.0 as isize)
            }
            WM_CLOSE if !data.is_null() => {
                finish_inline_edit(hwnd, wp.0 != 0, data);
                LRESULT(0)
            }
            WM_ACTIVATE if !data.is_null() && (wp.0 as u32 & 0xffff) == WA_INACTIVE as u32 => {
                finish_inline_edit(hwnd, true, data);
                LRESULT(0)
            }
            WM_NCDESTROY if !data.is_null() => {
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, message, wp, lp)
            }
            _ => DefWindowProcW(hwnd, message, wp, lp),
        }
    }
}

unsafe extern "system" fn name_prompt_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NamePrompt;
        match message {
            WM_CREATE => {
                if !data.is_null() {
                    let scale = (*data).scale;
                    let px = |value: i32| (scale * value as f32).round() as i32;
                    let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                        .ok()
                        .map(Into::into);
                    let _ = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("STATIC"),
                        PCWSTR((*data).heading.as_ptr()),
                        WS_CHILD | WS_VISIBLE,
                        px(20),
                        px(18),
                        px(340),
                        px(22),
                        Some(hwnd),
                        Some(HMENU(10 as _)),
                        instance,
                        None,
                    );
                    let _ = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("STATIC"),
                        PCWSTR((*data).description.as_ptr()),
                        WS_CHILD | WS_VISIBLE,
                        px(20),
                        px(44),
                        px(340),
                        px(20),
                        Some(hwnd),
                        Some(HMENU(11 as _)),
                        instance,
                        None,
                    );
                    let edit = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("EDIT"),
                        w!(""),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                        px(20),
                        px(72),
                        px(340),
                        px(30),
                        Some(hwnd),
                        Some(HMENU(1 as _)),
                        instance,
                        None,
                    );
                    (*data).edit = edit.unwrap_or_default();
                    let _ = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("BUTTON"),
                        w!("Cancel"),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                        px(182),
                        px(126),
                        px(82),
                        px(32),
                        Some(hwnd),
                        Some(HMENU(2 as _)),
                        instance,
                        None,
                    );
                    let _ = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("BUTTON"),
                        PCWSTR((*data).accept.as_ptr()),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                        px(272),
                        px(126),
                        px(88),
                        px(32),
                        Some(hwnd),
                        Some(HMENU(3 as _)),
                        instance,
                        None,
                    );
                    let _ = SetFocus(Some((*data).edit));
                    let _ = SendMessageW((*data).edit, 0x00B1, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => {
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let _ = FillRect(HDC(wp.0 as _), &rect, (*data).background_brush);
                LRESULT(1)
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                let pen = CreatePen(
                    PS_SOLID,
                    1,
                    windows::Win32::Foundation::COLORREF(prompt_colorref((*data).colors.border)),
                );
                let old_pen = SelectObject(dc, pen.into());
                let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let _ = RoundRect(dc, 0, 0, rect.right, rect.bottom, 18, 18);
                let _ = SelectObject(dc, old_brush);
                let _ = SelectObject(dc, old_pen);
                let _ = DeleteObject(pen.into());
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC => {
                let dc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
                let control = HWND(lp.0 as _);
                let color = if GetDlgCtrlID(control) == 11 {
                    (*data).colors.muted
                } else {
                    (*data).colors.text
                };
                let _ = SetTextColor(dc, windows::Win32::Foundation::COLORREF(prompt_colorref(color)));
                let _ = SetBkColor(
                    dc,
                    windows::Win32::Foundation::COLORREF(prompt_colorref((*data).colors.background)),
                );
                LRESULT((*data).background_brush.0 as isize)
            }
            WM_CTLCOLOREDIT => {
                let dc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
                let _ = SetTextColor(
                    dc,
                    windows::Win32::Foundation::COLORREF(prompt_colorref((*data).colors.text)),
                );
                let _ = SetBkColor(
                    dc,
                    windows::Win32::Foundation::COLORREF(prompt_colorref((*data).colors.input)),
                );
                LRESULT((*data).input_brush.0 as isize)
            }
            WM_DRAWITEM => {
                let draw = &*(lp.0 as *const windows::Win32::UI::Controls::DRAWITEMSTRUCT);
                let primary = draw.CtlID == 3;
                let fill = if primary {
                    (*data).colors.accent
                } else {
                    (*data).colors.input
                };
                let text = if primary {
                    (*data).colors.button_text
                } else {
                    (*data).colors.text
                };
                let dc = draw.hDC;
                let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(prompt_colorref(fill)));
                let pen = CreatePen(
                    PS_SOLID,
                    1,
                    windows::Win32::Foundation::COLORREF(prompt_colorref((*data).colors.border)),
                );
                let old_brush = SelectObject(dc, brush.into());
                let old_pen = SelectObject(dc, pen.into());
                let radius = ((*data).scale * 9.0).round() as i32;
                let _ = RoundRect(
                    dc,
                    draw.rcItem.left,
                    draw.rcItem.top,
                    draw.rcItem.right,
                    draw.rcItem.bottom,
                    radius,
                    radius,
                );
                let _ = SelectObject(dc, old_brush);
                let _ = SelectObject(dc, old_pen);
                let _ = DeleteObject(brush.into());
                let _ = DeleteObject(pen.into());
                let _ = SetBkColor(dc, windows::Win32::Foundation::COLORREF(prompt_colorref(fill)));
                let _ = SetTextColor(dc, windows::Win32::Foundation::COLORREF(prompt_colorref(text)));
                let cancel_label: Vec<u16> = "Cancel\0".encode_utf16().collect();
                let label = if primary { &(*data).accept } else { &cancel_label };
                let label_len = label.iter().position(|ch| *ch == 0).unwrap_or(label.len()) as i32;
                let width = draw.rcItem.right - draw.rcItem.left;
                let text_width = (label_len as f32 * 8.0 * (*data).scale).round() as i32;
                let x = draw.rcItem.left + (width - text_width) / 2;
                let text_height = (16.0 * (*data).scale).round() as i32;
                let y = draw.rcItem.top + (draw.rcItem.bottom - draw.rcItem.top - text_height) / 2;
                let _ = TextOutW(dc, x, y, &label[..label_len as usize]);
                LRESULT(1)
            }
            WM_COMMAND => {
                let id = wp.0 & 0xFFFF;
                if id == 3 && !data.is_null() {
                    let mut buffer = [0u16; 256];
                    let _ = GetWindowTextW((*data).edit, &mut buffer);
                    let value = String::from_utf16_lossy(
                        &buffer[..buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len())],
                    );
                    let value = value.trim();
                    if !value.is_empty() {
                        (*data).result = Some(value.to_owned());
                    }
                }
                if id == 2 || id == 3 {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, message, wp, lp)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_ACTIVATE if (wp.0 as u32 & 0xffff) == WA_INACTIVE as u32 => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wp, lp),
        }
    }
}

fn prompt_fence_name(
    owner: HWND,
    initial: &str,
    heading: &str,
    description: &str,
    accept: &str,
    center: POINT,
    dpi: u32,
    dark: bool,
    tint: Option<u32>,
) -> Option<String> {
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(name_prompt_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            lpszClassName: w!("Doggo.FenceNamePrompt"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return None;
        }
        let scale = dpi.max(96) as f32 / 96.0;
        let px = |value: i32| (value as f32 * scale).round() as i32;
        let (width, height) = (px(380), px(180));
        let mut owner_rect = RECT::default();
        let _ = GetWindowRect(owner, &mut owner_rect);
        let left = (center.x - width / 2).clamp(owner_rect.left, (owner_rect.right - width).max(owner_rect.left));
        let top = (center.y - height / 2).clamp(owner_rect.top, (owner_rect.bottom - height).max(owner_rect.top));
        let title: Vec<u16> = "Doggo\0".encode_utf16().collect();
        let mut text: Vec<u16> = initial.encode_utf16().chain(Some(0)).collect();
        let heading = heading.encode_utf16().chain(Some(0)).collect();
        let description = description.encode_utf16().chain(Some(0)).collect();
        let accept = accept.encode_utf16().chain(Some(0)).collect();
        let colors = prompt_colors(dark, tint);
        let background_brush =
            CreateSolidBrush(windows::Win32::Foundation::COLORREF(prompt_colorref(colors.background)));
        let input_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(prompt_colorref(colors.input)));
        let mut prompt = NamePrompt {
            edit: HWND::default(),
            result: None,
            heading,
            description,
            accept,
            colors,
            scale,
            background_brush,
            input_brush,
        };
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("Doggo.FenceNamePrompt"),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            left,
            top,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            Some((&mut prompt as *mut NamePrompt).cast()),
        )
        .ok()?;
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, px(18), px(18));
        let _ = SetWindowRgn(hwnd, Some(region), true);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        let _ = SetWindowTextW(prompt.edit, PCWSTR(text.as_mut_ptr()));
        let _ = SetFocus(Some(prompt.edit));
        let _ = SendMessageW(prompt.edit, 0x00B1, Some(WPARAM(0)), Some(LPARAM(-1)));
        let _ = SetForegroundWindow(hwnd);
        let mut message = MSG::default();
        while IsWindow(Some(hwnd)).as_bool() {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status <= 0 {
                break;
            }
            if !IsDialogMessageW(hwnd, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let result = prompt.result;
        let _ = DeleteObject(prompt.background_brush.into());
        let _ = DeleteObject(prompt.input_brush.into());
        result
    }
}

struct State {
    hwnd: HWND,
    owner: HWND,
    desk: Desk,
    items: Vec<Item>,
    placed: Vec<Placed>,
    scroll_targets: HashMap<usize, f32>,
    icons: HashMap<String, Arc<IconPixels>>,
    loader: Option<IconLoader>,
    watch: Option<Watch>,
    /// The folders the watcher is currently watching.
    watched: Vec<PathBuf>,
    renderer: render::Renderer,
    palette: Palette,
    dark: bool,
    show_labels: bool,
    dpi: u32,
    hover: Option<Hit>,
    drag: Option<Drag>,
    tab_edit: Option<TabEdit>,
    resize: Option<Resize>,
    scroll_drag: Option<ScrollDrag>,
    /// Doggo hid Explorer's icons and must put them back.
    hid_icons: bool,
    /// Counts heal timer ticks; the icon check runs less often than the
    /// z-order check because asking the shell to hide its icons can block.
    heal_ticks: u32,
    /// Set when the renderer completed a frame; read by the UI test.
    #[cfg(test)]
    painted: bool,
}

impl State {
    fn rescan(&mut self) {
        self.items = desk::collect(&desk::desktop_dirs());
        self.relayout();
        self.request_icons();
        let dirs = self.desk.watch_dirs();
        if dirs != self.watched {
            self.watch = Watch::spawn(self.hwnd, dirs.clone());
            self.watched = dirs;
        }
        self.invalidate();
    }

    fn enter_folder(&mut self, index: usize, path: String) {
        if !Path::new(&path).is_dir() {
            return;
        }
        let current_height = self.placed.get(index).map(|fence| fence.height);
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        if fence.browse_stack.is_empty() && fence.height_override.is_none() {
            fence.browse_height = current_height.map(|height| height.max(desk::MIN_HEIGHT));
        }
        fence.browse_stack.push(path);
        fence.scroll_y = 0;
        self.hover = None;
        self.rescan();
    }

    fn navigate_back(&mut self, index: usize) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        if fence.browse_stack.pop().is_some() {
            if fence.browse_stack.is_empty() {
                fence.browse_height = None;
            }
            fence.scroll_y = 0;
            self.hover = None;
            self.rescan();
        }
    }

    fn relayout(&mut self) {
        if !self.scroll_targets.is_empty() {
            self.scroll_targets.clear();
            unsafe {
                let _ = KillTimer(Some(self.hwnd), SCROLL_TIMER);
            }
        }
        self.placed = self.desk.layout(&self.items);
        for placed in &mut self.placed {
            let max_scroll = placed.max_scroll();
            let scroll = self.desk.fence[placed.index].scroll_y.clamp(0, max_scroll);
            placed.scroll_y = scroll;
            self.desk.fence[placed.index].scroll_y = scroll;
        }
        let (width, height) = self.dip_size();
        if desk::separate(&mut self.placed, width.round() as i32, height.round() as i32) {
            for fence in &self.placed {
                if let Some(saved) = self.desk.fence.get_mut(fence.index) {
                    saved.x = fence.x;
                    saved.y = fence.y;
                }
            }
            self.save();
        }
        self.apply_region();
    }

    fn request_icons(&mut self) {
        let Some(loader) = &self.loader else {
            return;
        };
        let missing: Vec<(String, bool)> = self
            .placed
            .iter()
            .flat_map(|fence| {
                fence
                    .items
                    .iter()
                    .map(move |item| (item.path.clone(), fence.view == desk::FenceView::Thumbnails))
            })
            .filter(|(path, thumbnail)| !self.icons.contains_key(&icon_key(path, *thumbnail)))
            .collect();
        loader.request(missing);
    }

    /// The window region is the union of the fence panels, in physical pixels.
    /// Everything outside it belongs to the desktop again.
    fn apply_region(&self) {
        let scale = self.dpi as f32 / 96.0;
        let diameter = (render::RADIUS * 2.0 * scale).round().max(1.0) as i32;
        unsafe {
            let total = CreateRectRgn(0, 0, 0, 0);
            if total.is_invalid() {
                return;
            }
            for fence in &self.placed {
                let left = (fence.x as f32 * scale).round() as i32;
                let top = (fence.y as f32 * scale).round() as i32;
                let right = ((fence.x + fence.width) as f32 * scale).round() as i32;
                let bottom = ((fence.y + fence.height) as f32 * scale).round() as i32;
                let part = CreateRoundRectRgn(left, top, right + 1, bottom + 1, diameter, diameter);
                if !part.is_invalid() {
                    let _ = CombineRgn(Some(total), Some(total), Some(part), RGN_OR);
                    let _ = DeleteObject(part.into());
                }
            }
            if SetWindowRgn(self.hwnd, Some(total), false) == 0 {
                let _ = DeleteObject(total.into());
            }
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn apply_material(&mut self) {
        // The layered DIB carries per-pixel alpha over the real desktop, so
        // the system backdrop must not paint an opaque surface behind it.
        self.palette = Palette::new(self.dark, true);
        self.apply_region();
    }

    fn paint(&mut self) {
        let frame = Frame {
            palette: &self.palette,
            fences: &self.placed,
            icons: &self.icons,
            show_labels: self.show_labels,
            hover: self.hover,
        };
        if let Err(error) = self.renderer.draw(self.hwnd, &frame) {
            eprintln!("could not draw the desk: {error}");
        } else {
            #[cfg(test)]
            {
                self.painted = true;
            }
        }
    }

    /// Client pixels to DIPs, matching the renderer's coordinate space.
    fn dip(&self, lparam: LPARAM) -> (f32, f32) {
        let (x, y) = (
            (lparam.0 & 0xFFFF) as i16 as i32,
            ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
        );
        let scale = 96.0 / self.dpi as f32;
        (x as f32 * scale, y as f32 * scale)
    }

    fn dip_size(&self) -> (f32, f32) {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rect);
        }
        let scale = 96.0 / self.dpi as f32;
        (rect.right as f32 * scale, rect.bottom as f32 * scale)
    }

    fn scroll_fence(&mut self, index: usize, wheel_delta: i16) {
        let Some(placed) = self.placed.get(index) else {
            return;
        };
        let max_scroll = placed.max_scroll();
        if max_scroll == 0 {
            return;
        }
        let current = self
            .scroll_targets
            .get(&index)
            .copied()
            .unwrap_or(placed.scroll_y as f32);
        let next =
            (current - wheel_delta as f32 * placed.cell_height() as f32 * 1.5 / 120.0).clamp(0.0, max_scroll as f32);
        if (next - current).abs() < 0.5 {
            return;
        }
        self.scroll_targets.insert(index, next);
        unsafe {
            let _ = SetTimer(Some(self.hwnd), SCROLL_TIMER, 16, None);
        }
    }

    fn animate_scroll(&mut self) {
        let targets: Vec<(usize, f32)> = self
            .scroll_targets
            .iter()
            .map(|(index, target)| (*index, *target))
            .collect();
        let mut changed = false;
        for (index, target) in targets {
            let Some(placed) = self.placed.get(index) else {
                self.scroll_targets.remove(&index);
                continue;
            };
            let current = placed.scroll_y as f32;
            let difference = target - current;
            if difference.abs() <= 1.0 {
                let final_value = target.round() as i32;
                self.placed[index].scroll_y = final_value;
                self.desk.fence[index].scroll_y = final_value;
                self.scroll_targets.remove(&index);
                changed |= final_value != current as i32;
            } else {
                let mut next = (current + difference * 0.28).round() as i32;
                if next == current as i32 {
                    next += difference.signum() as i32;
                }
                next = next.clamp(0, placed.max_scroll());
                self.placed[index].scroll_y = next;
                self.desk.fence[index].scroll_y = next;
                changed = true;
            }
        }
        if self.scroll_targets.is_empty() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), SCROLL_TIMER);
            }
        }
        if changed {
            self.invalidate();
        }
    }

    fn drag_scrollbar(&mut self, index: usize, start_y: f32, origin_scroll: i32, y: f32) {
        self.scroll_targets.remove(&index);
        if self.scroll_targets.is_empty() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), SCROLL_TIMER);
            }
        }
        let Some(fence) = self.placed.get(index) else {
            return;
        };
        let max_scroll = fence.max_scroll();
        if max_scroll == 0 {
            return;
        }
        let track_height = (fence.height as f32 - desk::TITLE_HEIGHT as f32 - 26.0).max(12.0);
        let view_height = (fence.height - desk::TITLE_HEIGHT).max(1) as f32;
        let thumb_height = (track_height * view_height / (view_height + max_scroll as f32)).clamp(18.0, track_height);
        let travel = track_height - thumb_height;
        if travel <= 0.0 {
            return;
        }
        let next = (origin_scroll as f32 + (y - start_y) * max_scroll as f32 / travel)
            .round()
            .clamp(0.0, max_scroll as f32) as i32;
        if next != fence.scroll_y {
            self.placed[index].scroll_y = next;
            self.desk.fence[index].scroll_y = next;
            self.invalidate();
        }
    }

    fn toggle_collapsed(&mut self, index: usize) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        fence.collapsed = !fence.collapsed;
        self.relayout();
        self.invalidate();
        self.save();
    }

    /// Tidies every fence into `how` and remembers the result.
    fn arrange(&mut self, how: desk::Arrange) {
        let (width, height) = self.dip_size();
        if !desk::arrange(&mut self.placed, width.round() as i32, height.round() as i32, how) {
            return;
        }
        for fence in &self.placed {
            if let Some(saved) = self.desk.fence.get_mut(fence.index) {
                saved.x = fence.x;
                saved.y = fence.y;
            }
        }
        self.save();
        self.apply_region();
        self.invalidate();
    }

    /// The right-click menu for a fence: roll-up, the tidy layouts, settings
    /// and the way back to Explorer's icons.
    fn fence_menu(&mut self, index: usize, item_slot: Option<usize>) {
        let Some(fence) = self.desk.fence.get(index) else {
            return;
        };
        let collapsed = fence.collapsed;
        let active_is_main = fence.tabs.get(fence.active_tab).is_some_and(|tab| tab.is_main);
        let is_custom = fence.category.is_none() && fence.folder.is_none();
        let move_targets: Vec<(usize, String)> = item_slot.map_or_else(Vec::new, |_| {
            self.desk
                .fence
                .iter()
                .enumerate()
                .filter(|(target, fence)| *target != index && fence.category.is_none() && fence.folder.is_none())
                .map(|(target, fence)| (target, fence.name.clone()))
                .collect()
        });
        let owner = self.owner;
        self.hover = None;
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                if collapsed { MENU_EXPAND } else { MENU_ROLL },
                if collapsed { w!("Expand") } else { w!("Roll up") },
            );
            let Ok(colors) = CreatePopupMenu() else {
                let _ = DestroyMenu(menu);
                return;
            };
            let _ = AppendMenuW(colors, MF_STRING, MENU_TINT_NONE, w!("Theme default"));
            let _ = AppendMenuW(colors, MF_STRING, MENU_TINT_BLUE, w!("Electric blue"));
            let _ = AppendMenuW(colors, MF_STRING, MENU_TINT_VIOLET, w!("Violet"));
            let _ = AppendMenuW(colors, MF_STRING, MENU_TINT_PINK, w!("Pink"));
            let _ = AppendMenuW(colors, MF_STRING, MENU_TINT_GREEN, w!("Green"));
            let _ = AppendMenuW(colors, MF_STRING, MENU_TINT_AMBER, w!("Amber"));
            let _ = AppendMenuW(menu, MF_POPUP, colors.0 as usize, w!("Fence color"));
            if true {
                let Ok(icon_sizes) = CreatePopupMenu() else {
                    let _ = DestroyMenu(menu);
                    return;
                };
                let mut icon_labels = Vec::new();
                for (offset, size) in ICON_SIZES.iter().enumerate() {
                    icon_labels.push(format!("{size} DIP\0").encode_utf16().collect::<Vec<_>>());
                    let flags = MF_STRING
                        | if fence.icon_size == *size {
                            MF_CHECKED
                        } else {
                            MF_UNCHECKED
                        };
                    let _ = AppendMenuW(
                        icon_sizes,
                        flags,
                        MENU_ICON_BASE + offset,
                        PCWSTR(icon_labels.last().unwrap().as_ptr()),
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, icon_sizes.0 as usize, w!("Icon size"));
                let Ok(views) = CreatePopupMenu() else {
                    let _ = DestroyMenu(menu);
                    return;
                };
                for (offset, view) in VIEWS.iter().enumerate() {
                    let label = match view {
                        desk::FenceView::Icons => w!("Icons"),
                        desk::FenceView::Thumbnails => w!("Thumbnails"),
                        desk::FenceView::Details => w!("Details"),
                    };
                    let flags = MF_STRING | if fence.view == *view { MF_CHECKED } else { MF_UNCHECKED };
                    let _ = AppendMenuW(views, flags, MENU_VIEW_BASE + offset, label);
                }
                let _ = AppendMenuW(menu, MF_POPUP, views.0 as usize, w!("View as"));
                let Ok(sorts) = CreatePopupMenu() else {
                    let _ = DestroyMenu(menu);
                    return;
                };
                for (offset, sort) in SORTS.iter().enumerate() {
                    let label = match sort {
                        desk::FenceSort::Name => w!("Name"),
                        desk::FenceSort::Type => w!("Type"),
                        desk::FenceSort::Size => w!("Size"),
                        desk::FenceSort::Modified => w!("Date modified"),
                    };
                    let flags = MF_STRING | if fence.sort == *sort { MF_CHECKED } else { MF_UNCHECKED };
                    let _ = AppendMenuW(sorts, flags, MENU_SORT_BASE + offset, label);
                }
                let _ = AppendMenuW(menu, MF_POPUP, sorts.0 as usize, w!("Sort by"));
            }
            let Ok(transparency) = CreatePopupMenu() else {
                let _ = DestroyMenu(menu);
                return;
            };
            for (offset, amount) in TRANSPARENCY.iter().enumerate() {
                let label: Vec<u16> = if *amount == 0 {
                    "Opaque\0".encode_utf16().collect()
                } else {
                    format!("{amount}%\0").encode_utf16().collect()
                };
                let flags = MF_STRING
                    | if fence.transparency == *amount {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    };
                let _ = AppendMenuW(
                    transparency,
                    flags,
                    MENU_TRANSPARENCY_BASE + offset,
                    PCWSTR(label.as_ptr()),
                );
            }
            let _ = AppendMenuW(menu, MF_POPUP, transparency.0 as usize, w!("Transparency"));
            let Ok(arrange) = CreatePopupMenu() else {
                let _ = DestroyMenu(menu);
                return;
            };
            let _ = AppendMenuW(arrange, MF_STRING, MENU_GRID, w!("Snap to grid"));
            let _ = AppendMenuW(arrange, MF_STRING, MENU_LEFT, w!("Stack left"));
            let _ = AppendMenuW(arrange, MF_STRING, MENU_RIGHT, w!("Stack right"));
            let _ = AppendMenuW(menu, MF_POPUP, arrange.0 as usize, w!("Arrange fences"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING | if fence.tabs.len() >= 8 { MF_GRAYED } else { MF_ENABLED },
                MENU_ADD_TAB,
                w!("New tab"),
            );
            let _ = AppendMenuW(menu, MF_STRING, MENU_RENAME_TAB, w!("Rename current tab…"));
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if fence.tabs.len() <= 1 || active_is_main {
                        MF_GRAYED
                    } else {
                        MF_ENABLED
                    },
                MENU_DELETE_TAB,
                w!("Remove current tab"),
            );
            let _ = AppendMenuW(menu, MF_STRING, MENU_CREATE_FENCE, w!("Create custom fence…"));
            if is_custom {
                let _ = AppendMenuW(menu, MF_STRING, MENU_RENAME_FENCE, w!("Rename fence…"));
                let _ = AppendMenuW(menu, MF_STRING, MENU_DELETE_FENCE, w!("Remove fence…"));
            }
            let mut move_names = Vec::new();
            if !move_targets.is_empty() {
                if let Ok(move_menu) = CreatePopupMenu() {
                    for (target, name) in &move_targets {
                        move_names.push(name.encode_utf16().chain(Some(0)).collect::<Vec<_>>());
                        let id = MENU_MOVE_BASE + target;
                        let _ = AppendMenuW(move_menu, MF_STRING, id, PCWSTR(move_names.last().unwrap().as_ptr()));
                    }
                    let _ = AppendMenuW(menu, MF_POPUP, move_menu.0 as usize, w!("Move item to"));
                }
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_DESK_FILE, w!("Desk settings\u{2026}"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Doggo settings\u{2026}"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_ICONS, w!("Restore desktop icons"));
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            // A menu needs its owner in front to close when the user clicks
            // elsewhere, just like the dock's menus.
            let _ = SetForegroundWindow(self.hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                None,
                self.hwnd,
                None,
            )
            .0;
            let _ = DestroyMenu(menu);
            // A menu can raise Explorer over the wallpaper; return to our
            // layer and repaint once it is gone.
            place_on_wallpaper(self.hwnd);
            self.invalidate();
            match command as usize {
                MENU_ROLL | MENU_EXPAND => self.toggle_collapsed(index),
                MENU_GRID => self.arrange(desk::Arrange::Grid),
                MENU_LEFT => self.arrange(desk::Arrange::Left),
                MENU_RIGHT => self.arrange(desk::Arrange::Right),
                MENU_TINT_NONE => self.set_tint(index, None),
                MENU_TINT_BLUE => self.set_tint(index, Some(0x2878D0)),
                MENU_TINT_VIOLET => self.set_tint(index, Some(0x8055C8)),
                MENU_TINT_PINK => self.set_tint(index, Some(0xC64F87)),
                MENU_TINT_GREEN => self.set_tint(index, Some(0x38856A)),
                MENU_TINT_AMBER => self.set_tint(index, Some(0xB78335)),
                command if (MENU_ICON_BASE..MENU_ICON_BASE + ICON_SIZES.len()).contains(&command) => {
                    self.set_icon_size(index, ICON_SIZES[command - MENU_ICON_BASE]);
                }
                command if (MENU_VIEW_BASE..MENU_VIEW_BASE + VIEWS.len()).contains(&command) => {
                    self.set_view(index, VIEWS[command - MENU_VIEW_BASE]);
                }
                command if (MENU_SORT_BASE..MENU_SORT_BASE + SORTS.len()).contains(&command) => {
                    self.set_sort(index, SORTS[command - MENU_SORT_BASE]);
                }
                command if (MENU_TRANSPARENCY_BASE..MENU_TRANSPARENCY_BASE + TRANSPARENCY.len()).contains(&command) => {
                    self.set_transparency(index, TRANSPARENCY[command - MENU_TRANSPARENCY_BASE]);
                }
                MENU_CREATE_FENCE => self.create_fence(index),
                MENU_RENAME_FENCE => self.rename_fence(index),
                MENU_DELETE_FENCE => self.delete_fence(index),
                MENU_ADD_TAB => self.add_tab(index),
                MENU_RENAME_TAB => self.rename_tab(index),
                MENU_DELETE_TAB => self.delete_tab(index),
                MENU_DESK_FILE => edit_desk(),
                MENU_SETTINGS => {
                    if let Err(error) = launch::settings(false) {
                        tray::notify(owner, "Could not open Settings", &error.message(), true);
                    }
                }
                MENU_ICONS => {
                    let _ = PostMessageW(Some(owner), WM_DOCK_DESK_ICONS, WPARAM(0), LPARAM(0));
                }
                _ => {}
            }
            if command as usize >= MENU_MOVE_BASE
                && let Some(slot) = item_slot
            {
                self.move_item_to_fence(index, slot, command as usize - MENU_MOVE_BASE);
            }
        }
    }

    fn set_tint(&mut self, index: usize, tint: Option<u32>) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        fence.tint = tint;
        self.relayout();
        self.invalidate();
        self.save();
    }

    fn set_icon_size(&mut self, index: usize, size: u32) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        fence.icon_size = size.clamp(desk::MIN_ICON_SIZE, desk::MAX_ICON_SIZE);
        self.relayout();
        self.save();
        self.invalidate();
    }

    fn set_view(&mut self, index: usize, view: desk::FenceView) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        fence.view = view;
        self.relayout();
        self.request_icons();
        self.save();
        self.invalidate();
    }

    fn set_sort(&mut self, index: usize, sort: desk::FenceSort) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        fence.sort = sort;
        self.relayout();
        self.save();
        self.invalidate();
    }

    fn set_transparency(&mut self, index: usize, transparency: u8) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        fence.transparency = transparency.min(desk::MAX_TRANSPARENCY);
        // `PlacedFence` is a render-time snapshot of saved fence settings.
        // Rebuild it now so the chosen opacity is visible on this click.
        self.relayout();
        self.apply_material();
        self.invalidate();
        self.save();
    }

    fn create_fence(&mut self, anchor: usize) {
        let Some(name) = self.prompt_for_fence(
            anchor,
            "Custom fence",
            "Create a fence",
            "Give this desktop group a name.",
            "Create fence",
        ) else {
            return;
        };
        let (x, y) = self
            .placed
            .get(anchor)
            .map_or((40, 40), |fence| (fence.x + fence.width + desk::GAP, fence.y));
        let id = format!("custom-{}-{}", crate::util::now_secs(), self.desk.fence.len());
        self.desk.fence.push(desk::Fence {
            id,
            name,
            tint: None,
            x,
            y,
            width: 320,
            icon_size: 48,
            view: desk::FenceView::Icons,
            sort: desk::FenceSort::Name,
            transparency: 0,
            height_override: None,
            scroll_y: 0,
            columns: 3,
            collapsed: false,
            category: None,
            folder: None,
            items: Vec::new(),
            tabs: vec![desk::FenceTab {
                name: "Main".into(),
                is_main: true,
                category: None,
                folder: None,
                items: Vec::new(),
            }],
            active_tab: 0,
            browse_stack: Vec::new(),
            browse_height: None,
        });
        self.relayout();
        self.save();
        self.invalidate();
    }

    fn rename_fence(&mut self, index: usize) {
        let Some(current) = self.desk.fence.get(index).map(|fence| fence.name.clone()) else {
            return;
        };
        let Some(name) = self.prompt_for_fence(
            index,
            &current,
            "Rename fence",
            "Choose a new title for this group.",
            "Save",
        ) else {
            return;
        };
        if let Some(fence) = self.desk.fence.get_mut(index) {
            fence.name = name;
            self.relayout();
            self.save();
            self.invalidate();
        }
    }

    fn delete_fence(&mut self, index: usize) {
        let Some(fence) = self.desk.fence.get(index) else {
            return;
        };
        let title = windows::core::HSTRING::from(format!(
            "Remove '{}' fence? Its files will stay on your desktop.",
            fence.name
        ));
        unsafe {
            if MessageBoxW(Some(self.hwnd), &title, w!("Doggo Desk"), MB_YESNO | MB_ICONQUESTION) != IDYES {
                return;
            }
        }
        if index < self.desk.fence.len() {
            self.desk.fence.remove(index);
            self.relayout();
            self.save();
            self.invalidate();
        }
    }

    fn prompt_for_fence(
        &self,
        index: usize,
        initial: &str,
        heading: &str,
        description: &str,
        accept: &str,
    ) -> Option<String> {
        let fence = self.placed.get(index)?;
        let tint = self.desk.fence.get(index).and_then(|f| f.tint);
        let mut origin = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut origin);
        }
        let scale = self.dpi as f32 / 96.0;
        let center = POINT {
            x: origin.left + ((fence.x + fence.width / 2) as f32 * scale).round() as i32,
            y: origin.top + ((fence.y + fence.height / 2) as f32 * scale).round() as i32,
        };
        prompt_fence_name(
            self.hwnd,
            initial,
            heading,
            description,
            accept,
            center,
            self.dpi,
            self.dark,
            tint,
        )
    }

    fn move_item_to_fence(&mut self, source: usize, slot: usize, target: usize) {
        let Some(path) = self
            .placed
            .get(source)
            .and_then(|fence| fence.items.get(slot))
            .map(|item| item.path.clone())
        else {
            return;
        };
        if target >= self.desk.fence.len() {
            return;
        }
        for fence in &mut self.desk.fence {
            fence.items.retain(|saved| !saved.eq_ignore_ascii_case(&path));
        }
        if !self.desk.fence[target]
            .items
            .iter()
            .any(|saved| saved.eq_ignore_ascii_case(&path))
        {
            self.desk.fence[target].items.push(path);
        }
        self.rescan();
        self.save();
    }

    fn launch(&self, item: &Item) {
        if let Err(error) = launch::open(&item.path) {
            tray::notify(
                self.owner,
                "Could not open",
                &format!("{}: {}", item.name, error.message()),
                true,
            );
        }
    }

    fn save(&mut self) {
        if let Err(error) = self.desk.save() {
            eprintln!("could not save the desk: {error}");
        }
    }

    fn select_tab(&mut self, index: usize, tab_index: usize) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        if tab_index >= fence.tabs.len() || tab_index == fence.active_tab {
            return;
        }
        fence.save_active_tab();
        fence.active_tab = tab_index;
        fence.load_active_tab();
        fence.browse_stack.clear();
        fence.browse_height = None;
        fence.scroll_y = 0;
        self.relayout();
        self.request_icons();
        self.save();
        self.invalidate();
    }

    fn add_tab(&mut self, index: usize) {
        if self.desk.fence.get(index).is_none_or(|f| f.tabs.len() >= 8) {
            return;
        }
        let mut suffix = self.desk.fence[index].tabs.len() + 1;
        let mut name = format!("Tab {suffix}");
        while self.desk.fence[index]
            .tabs
            .iter()
            .any(|tab| tab.name.eq_ignore_ascii_case(&name))
        {
            suffix += 1;
            name = format!("Tab {suffix}");
        }
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        fence.save_active_tab();
        fence.tabs.push(desk::FenceTab {
            name,
            is_main: false,
            category: None,
            folder: None,
            items: Vec::new(),
        });
        fence.active_tab = fence.tabs.len() - 1;
        fence.load_active_tab();
        fence.browse_stack.clear();
        fence.browse_height = None;
        fence.scroll_y = 0;
        self.relayout();
        self.save();
        self.invalidate();
        self.begin_tab_edit(index, self.desk.fence[index].active_tab);
    }

    fn rename_tab(&mut self, index: usize) {
        let Some(tab) = self.desk.fence.get(index).map(|f| f.active_tab) else {
            return;
        };
        self.begin_tab_edit(index, tab);
    }

    fn begin_tab_edit(&mut self, fence_index: usize, tab_index: usize) {
        if let Some(existing) = self.tab_edit.as_ref().map(|edit| edit.hwnd) {
            self.finish_tab_edit(existing, true);
        }
        let (name, tint, tab_x, tab_width, fence_x, fence_y) = {
            let (Some(placed), Some(tab)) = (
                self.placed.get(fence_index),
                self.desk.fence.get(fence_index).and_then(|f| f.tabs.get(tab_index)),
            ) else {
                return;
            };
            let mut tab_x = 7.0_f32;
            for prior in placed.tabs.iter().take(tab_index) {
                tab_x += (prior.chars().count() as f32 * 7.5 + 24.0).clamp(50.0, 150.0) + 4.0;
            }
            (
                tab.name.clone(),
                self.desk.fence[fence_index].tint,
                tab_x,
                (tab.name.chars().count() as f32 * 7.5 + 24.0).clamp(50.0, 150.0),
                placed.x,
                placed.y,
            )
        };
        let mut origin = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut origin);
        }
        let scale = self.dpi as f32 / 96.0;
        let px = |value: f32| (value * scale).round() as i32;
        let left = origin.left + px(fence_x as f32 + tab_x);
        let top = origin.top + px(fence_y as f32 + desk::TITLE_HEIGHT as f32 + 3.0);
        let width = px(tab_width).max(1);
        let height = px((desk::TAB_HEIGHT - 6) as f32).max(1);
        let mut colors = prompt_colors(self.dark, tint);
        colors.input = if self.dark {
            prompt_tint(0x2b2b2b, tint, 0.32)
        } else {
            prompt_tint(0xe3e6eb, tint, 0.09)
        };
        let background_brush =
            unsafe { CreateSolidBrush(windows::Win32::Foundation::COLORREF(prompt_colorref(colors.input))) };
        let mut data = Box::new(InlineEditData {
            owner: self.hwnd,
            edit: HWND::default(),
            colors,
            background_brush,
            scale,
            ended: false,
        });
        let hwnd = unsafe {
            let instance = match windows::Win32::System::LibraryLoader::GetModuleHandleW(None) {
                Ok(instance) => instance,
                Err(_) => return,
            };
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(inline_tab_proc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_IBEAM).unwrap_or_default(),
                lpszClassName: INLINE_TAB_CLASS,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return;
            }
            match CreateWindowExW(
                WS_EX_TOOLWINDOW,
                INLINE_TAB_CLASS,
                w!(""),
                WS_POPUP,
                left,
                top,
                width,
                height,
                Some(self.hwnd),
                None,
                Some(instance.into()),
                Some((&mut *data as *mut InlineEditData).cast()),
            ) {
                Ok(hwnd) => hwnd,
                Err(_) => return,
            }
        };
        let edit = data.edit;
        let edit_data = data;
        if edit.0.is_null() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return;
        }
        self.tab_edit = Some(TabEdit {
            hwnd,
            edit,
            fence: fence_index,
            tab: tab_index,
            original: name.clone(),
            _data: edit_data,
        });
        unsafe {
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, px(6.0), px(6.0));
            let _ = SetWindowRgn(hwnd, Some(region), true);
            let text: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let _ = SetWindowTextW(edit, PCWSTR(text.as_ptr()));
            let _ = SendMessageW(edit, 0x00B1, Some(WPARAM(0)), Some(LPARAM(-1)));
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(edit));
        }
    }

    fn finish_tab_edit(&mut self, hwnd: HWND, commit: bool) {
        let Some(edit) = self.tab_edit.take().filter(|edit| edit.hwnd == hwnd) else {
            return;
        };
        if commit {
            let mut buffer = [0u16; 256];
            unsafe {
                let _ = GetWindowTextW(edit.edit, &mut buffer);
            }
            let value =
                String::from_utf16_lossy(&buffer[..buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len())]);
            let value = value.trim();
            if !value.is_empty()
                && let Some(tab) = self
                    .desk
                    .fence
                    .get_mut(edit.fence)
                    .and_then(|f| f.tabs.get_mut(edit.tab))
            {
                tab.name = value.chars().take(24).collect();
                self.relayout();
                self.save();
            }
        } else if let Some(tab) = self
            .desk
            .fence
            .get_mut(edit.fence)
            .and_then(|f| f.tabs.get_mut(edit.tab))
        {
            tab.name = edit.original;
        }
        unsafe {
            let _ = DestroyWindow(edit.hwnd);
        }
        self.invalidate();
    }

    fn delete_tab(&mut self, index: usize) {
        let Some(fence) = self.desk.fence.get_mut(index) else {
            return;
        };
        if fence.tabs.len() <= 1 || fence.tabs.get(fence.active_tab).is_some_and(|tab| tab.is_main) {
            return;
        }
        fence.tabs.remove(fence.active_tab);
        fence.active_tab = fence.active_tab.min(fence.tabs.len() - 1);
        fence.load_active_tab();
        fence.browse_stack.clear();
        fence.browse_height = None;
        fence.scroll_y = 0;
        self.relayout();
        self.request_icons();
        self.save();
        self.invalidate();
    }
}

pub struct DeskWindow {
    hwnd: HWND,
    state: Rc<RefCell<State>>,
}

impl DeskWindow {
    pub fn with_desk(owner: HWND, mut desk: Desk, dark: bool, _backdrop: Backdrop) -> Result<Self> {
        // If the previous run ended without restoring the icons, do it before
        // touching the desktop again.
        desk::native::restore();

        let (left, top, width, height, dpi) = monitor();
        let items = desk::collect(&desk::desktop_dirs());
        let mut placed = desk.layout(&items);
        // A hand-edited file may put two fences on top of each other. Move the
        // later one to the nearest free spot and remember it.
        let scale = dpi as f32 / 96.0;
        if desk::separate(
            &mut placed,
            (width as f32 / scale).round() as i32,
            (height as f32 / scale).round() as i32,
        ) {
            for fence in &placed {
                if let Some(saved) = desk.fence.get_mut(fence.index) {
                    saved.x = fence.x;
                    saved.y = fence.y;
                }
            }
            if let Err(error) = desk.save() {
                eprintln!("could not save the desk: {error}");
            }
        }
        let show_labels = desk.show_labels;
        let watched = desk.watch_dirs();
        let state = Rc::new(RefCell::new(State {
            hwnd: HWND::default(),
            owner,
            desk,
            items,
            placed,
            icons: HashMap::new(),
            loader: None,
            watch: None,
            watched,
            // The renderer starts at 96 DPI, but fences, hit tests and the
            // window region are all in DIPs. Tell it the monitor DPI before
            // the first paint, or it draws at 1:1 and the region clips most
            // of every card away.
            renderer: {
                let mut renderer = render::Renderer::new()?;
                renderer.set_dpi(dpi);
                renderer
            },
            palette: Palette::new(dark, true),
            dark,
            show_labels,
            dpi,
            hover: None,
            drag: None,
            tab_edit: None,
            resize: None,
            scroll_drag: None,
            scroll_targets: HashMap::new(),
            hid_icons: false,
            heal_ticks: 0,
            #[cfg(test)]
            painted: false,
        }));

        let hwnd = unsafe {
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                CLASS_NAME,
                w!("Doggo Desk"),
                WS_POPUP,
                left,
                top,
                width,
                height,
                None,
                None,
                Some(instance.into()),
                Some(Rc::as_ptr(&state).cast()),
            )?;
            // Keep it directly above the wallpaper, below every normal
            // window, so the fences are visible without floating over apps.
            place_on_wallpaper(hwnd);
            let icon_px = ((desk::MAX_ICON_SIZE as f32 * dpi as f32 / 96.0).round() as i32).max(16);
            {
                let mut s = state.borrow_mut();
                s.hwnd = hwnd;
                s.loader = Some(IconLoader::spawn(hwnd, icon_px)?);
                s.watch = Watch::spawn(hwnd, s.watched.clone());
                s.apply_material();
                s.apply_region();
                s.request_icons();
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // Showing a popup can raise it; return to the wallpaper layer.
            place_on_wallpaper(hwnd);
            // Explorer can raise its desktop or show its icons again after a
            // refresh; check a few times a second and repair both.
            // Explorer can raise its desktop when it gets focus. Repair this
            // quickly enough that fences don't visibly vanish on a desktop click.
            let _ = SetTimer(Some(hwnd), HEAL_TIMER, 100, None);
            // Paint once before hiding Explorer's icons, so the desk appears
            // in the same place the icons disappeared from.
            let _ = UpdateWindow(hwnd);
            {
                let mut s = state.borrow_mut();
                if s.desk.hide_icons {
                    s.hid_icons = desk::native::hide();
                }
            }
            hwnd
        };
        Ok(Self { hwnd, state })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Replaces the saved layout and shows or restores the native icons to
    /// match it, without reopening the window.
    pub fn set_config(&mut self, desk: Desk) {
        let Ok(mut s) = self.state.try_borrow_mut() else {
            return;
        };
        let hide = desk.hide_icons;
        s.desk = desk;
        s.apply_material();
        s.show_labels = s.desk.show_labels;
        // A portal fence watches an extra folder, so the watcher must be
        // rebuilt when the set of folders changes.
        let dirs = s.desk.watch_dirs();
        if dirs != s.watched {
            s.watch = Watch::spawn(s.hwnd, dirs.clone());
            s.watched = dirs;
        }
        s.rescan();
        if hide && !s.hid_icons {
            s.hid_icons = desk::native::hide();
        } else if !hide && s.hid_icons {
            desk::native::restore();
            s.hid_icons = false;
        }
    }

    pub fn apply_theme(&mut self, dark: bool, _backdrop: Backdrop) {
        if let Ok(mut s) = self.state.try_borrow_mut() {
            s.dark = dark;
            s.apply_material();
            s.invalidate();
        }
    }

    /// Re-reads the desktop after a display, theme or Explorer change.
    pub fn refresh(&mut self) {
        if let Ok(mut s) = self.state.try_borrow_mut() {
            // A restarted Explorer may create its desktop above us; return to
            // the wallpaper layer before repainting.
            place_on_wallpaper(s.hwnd);
            s.rescan();
        }
    }

    /// Restores the fence layer after another app or Explorer takes focus.
    pub fn ensure_wallpaper_layer(&mut self) {
        if let Ok(s) = self.state.try_borrow() {
            unsafe {
                if !IsWindowVisible(s.hwnd).as_bool() {
                    let _ = ShowWindow(s.hwnd, SW_SHOWNOACTIVATE);
                    let _ = InvalidateRect(Some(s.hwnd), None, false);
                }
            }
            if !above_shell(s.hwnd) {
                place_on_wallpaper(s.hwnd);
                s.invalidate();
            }
        }
    }

    /// The tray escape hatch: show Explorer's icons and stop hiding them, so
    /// they stay visible across the next reload and restart.
    pub fn show_icons(&mut self) {
        if let Ok(mut s) = self.state.try_borrow_mut() {
            s.desk.hide_icons = false;
            s.hid_icons = false;
            s.save();
        }
        desk::native::restore();
    }
}

impl Drop for DeskWindow {
    fn drop(&mut self) {
        // restore only acts when Doggo left its marker behind.
        desk::native::restore();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The primary monitor's rectangle and scale. Explorer shows its desktop icons
/// on the primary monitor, so the first version of the desk lives there too.
fn monitor() -> (i32, i32, i32, i32, u32) {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint};
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let rect = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcMonitor
        } else {
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        };
        let dpi = GetDpiForSystem();
        (rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, dpi)
    }
}

fn reflow(state: &mut State) {
    let (left, top, width, height, dpi) = monitor();
    state.dpi = dpi;
    state.renderer.set_dpi(dpi);
    state.renderer.resize(width as u32, height as u32);
    unsafe {
        let _ = SetWindowPos(
            state.hwnd,
            None,
            left,
            top,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    place_on_wallpaper(state.hwnd);
    state.rescan();
}

/// The window the desk should be inserted below: the window directly above
/// Explorer's desktop.
fn wallpaper_insert(hwnd: HWND) -> Option<HWND> {
    unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return None;
        }
        match GetWindow(shell, GW_HWNDPREV).ok() {
            // Already directly above the desktop.
            Some(above) if above == hwnd => Some(hwnd),
            Some(above) => Some(above),
            // The desktop is the topmost window.
            None => Some(HWND_TOP),
        }
    }
}

/// Keeps the desk directly above Explorer's desktop and below every normal
/// window. `HWND_BOTTOM` alone would put it beneath the wallpaper, where
/// nothing it draws can be seen.
fn place_on_wallpaper(hwnd: HWND) {
    unsafe {
        let Some(insert) = wallpaper_insert(hwnd) else {
            return;
        };
        if insert == hwnd {
            return;
        }
        let _ = SetWindowPos(
            hwnd,
            Some(insert),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

/// Whether the desk is still directly above Explorer's desktop. Explorer can
/// raise itself over the desk when it refreshes or opens one of its own menus;
/// the timer asks this question and puts the desk back when the answer is no.
fn above_shell(hwnd: HWND) -> bool {
    unsafe {
        let shell = GetShellWindow();
        !shell.is_invalid() && GetWindow(hwnd, GW_HWNDNEXT).ok() == Some(shell)
    }
}

/// Opens `desk.toml` for hand editing, falling back to Notepad when Windows
/// has no editor registered for `.toml` files.
fn edit_desk() {
    let path = Desk::path();
    let path = path.to_string_lossy();
    if launch::open(&path).is_err() {
        let _ = launch::notepad(&path);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        if msg == WM_NCHITTEST {
            return LRESULT(HTCLIENT as isize);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        if msg == WM_WINDOWPOSCHANGING {
            // Stay on the wallpaper layer even when Windows offers to raise
            // us after a click. The region keeps empty space clickable.
            let pos = &mut *(lp.0 as *mut WINDOWPOS);
            if !pos.flags.contains(SWP_NOZORDER) {
                match wallpaper_insert(hwnd) {
                    Some(insert) if insert != hwnd => pos.hwndInsertAfter = insert,
                    Some(_) => pos.flags |= SWP_NOZORDER,
                    None => {}
                }
            }
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_PAINT {
            let mut paint = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut paint);
            let pointer = state_pointer(hwnd);
            if !pointer.is_null()
                && let Ok(mut s) = (*pointer).try_borrow_mut()
            {
                s.paint();
            }
            let _ = EndPaint(hwnd, &paint);
            return LRESULT(0);
        }

        let pointer = state_pointer(hwnd);
        if pointer.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let Ok(mut s) = (*pointer).try_borrow_mut() else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        // The desk window deliberately does not activate when clicked, so an
        // inline tab editor may keep keyboard focus while the user clicks
        // somewhere else inside the container. Treat any click on the desk as
        // leaving the tab name and commit it before processing that click.
        if matches!(
            msg,
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP
        ) && let Some(editor) = s.tab_edit.as_ref().map(|edit| edit.hwnd)
        {
            s.finish_tab_edit(editor, true);
        }
        match msg {
            WM_TAB_EDIT_DONE => {
                s.finish_tab_edit(HWND(wp.0 as _), lp.0 != 0);
                LRESULT(0)
            }
            WM_DESK_ICONS => {
                let mut batches = Vec::new();
                if let Some(loader) = &s.loader {
                    while let Ok(batch) = loader.events.try_recv() {
                        batches.push(batch);
                    }
                }
                for batch in batches {
                    for (path, thumbnail, pixels) in batch {
                        s.icons.insert(icon_key(&path, thumbnail), pixels);
                    }
                }
                s.invalidate();
                LRESULT(0)
            }
            watch::WM_DESK_CHANGED => {
                s.rescan();
                LRESULT(0)
            }
            WM_TIMER if wp.0 == HEAL_TIMER => {
                // Explorer sometimes puts itself back over the desk or hides
                // the desk's window; repair both without waiting for a restart.
                // Nothing is redrawn when everything is still right.
                if !IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    place_on_wallpaper(hwnd);
                    s.invalidate();
                } else if !above_shell(hwnd) {
                    place_on_wallpaper(hwnd);
                    s.invalidate();
                }
                // Asking the shell to hide icons waits for it, so check every
                // other second rather than every tick.
                s.heal_ticks = s.heal_ticks.wrapping_add(1);
                if s.heal_ticks % 4 == 0 && s.desk.hide_icons && !desk::native::hidden() && desk::native::hide() {
                    s.hid_icons = true;
                }
                LRESULT(0)
            }
            WM_TIMER if wp.0 == SCROLL_TIMER => {
                s.animate_scroll();
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let packed = lp.0 as u32;
                let mut point = POINT {
                    x: (packed as u16 as i16) as i32,
                    y: ((packed >> 16) as u16 as i16) as i32,
                };
                let _ = ScreenToClient(hwnd, &mut point);
                let scale = 96.0 / s.dpi as f32;
                if let Some(hit) = render::hit(&s.placed, point.x as f32 * scale, point.y as f32 * scale) {
                    let index = match hit {
                        Hit::Fence(index) | Hit::Item(index, _) | Hit::Resize(index) | Hit::Scrollbar(index) => {
                            Some(index)
                        }
                        _ => None,
                    };
                    if let Some(index) = index {
                        s.scroll_fence(index, (wp.0 >> 16) as i16);
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = s.dip(lp);
                let hit = render::hit(&s.placed, x, y);
                if hit != s.hover {
                    s.hover = hit;
                    s.invalidate();
                }
                // Ask for WM_MOUSELEAVE so a hover highlight does not stick.
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);

                if let Some(drag) = &mut s.drag {
                    let dx = x - drag.start.0;
                    let dy = y - drag.start.1;
                    if dx.abs() > 2.0 || dy.abs() > 2.0 {
                        drag.moved = true;
                    }
                }
                // Copy the drag out before borrowing the whole state to move
                // the fence and rebuild the region.
                let drag = s
                    .drag
                    .as_ref()
                    .map(|d| (d.moved, d.fence, d.origin, x - d.start.0, y - d.start.1));
                if let Some((true, fence, origin, dx, dy)) = drag {
                    let (window_w, window_h) = s.dip_size();
                    let width = s.placed[fence].width;
                    let height = s.placed[fence].height;
                    let x = (origin.0 as f32 + dx).clamp(0.0, (window_w - width as f32).max(0.0));
                    let y = (origin.1 as f32 + dy).clamp(0.0, (window_h - height as f32).max(0.0));
                    let (x, y) = (x.round() as i32, y.round() as i32);
                    // Do not let one fence cover another: keep the last spot
                    // where the drag was clear.
                    let clear = !s.placed.iter().enumerate().any(|(other, placed)| {
                        other != fence
                            && desk::overlaps((x, y, width, height), (placed.x, placed.y, placed.width, placed.height))
                    });
                    if clear {
                        s.desk.fence[fence].x = x;
                        s.desk.fence[fence].y = y;
                        s.placed[fence].x = x;
                        s.placed[fence].y = y;
                        s.apply_region();
                        s.invalidate();
                    }
                }
                let resize = s.resize.as_ref().map(|r| {
                    (
                        r.fence,
                        r.start,
                        r.origin_width,
                        r.origin_height,
                        x - r.start.0,
                        y - r.start.1,
                    )
                });
                if let Some((index, _start, origin_width, origin_height, dx, dy)) = resize {
                    let (screen_width, screen_height) = s.dip_size();
                    let fence = &s.placed[index];
                    let max_width = (screen_width.round() as i32 - fence.x).clamp(desk::MIN_WIDTH, desk::MAX_WIDTH);
                    let next_width = (origin_width as f32 + dx).round() as i32;
                    let next_width = next_width.clamp(desk::MIN_WIDTH, max_width);
                    let columns = ((next_width - desk::PAD * 2) / desk::CELL).clamp(1, desk::MAX_COLUMNS as i32) as u32;
                    let saved = &s.desk.fence[index];
                    let min_height = desk::MIN_HEIGHT;
                    let max_height = (screen_height.round() as i32 - saved.y).clamp(min_height, desk::MAX_HEIGHT);
                    let next_height = ((origin_height as f32 + dy).round() as i32).clamp(min_height, max_height);
                    let rectangle = (saved.x, saved.y, next_width, next_height);
                    let clear = !s.placed.iter().enumerate().any(|(other, placed)| {
                        other != index && desk::overlaps(rectangle, (placed.x, placed.y, placed.width, placed.height))
                    });
                    let height_override = Some(next_height);
                    if clear
                        && (s.desk.fence[index].width != next_width
                            || s.desk.fence[index].columns != columns
                            || s.desk.fence[index].height_override != height_override)
                    {
                        s.desk.fence[index].width = next_width;
                        s.desk.fence[index].columns = columns;
                        s.desk.fence[index].height_override = height_override;
                        s.relayout();
                        s.invalidate();
                    }
                }
                let scroll_drag = s
                    .scroll_drag
                    .as_ref()
                    .map(|drag| (drag.fence, drag.start_y, drag.origin_scroll));
                if let Some((index, start_y, origin_scroll)) = scroll_drag {
                    s.drag_scrollbar(index, start_y, origin_scroll, y);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if s.hover.take().is_some() {
                    s.invalidate();
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if matches!(s.hover, Some(Hit::Resize(_))) {
                    if let Ok(cursor) = LoadCursorW(None, IDC_SIZENWSE) {
                        SetCursor(Some(cursor));
                    }
                    return LRESULT(1);
                }
                if matches!(
                    s.hover,
                    Some(
                        Hit::Item(..)
                            | Hit::Title(_)
                            | Hit::NavigateBack(_)
                            | Hit::Collapse(_)
                            | Hit::Tab(..)
                            | Hit::TabBar(_)
                            | Hit::NewTab(_)
                            | Hit::Scrollbar(_)
                    )
                ) {
                    if let Ok(cursor) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(Some(cursor));
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = s.dip(lp);
                match render::hit(&s.placed, x, y) {
                    Some(Hit::Collapse(index)) => s.toggle_collapsed(index),
                    Some(Hit::NavigateBack(index)) => s.navigate_back(index),
                    Some(Hit::Tab(index, tab)) => s.select_tab(index, tab),
                    Some(Hit::NewTab(index)) => s.add_tab(index),
                    Some(Hit::Resize(index)) => {
                        s.resize = Some(Resize {
                            fence: index,
                            start: (x, y),
                            origin_width: s.placed[index].width,
                            origin_height: s.placed[index].height,
                        });
                        let _ = SetCapture(hwnd);
                    }
                    Some(Hit::Scrollbar(index)) => {
                        s.scroll_drag = Some(ScrollDrag {
                            fence: index,
                            start_y: y,
                            origin_scroll: s.placed[index].scroll_y,
                        });
                        let _ = SetCapture(hwnd);
                    }
                    Some(Hit::Title(index) | Hit::Fence(index)) => {
                        let fence = &s.desk.fence[index];
                        s.drag = Some(Drag {
                            fence: index,
                            start: (x, y),
                            origin: (fence.x, fence.y),
                            moved: false,
                        });
                        let _ = SetCapture(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = s.dip(lp);
                match render::hit(&s.placed, x, y) {
                    Some(Hit::Item(index, slot)) => {
                        if let Some(item) = s.placed.get(index).and_then(|fence| fence.items.get(slot)).cloned() {
                            if item.is_dir {
                                s.enter_folder(index, item.path);
                            } else {
                                s.launch(&item);
                            }
                        }
                    }
                    Some(Hit::Title(index)) => s.toggle_collapsed(index),
                    Some(Hit::Tab(index, _)) => s.rename_tab(index),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if s.scroll_drag.take().is_some() {
                    let _ = ReleaseCapture();
                } else if s.resize.take().is_some() {
                    let _ = ReleaseCapture();
                    s.save();
                } else if let Some(drag) = s.drag.take() {
                    let _ = ReleaseCapture();
                    if drag.moved {
                        s.save();
                    } else {
                    }
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let (x, y) = s.dip(lp);
                let hit = render::hit(&s.placed, x, y);
                let (index, item) = match hit {
                    Some(
                        Hit::Fence(index)
                        | Hit::Title(index)
                        | Hit::NavigateBack(index)
                        | Hit::Collapse(index)
                        | Hit::Tab(index, _)
                        | Hit::TabBar(index)
                        | Hit::NewTab(index)
                        | Hit::Resize(index)
                        | Hit::Scrollbar(index),
                    ) => (Some(index), None),
                    Some(Hit::Item(index, slot)) => (Some(index), Some(slot)),
                    None => (None, None),
                };
                if let Some(index) = index {
                    s.fence_menu(index, item);
                }
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                s.scroll_drag = None;
                if s.resize.take().is_some() {
                    s.save();
                }
                if let Some(drag) = s.drag.take()
                    && drag.moved
                {
                    s.save();
                }
                LRESULT(0)
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
                reflow(&mut s);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xFFFF) as u32;
                s.dpi = dpi;
                s.renderer.set_dpi(dpi);
                let rect = &*(lp.0 as *const RECT);
                let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
                s.renderer.resize(width as u32, height as u32);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                place_on_wallpaper(hwnd);
                s.apply_region();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe fn state_pointer(hwnd: HWND) -> *const RefCell<State> {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State> }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Backdrop;
    use windows::Win32::Graphics::Gdi::{
        CreateRectRgn, DeleteObject, GetDC, GetPixel, GetWindowRgn, PtInRegion, ReleaseDC,
    };
    use windows::Win32::UI::HiDpi::GetDpiForWindow;

    /// Creates a real desk window for a moment and checks that its region is
    /// the fence and nothing else, which is what lets clicks between fences
    /// reach the desktop. Ignored: it needs an interactive desktop.
    #[test]
    #[ignore = "needs an interactive desktop"]
    fn the_window_region_is_the_fences() {
        let mut desk = Desk::default_layout();
        desk.fence.truncate(1);
        let fence = &mut desk.fence[0];
        fence.x = 120;
        fence.y = 90;
        fence.width = 320;
        fence.columns = 4;
        fence.category = None;
        fence.items.clear();
        let (fence_x, fence_y) = (fence.x, fence.y);

        let window = DeskWindow::with_desk(HWND::default(), desk, false, Backdrop::Solid).expect("desk window");
        let hwnd = window.hwnd();
        let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
        let inside = ((120.0 + 12.0) * scale, (90.0 + 12.0) * scale);
        let outside = ((120.0 - 20.0) * scale, (90.0 - 20.0) * scale);

        unsafe {
            let region = CreateRectRgn(0, 0, 0, 0);
            assert_ne!(GetWindowRgn(hwnd, region).0, 0, "the desk should have a window region");
            assert!(PtInRegion(region, inside.0 as i32, inside.1 as i32).as_bool());
            assert!(!PtInRegion(region, outside.0 as i32, outside.1 as i32).as_bool());
            let _ = DeleteObject(region.into());

            // The window has a region, so a frame must have been drawn through
            // the Direct2D path the desk uses at runtime.
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = UpdateWindow(hwnd);
            let painted = window.state.try_borrow().map(|s| s.painted).unwrap_or(false);
            assert!(painted, "the fence should have been drawn");

            // The renderer must have been told the monitor DPI. If it draws at
            // 1:1, the card lands outside the DIP-based region and this point
            // inside the title bar shows the clear colour instead.
            let dc = GetDC(Some(hwnd));
            let inside_title = GetPixel(
                dc,
                ((fence_x as f32 + 6.0) * scale) as i32,
                ((fence_y as f32 + 6.0) * scale) as i32,
            );
            let _ = ReleaseDC(Some(hwnd), dc);
            assert_ne!(
                inside_title.0 & 0x00FF_FFFF,
                0x00FF_FFFF,
                "the card was drawn at the wrong DPI"
            );

            // The desk sits directly above the shell's desktop window and
            // below every normal window, so the fences cover the wallpaper
            // without floating over apps.
            let shell = GetShellWindow();
            println!(
                "desk={hwnd:?} shell={shell:?} above={:?} below={:?}",
                GetWindow(hwnd, GW_HWNDPREV).ok(),
                GetWindow(hwnd, GW_HWNDNEXT).ok(),
            );
            assert_eq!(
                GetWindow(hwnd, GW_HWNDNEXT).ok(),
                Some(shell),
                "the desk should sit directly above the desktop"
            );
        }
    }

    /// Drives the icon worker the way the message loop does, without a running
    /// app, to prove the desk actually receives bitmaps for its items.
    #[test]
    #[ignore = "needs an interactive desktop"]
    fn the_desk_receives_icon_bitmaps() {
        use std::time::{Duration, Instant};
        let window = DeskWindow::with_desk(HWND::default(), Desk::default_layout(), false, Backdrop::Solid)
            .expect("desk window");
        let items = window.state.borrow().items.len();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut msg = MSG::default();
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                let _ = unsafe { TranslateMessage(&msg) };
                unsafe { DispatchMessageW(&msg) };
            }
            let icons = window.state.borrow().icons.len();
            if icons >= items || Instant::now() > deadline {
                println!("items={items} icons={icons}");
                assert!(items > 0, "the desk has no items to draw");
                assert_eq!(icons, items, "the desk never received every icon");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
