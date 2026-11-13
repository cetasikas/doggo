<p align="center"><img src="assets/icon.svg" width="80" height="80" alt="Doggo"></p>
<h1 align="center">Doggo</h1>
<p align="center">A lightweight dock and launcher for Windows, built with Rust, Win32 and Direct2D.</p>

Doggo is a personal fork of [Wisp](https://github.com/SumirSeth/wisp) by Sumir Seth and contributors. It keeps Wisp's native architecture, dock-to-search animation, tray integration and MIT license.

## Features

- **Pinned apps:** reuse upstream pinning, with saved drag reordering. Closed pins launch their apps; running pins switch to existing windows.
- **Windows-key dock:** optionally tap Windows to reveal the dock, then type to search. Windows combinations remain available. The configured search shortcut still opens search directly.
- **System controls:** tray access remains available, with a native Windows taskbar fallback. When the taskbar is shown, Doggo stays above it.
- **Power menu:** a small icon beside the clock offers Lock, Restart and Shut down.
- **Vertical volume:** click the speaker for a slim slider and mute button, matching the dock. Drag up/down, use arrow keys, or scroll over the dock speaker.
- **Desk fences:** optional labelled groups that gather desktop shortcuts without moving files. Create saved tabs inside each fence, choose Icons, Thumbnails or Details, set icon size and transparency, resize and scroll, or browse folders in place.

## Desk fences

Doggo can draw Fences-style labelled groups on the primary monitor, above the wallpaper and below normal windows. Fences are views, not folders: they store references, never move or delete files, and disappear when Doggo exits. The desk is off by default.

Set `enabled = true` in `%APPDATA%\doggo\desk.toml` (created on first run) and restart Doggo. The default file groups shortcuts, folders and files into three fences. Click **+** to create a tab and edit its selected name in place; press Enter or click away to save, or Escape to keep its default name. Double-click any tab to rename it in place. Right-click the fence for **New tab**, **Rename current tab**, and **Remove current tab**. The **Main** tab is protected from removal. Each tab keeps its own item rules and contents across restarts. Right-click a fence to create, rename or remove a custom fence, change its view, icon size, sort order or transparency. The Details view shows name, type, size and modified date; Thumbnails uses Windows Shell previews where available and falls back to the file icon. Drag the lower-right corner to resize in both directions; scroll smoothly with the mouse wheel or drag the scrollbar to browse contents in a smaller fence. Double-click a folder to browse it inside the fence, then use the back arrow in the title bar to return. Right-click an item to move it to another custom fence. Drag a fence by its title, click the title to roll it up, and double-click a file to open it. Fence dimensions and appearance settings are saved.

- `category = "apps" | "folders" | "files"` claims desktop items by kind.
- `items = ["C:\\..."]` pins exact paths to a fence, ahead of the category rules.
- `folder = "C:\\..."` makes the fence a live mirror of that folder; the files stay where they are.
- Right-click a fence and choose **Fence color** to save a tint preset for that group.
- `columns`, `width`, optional saved `height`, `icon_size`, `view`, `sort`, `transparency`, `x`, `y` and `collapsed` control the layout.

`hide_icons = true` hides Explorer's own icons so only the fences show. Only enable it once the desk itself looks right: the tray menu's **Restore desktop icons** brings them back at any time. Doggo restores them on exit and repairs a crash on the next start. Everything lives in `desk.toml`; deleting it and stopping Doggo returns the ordinary desktop untouched.

## Build and run

Requires Windows, Rust with the MSVC toolchain, and the Windows SDK.

```powershell
git clone https://github.com/cetasikas/doggo.git
cd doggo
.\scripts\build-doggo.ps1
.\dist\Doggo\doggo.exe
```

The build script uses `cargo +stable`; the verified compiler is Rust 1.99.1. Keep `doggo-settings.exe` beside `doggo.exe`. The portable files are written to `dist\Doggo`.

Doggo stores settings, pins and the desk layout in `%APPDATA%\doggo`, separately from Wisp. Open Settings from the tray icon to enable **Windows key dock**, choose a shortcut or enable **Start with Windows**. Startup is optional and is never enabled by the build script.

Run one dock/taskbar controller at a time. Exit Wisp and any script that remaps the Windows key before using Doggo's native Windows-key option. Normal launches reveal the dock when Doggo is already running; `doggo.exe --show` opens search and `doggo.exe --quit` exits it.

See [DOGGO.md](DOGGO.md) for controls and settings and [VALIDATION.md](VALIDATION.md) for verification limits.

## Checks

```powershell
cargo +stable fmt --all --check
cargo +stable test --workspace --locked
cargo +stable clippy --workspace --all-targets -- -D warnings
cargo +stable clippy --workspace --all-targets --features wisp/nightly,wisp-settings/nightly,wisp/perf-trace -- -D warnings
```

## License and attribution

[MIT](LICENSE). Original Wisp code and icons: copyright Sumir Seth and contributors. Upstream persistent dock pinning was contributed by SorbetMango. The upstream project remains [SumirSeth/wisp](https://github.com/SumirSeth/wisp); Doggo is an independent fork.
