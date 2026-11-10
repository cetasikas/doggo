//! Parses shortcuts such as `"Alt+Space"` into `RegisterHotKey` arguments.

use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub struct Hotkey {
    pub modifiers: HOT_KEY_MODIFIERS,
    pub vk: u32,
}

pub fn parse(spec: &str) -> Result<Hotkey, String> {
    let mut modifiers = MOD_NOREPEAT;
    let mut vk = None;
    for part in spec.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "alt" => modifiers |= MOD_ALT,
            "ctrl" | "control" => modifiers |= MOD_CONTROL,
            "shift" => modifiers |= MOD_SHIFT,
            "win" | "super" | "meta" => modifiers |= MOD_WIN,
            key => {
                if vk.is_some() {
                    return Err(format!("shortcut \"{spec}\" has more than one non-modifier key"));
                }
                vk = Some(key_code(key).ok_or_else(|| format!("unknown key \"{part}\" in shortcut \"{spec}\""))?);
            }
        }
    }
    let vk = vk.ok_or_else(|| format!("shortcut \"{spec}\" has no non-modifier key"))?;
    Ok(Hotkey { modifiers, vk })
}

fn key_code(key: &str) -> Option<u32> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u32);
        }
        let vk = match c {
            '`' => VK_OEM_3,
            '-' => VK_OEM_MINUS,
            '=' => VK_OEM_PLUS,
            '[' => VK_OEM_4,
            ']' => VK_OEM_6,
            '\\' => VK_OEM_5,
            ';' => VK_OEM_1,
            '\'' => VK_OEM_7,
            ',' => VK_OEM_COMMA,
            '.' => VK_OEM_PERIOD,
            '/' => VK_OEM_2,
            _ => return None,
        };
        return Some(vk.0 as u32);
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&n).then(|| (VK_F1.0 + n - 1) as u32);
    }
    let vk = match key {
        "space" => VK_SPACE,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "esc" | "escape" => VK_ESCAPE,
        "backquote" | "tilde" => VK_OEM_3,
        "insert" => VK_INSERT,
        "delete" => VK_DELETE,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        _ => return None,
    };
    Some(vk.0 as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts_and_rejects_invalid_keys() {
        let hk = parse("Alt+Space").unwrap();
        assert_eq!(hk.modifiers, MOD_ALT | MOD_NOREPEAT);
        assert_eq!(hk.vk, VK_SPACE.0 as u32);

        let hk = parse("ctrl + shift + k").unwrap();
        assert_eq!(hk.modifiers, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT);
        assert_eq!(hk.vk, 'K' as u32);

        assert_eq!(parse("Win+F12").unwrap().vk, VK_F12.0 as u32);
        assert!(parse("Alt").is_err());
        assert!(parse("Alt+Foo").is_err());
        assert!(parse("A+B").is_err());
    }
}

// The hook only tracks physical input. Win down reaches Windows immediately,
// so shell combinations retain their normal timing and semantics.
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_WIN_TAP: u32 = WM_APP + 30;

#[derive(Default)]
struct TapState {
    held: [bool; 2],
    chord: bool,
}
impl TapState {
    fn event(&mut self, vk: u32, down: bool, other_held: bool) -> bool {
        let side = match vk {
            0x5b => Some(0),
            0x5c => Some(1),
            _ => None,
        };
        let Some(side) = side else {
            if down && self.held.iter().any(|&v| v) {
                self.chord = true;
            }
            return false;
        };
        if down {
            if self.held[side] {
                return false;
            } // auto-repeat never rearms
            if self.held.iter().any(|&v| v) {
                self.chord = true;
            } else {
                self.chord = other_held;
            }
            self.held[side] = true;
            false
        } else {
            let was_held = self.held[side];
            self.held[side] = false;
            was_held && !self.chord && !self.held.iter().any(|&v| v)
        }
    }
}
thread_local! {
    static WIN_TAP: RefCell<(HWND, TapState)> = RefCell::new((HWND::default(), TapState::default()));
}
pub struct WinTapHook(HHOOK);
impl WinTapHook {
    pub fn install(hwnd: HWND) -> windows::core::Result<Self> {
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(win_tap_proc),
                Some(windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?.into()),
                0,
            )?
        };
        WIN_TAP.with(|s| *s.borrow_mut() = (hwnd, TapState::default()));
        Ok(Self(hook))
    }
}
impl Drop for WinTapHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
        WIN_TAP.with(|s| *s.borrow_mut() = (HWND::default(), TapState::default()));
    }
}
unsafe extern "system" fn win_tap_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        if !key.flags.contains(LLKHF_INJECTED) {
            let down = matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wp.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            if down || up {
                let target = WIN_TAP.with(|s| {
                    let mut s = s.borrow_mut();
                    // Win+L can move to the secure desktop before key-up reaches us.
                    // Async state still describes the previous event inside this hook.
                    if down && matches!(key.vkCode, 0x5b | 0x5c) && unsafe { GetAsyncKeyState(key.vkCode as i32) } >= 0
                    {
                        let side = usize::from(key.vkCode == 0x5c);
                        s.1.held[side] = false;
                    }
                    let other_held = down
                        && matches!(key.vkCode, 0x5b | 0x5c)
                        && (1..=254)
                            .filter(|v| !matches!(v, 0x5b | 0x5c))
                            .any(|v| unsafe { GetAsyncKeyState(v) } < 0);
                    s.1.event(key.vkCode, down, other_held).then_some(s.0)
                });
                if let Some(hwnd) = target {
                    // A harmless shell menu-mask key prevents Start on the following
                    // physical Win-up. Both Win events still reach the shell.
                    let input = |flags| INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(0xe8),
                                dwFlags: flags,
                                ..Default::default()
                            },
                        },
                    };
                    let mask = [input(KEYBD_EVENT_FLAGS(0)), input(KEYEVENTF_KEYUP)];
                    if unsafe { SendInput(&mask, size_of::<INPUT>() as i32) } == 2 {
                        unsafe {
                            let _ = PostMessageW(Some(hwnd), WM_WIN_TAP, WPARAM(0), LPARAM(0));
                        }
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

#[cfg(test)]
mod tap_tests {
    use super::*;
    #[test]
    fn standalone_release_and_repeats() {
        for win in [0x5b, 0x5c] {
            let mut s = TapState::default();
            assert!(!s.event(win, false, false));
            assert!(!s.event(win, true, false));
            for _ in 0..20 {
                assert!(!s.event(win, true, false));
            }
            assert!(s.event(win, false, false));
            assert!(!s.event(win, false, false));
        }
    }
    #[test]
    fn chords_and_preheld_keys_never_toggle() {
        for key in [0x45, 0x52, 0x44, 0x4c, 0x10, 0x11, 0x12, 0x5c] {
            let mut s = TapState::default();
            s.event(0x5b, true, false);
            s.event(key, true, false);
            s.event(key, false, false);
            assert!(!s.event(0x5b, false, false));
        }
        let mut s = TapState::default();
        s.event(0x5b, true, true);
        assert!(!s.event(0x5b, false, false));
        s.event(0x5b, true, false);
        assert!(s.event(0x5b, false, false));
    }
}
