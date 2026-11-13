# Doggo personal fork

Doggo is a personal fork of [Wisp](https://github.com/SumirSeth/wisp), based on upstream commit `4d86a783cf4b9c73202e72398c01b68b54f12267` (verified against upstream HEAD on November 10, 2026). Original code and icons are copyright Sumir Seth and contributors under the MIT license; see LICENSE. Upstream dock pinning was contributed by @SorbetMango. This fork retains Rust, Win32, Direct2D, the existing settings app, and the dock/launcher morph.

## Run and configure

Run `dist\Doggo\doggo.exe` after building, with `doggo-settings.exe` beside it. Stop any other dock/taskbar replacement and Windows-key remapping script first.

Open Settings from the notification icon. **Windows key dock** is optional and defaults off; the configured shortcut defaults to Alt+Space. Settings apply live. The saved `windows_key_search` key remains compatible with earlier Doggo builds.

Doggo uses `%APPDATA%\doggo` for config, history, surfaces and `dock-pins.toml`. Its instance mutex, window class and **Doggo** sign-in task are separate from Wisp. Enable **Start with Windows** only if desired; building and extracting the app do not enable it. Upstream binary updates are disabled so they cannot replace the fork.

## What changed

- Reused upstream's unreleased persistent pinning, application matching, launch/switch behavior, and close-window menu.
- Drag a pinned icon horizontally over another pinned icon to move it. The other pins shift; release to save. A normal click below Windows' drag threshold retains launch/switch behavior. Dragging does not launch an app. To reorder pins beyond the visible area, Shift+scroll the dock.
- Added an optional native low-level keyboard hook. Physical Windows-key down and all combinations pass through; a standalone release reveals and focuses the dock. Typing a printable character opens the existing search animation and keeps that character. Escape, another standalone tap, or switching apps dismisses the keyboard dock. The configured shortcut still opens search directly. Repeats do not rearm it. A menu-mask input on a standalone release prevents Start. Injected input is ignored. The state resets for a fresh physical press after a lost release, such as after locking. Disabling the setting removes the hook.
- Renamed runtime executables and identity to Doggo; preserved upstream licensing and attribution. Removed automatic first-run onboarding and blocked upstream update installation.
- Restyled the dock as a macOS-like strip with mono status readouts and a `>_` prompt in the search slot. The dock, search and Fences share Doggo's light, dark or Windows theme, including accent and backdrop colors. Icons magnify in a soft wave under the pointer instead of drawing taskbar-style tile boxes. Near-black icons get a lit plate so they stay legible against either theme. The dock is 64 DIP tall so the magnified icons stay inside the window.
- Replaced teal accents with midnight electric blue across both themes. Added per-fence color presets from the right-click menu, saved individually in `desk.toml`.

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
- Desk fences: with `enabled = true` and `hide_icons = false`, confirm fences sit above the wallpaper and below normal windows, clicks between fences reach the desktop, dragging moves a fence and saves its position, the fences never overlap (a saved layout that overlaps is separated automatically on load, and a drag stops at the other fence), clicking the title rolls it up, and double-clicking opens an item. Right-click a fence for **Roll up**, **Arrange fences → Snap to grid / Stack left / Stack right**, **Desk settings…**, **Doggo settings…** and **Restore desktop icons**, and right-click the dock background for the same settings entries; arranged fences must stay clear of one another. Add, rename and delete desktop files and confirm the fences refresh without restarting Doggo. Point a fence at a folder with `folder = "…"` and confirm it mirrors the folder live. Only then try `hide_icons = true`, exit, and confirm the icons return; kill Doggo from Task Manager, restart it, and confirm the crash repair restores them. Turn the icons back on from Explorer's **View** menu and confirm Doggo hides them again. Use **Restore desktop icons** from the tray, dock or fence menu as the escape hatch.

Real keyboard/menu behavior, UAC/elevated-app interactions, physical multi-monitor/DPI behavior, and interactive drag/launch behavior need manual desktop testing. No before/after performance claim is made without the upstream benchmark runs on a quiet desktop.

The dock-first regression check can be repeated with `scripts\smoke-doggo.ps1 -Scenario dock-typing`, and with `-Position center` for a separate dock window. It checks the first characters, Unicode surrogate pairs, Escape, and reopening during the closing animation without synthesizing physical Windows-key input.

Opening doggo.exe while it is already running reveals the dock; it does not open search or toggle the dock off. The tray Open action does the same. A fresh process starts the normal dock with its configured autohide behavior. The configured shortcut and explicit `--show` still open search directly. `scripts\smoke-doggo.ps1 -Scenario launch-dock` verifies this using real second-process invocations.

The tray arrow is always available. When Doggo hosts icons it opens the icon flyout; when no icon host is available it temporarily reveals the native Windows taskbar. Right-click the arrow and choose **Show taskbar** to access Windows system controls such as Wi-Fi. With **Hide the taskbar** off, Doggo stays above the native taskbar instead of covering it. The native taskbar retains its own auto-hide preference.

## Power and volume controls

A small power icon sits to the right of the clock. It opens a menu for Lock computer, Restart and Shut down. Selecting an action sends it directly to Windows.

Click the volume icon to open a slim vertical popup matching the dock's colors and rounded corners. It shows a percentage, vertical slider and speaker icon for mute/unmute. Drag up to increase volume and down to decrease it, or use the arrow keys for 1% steps and Page Up/Down for 10%. Home/End select 0/100. Click the speaker icon to mute/unmute, or press M. Tab switches between the slider and mute button; Space/Enter activates the focused mute button. Escape or a click outside closes the popup. The popup follows changes from media keys and other apps. Scrolling over dock volume and right-clicking it for Sound Settings still work. Moving the slider unmutes, as in Wisp's existing volume commands.

The popup uses Win32 and the existing software Direct2D renderer; no new UI framework or dependency was added. The `dock-controls` capture checks rendering, keyboard focus selection and Escape without changing volume or executing a power command. Actual shutdown, restart and locking are deliberately left for your own use.

## Always-visible overlay

**Keep the dock visible** keeps Doggo floating above normal and maximized windows on the primary screen. It does not reserve an app-bar strip or shrink the usable work area. Maximized windows can extend behind the dock. Turning the setting off retains bottom-edge autohide. The native Windows taskbar still keeps its own area when it is shown; fullscreen suppression is unchanged.

## Desktop fences

Doggo can take over the primary monitor's desktop with labelled groups, similar to Fences. The desk window covers the monitor but its window region is exactly the union of the fences, so clicks between groups still reach Explorer's desktop. It sits on the wallpaper layer: directly above Explorer's desktop window and below every normal window. It is built from the existing Win32 and Direct2D code, with no shell extension, service or new UI framework.

Layout lives in `%APPDATA%\doggo\desk.toml`, separate from `config.toml`, in the same way as `dock-pins.toml`. It is created with the defaults on first run and read at startup and when settings reload. The desk is off until `enabled = true`.

Each fence has saved tabs. Click **+** to create one and edit its selected name in place; Enter or clicking elsewhere saves the name, while Escape keeps its default. Double-click any tab to rename it. The fence's main right-click menu includes New, Rename and Remove tab actions. The **Main** tab cannot be removed and is restored in older layouts where it was deleted.

```toml
enabled = true
hide_icons = false
show_labels = true

[[fence]]
id = "apps"
name = "Apps"
x = 40
y = 40
width = 320
columns = 4
collapsed = false
category = "apps"
items = ["C:\\Users\\me\\Desktop\\Work.lnk"]

[[fence]]
id = "projects"
name = "Projects"
x = 380
y = 40
width = 360
columns = 4
folder = "C:\\Users\\me\\Projects"
```

- `enabled` turns the desk on. `hide_icons` additionally hides Explorer's own desktop icons so only the fences show. `show_labels` controls the names under the icons.
- `id` is unique and stable; `name` is the title drawn on the fence. `x`, `y`, `width`, `columns` and `collapsed` are the saved geometry. Dragging and rolling up write these back.
- `category` claims desktop items by kind: `apps` (`.lnk`, `.url`, `.appref-ms`), `folders`, or `files`. The first fence with a matching category takes the item.
- `items` pins exact absolute paths to a fence. Explicit paths win over category rules, and matching ignores case.
- `folder` makes the fence a portal: it lists that folder's contents live and ignores `category`. The files never move. The desk watches portal folders as well as the desktop, so changes appear without polling.

Fences never move, rename or delete a file. They hold strings that point at the real items in `Desktop` (or a portal folder). Quitting Doggo removes the desk and leaves the desktop exactly as it was. Deleting `desk.toml` while Doggo is stopped discards the layout; the desktop files are unaffected.

### Right-click menus

Right-clicking a fence opens **Roll up**/`Expand`, a **Fence color** submenu with **Theme default**, **Electric blue**, **Violet**, **Pink**, **Green** and **Amber**, an **Arrange fences** submenu with **Snap to grid**, **Stack left** and **Stack right**, **Desk settings…** (opens `desk.toml`), **Doggo settings…**, and **Restore desktop icons**. The selected tint is saved per fence and blended with the current light or dark base colors. The arrange presets make a tidy, gap-separated layout that fits the screen and save the new positions; they never stack one fence on another. A double-click on a title still rolls the fence up, and dragging still moves a single fence as before.

Right-clicking the dock where no app or button sits opens **Doggo settings…**, **Desk settings…** and **Restore desktop icons**. Both menus use the tray's escape hatch for Explorer's icons.

### Hiding Explorer's icons

`hide_icons = true` uses the shell's own **Show desktop icons** command, so Explorer keeps ownership of the icons; Doggo does not patch or subclass it. Doggo restores the icons when it exits and writes `desk-icons.state` while they are hidden. If Doggo crashes or is killed, the next start sees the marker and restores them before drawing the desk. If the shell still refuses, the marker is kept so the next attempt can retry.

Doggo decides whether the icons are hidden from the desktop icon list itself, not from the `HideIcons` registry value: Explorer can rebuild its desktop and show the icons again while that value still says 1. While `hide_icons = true`, a half-second repair timer puts the desk back above Explorer's desktop (and shows it again if Explorer hid it), and checks the icons every other second so a refusing shell cannot stall the UI. Repainting survives a lost Direct2D device by recreating the target and drawing the frame again. A menu closing returns the desk to the wallpaper layer, so right-clicking or dragging never leaves the fences behind another window.

The tray menu always offers **Restore desktop icons**, and the dock and fence menus repeat it. It shows the icons and sets `hide_icons = false` so they stay visible after a reload or restart. Prefer verifying the desk with `hide_icons = false` first, then opt in. Window region, z-order and a completed Direct2D frame are covered by an ignored UI test that builds a desk window directly; interactive drag, launch and real explorer-restart behavior remain manual checks.
