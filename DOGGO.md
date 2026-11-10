# Doggo personal fork

Doggo is a personal fork of [Wisp](https://github.com/SumirSeth/wisp), based on upstream commit `4d86a783cf4b9c73202e72398c01b68b54f12267` (verified against upstream HEAD on November 10, 2026). Original code and icons are copyright Sumir Seth and contributors under the MIT license; see LICENSE. Upstream dock pinning was contributed by @SorbetMango. This fork retains Rust, Win32, Direct2D, the existing settings app, and the dock/launcher morph.

## Run and configure

Run `dist\Doggo\doggo.exe` after building, with `doggo-settings.exe` beside it. Stop any other dock/taskbar replacement and Windows-key remapping script first.

Open Settings from the notification icon. **Windows key dock** is optional and defaults off; the configured shortcut defaults to Alt+Space. Settings apply live. The saved `windows_key_search` key remains compatible with earlier Doggo builds.

Doggo uses `%APPDATA%\doggo` for config, history, surfaces and `dock-pins.toml`. Its instance mutex, window class and **Doggo** startup value are separate from Wisp. Enable **Start with Windows** only if desired; building and extracting the app do not enable it. Upstream binary updates are disabled so they cannot replace the fork.

## What changed

- Reused upstream's unreleased persistent pinning, application matching, launch/switch behavior, and close-window menu.
- Drag a pinned icon horizontally over another pinned icon to move it. The other pins shift; release to save. A normal click below Windows' drag threshold retains launch/switch behavior. Dragging does not launch an app. To reorder pins beyond the visible area, Shift+scroll the dock.
- Added an optional native low-level keyboard hook. Physical Windows-key down and all combinations pass through; a standalone release reveals and focuses the dock. Typing a printable character opens the existing search animation and keeps that character. Escape, another standalone tap, or switching apps dismisses the keyboard dock. The configured shortcut still opens search directly. Repeats do not rearm it. A menu-mask input on a standalone release prevents Start. Injected input is ignored. The state resets for a fresh physical press after a lost release, such as after locking. Disabling the setting removes the hook.
- Renamed runtime executables and identity to Doggo; preserved upstream licensing and attribution. Removed automatic first-run onboarding and blocked upstream update installation.

## Build and automated checks

Use the installed Rust 1.99.1 MSVC toolchain (`cargo +stable` on this machine) and Windows SDK. Upstream's toolchain file requests 1.99; the explicit override uses the installed patch release.

```powershell
cargo +stable fmt --all --check
cargo +stable test --workspace --locked
cargo +stable clippy --workspace --all-targets -- -D warnings
cargo +stable clippy --workspace --all-targets --features wisp/nightly,wisp-settings/nightly,wisp/perf-trace -- -D warnings
.\scripts\build-doggo.ps1
```

Tests cover pin-file save/replacement, order through restart, unpin persistence, normalized full executable paths and Store AUMIDs, duplicate-free pinned/running ordering, reordering in both directions, invalid reorder targets, shortcut parsing, standalone left/right Windows taps, repeats, chords, pre-held keys, and opt-in defaults. They do not prove Explorer's Start-menu behavior or real application launches. See VALIDATION.md for the checks actually run on this build. `scripts\smoke-doggo.ps1` builds and runs upstream's optimized instrumented launcher with temporary config, verifies initialization and presented frames, and waits for its automatic shutdown. It does not assert interactive Windows-shell behavior.

## Manual desktop checklist (required)

- Pin two desktop apps and a Microsoft Store app. Close them: icons remain. Quit/restart Doggo: pins and order remain. Unpin one; restart and confirm it stays unpinned.
- Launch each closed pin. Confirm exactly one dock icon gains its running indicator; clicking it switches/minimizes using existing behavior. Try multiple windows, a path with spaces, and an app with an updater/versioned executable. Confirm apps with the same executable basename in different folders stay distinct. Check **Close window** on a running pinned and unpinned app.
- Drag first pin to last and back, including while apps are running. Restart to confirm order. A tiny mouse movement should still click; a completed drag must not launch/switch. Release outside the dock and verify subsequent clicks still work.
- Enable Windows key dock. Tap left and right Win separately: the dock opens/closes once, Start stays closed. Type immediately: search opens with the first and all following characters present. Repeat with Shift/capital letters, spaces, accented characters and Unicode. Test after an old query; the new query must replace it. Escape and switching apps dismiss the keyboard dock. Hold Win through repeats, then release: only one dock toggle. Try Win+E, Win+R, Win+D and Win+L; Doggo must not toggle and the Windows action must work. After unlocking, a fresh tap must work. Try pressing/releasing keys in different orders, both Win keys together, and Ctrl/Alt/Shift already held. Disable the option: Start returns; your configured shortcut continues working. Repeat after restarting Doggo.
- Check autohide and always-visible dock modes, secondary monitors, mixed DPI, virtual desktops, fullscreen apps, tray icons, Explorer restart, and fast dock-to-search toggles. Confirm the existing animation is smooth.

Real keyboard/menu behavior, UAC/elevated-app interactions, physical multi-monitor/DPI behavior, and interactive drag/launch behavior need manual desktop testing. No before/after performance claim is made without the upstream benchmark runs on a quiet desktop.

The dock-first regression check can be repeated with `scripts\smoke-doggo.ps1 -Scenario dock-typing`, and with `-Position center` for a separate dock window. It checks the first characters, Unicode surrogate pairs, Escape, and reopening during the closing animation without synthesizing physical Windows-key input.

Opening doggo.exe while it is already running reveals the dock; it does not open search or toggle the dock off. The tray Open action does the same. A fresh process starts the normal dock with its configured autohide behavior. The configured shortcut and explicit `--show` still open search directly. `scripts\smoke-doggo.ps1 -Scenario launch-dock` verifies this using real second-process invocations.

The tray arrow is always available. When Doggo hosts icons it opens the icon flyout; when no icon host is available it temporarily reveals the native Windows taskbar. Right-click the arrow and choose **Show taskbar** to access Windows system controls such as Wi-Fi. With **Hide the taskbar** off, Doggo stays above the native taskbar instead of covering it. The native taskbar retains its own auto-hide preference.

## Power and volume controls

A small power icon sits to the right of the clock. It opens a menu for Lock computer, Restart and Shut down. Lock acts immediately; Restart and Shut down ask for confirmation with No selected by default.

Click the volume icon to open a slim vertical popup matching the dock's colors and rounded corners. It shows a percentage, vertical slider and speaker icon for mute/unmute. Drag up to increase volume and down to decrease it, or use the arrow keys for 1% steps and Page Up/Down for 10%. Home/End select 0/100. Click the speaker icon to mute/unmute, or press M. Tab switches between the slider and mute button; Space/Enter activates the focused mute button. Escape or a click outside closes the popup. The popup follows changes from media keys and other apps. Scrolling over dock volume and right-clicking it for Sound Settings still work. Moving the slider unmutes, as in Wisp's existing volume commands.

The popup uses Win32 and the existing software Direct2D renderer; no new UI framework or dependency was added. The `dock-controls` capture checks rendering, keyboard focus selection and Escape without changing volume or executing a power command. Actual shutdown, restart and locking are deliberately left for your own use.
