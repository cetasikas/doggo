<p align="center"><img src="assets/icon.svg" width="80" height="80" alt="Doggo"></p>
<h1 align="center">Doggo</h1>
<p align="center">A lightweight dock and launcher for Windows, built with Rust, Win32 and Direct2D.</p>

Doggo is a personal fork of [Wisp](https://github.com/SumirSeth/wisp) by Sumir Seth and contributors. It keeps Wisp's native architecture, dock-to-search animation, tray integration and MIT license.

## Features

- **Pinned apps:** reuse upstream pinning, with saved drag reordering. Closed pins launch their apps; running pins switch to existing windows.
- **Windows-key dock:** optionally tap Windows to reveal the dock, then type to search. Windows combinations remain available. The configured search shortcut still opens search directly.
- **System controls:** tray access remains available, with a native Windows taskbar fallback. When the taskbar is shown, Doggo stays above it.
- **Power menu:** a small icon beside the clock offers Lock, Restart and Shut down. Restart and shutdown ask for confirmation.
- **Vertical volume:** click the speaker for a slim slider and mute button, matching the dock. Drag up/down, use arrow keys, or scroll over the dock speaker.

## Build and run

Requires Windows, Rust with the MSVC toolchain, and the Windows SDK.

```powershell
git clone https://github.com/cetasikas/doggo.git
cd doggo
.\scripts\build-doggo.ps1
.\dist\Doggo\doggo.exe
```

The build script uses `cargo +stable`; the verified compiler is Rust 1.99.1. Keep `doggo-settings.exe` beside `doggo.exe`. The portable files are written to `dist\Doggo`.

Doggo stores settings and pins in `%APPDATA%\doggo`, separately from Wisp. Open Settings from the tray icon to enable **Windows key dock**, choose a shortcut or enable **Start with Windows**. Startup is optional and is never enabled by the build script.

Run one dock/taskbar controller at a time. Exit Wisp and any script that remaps the Windows key before using Doggo's native Windows-key option. Normal launches reveal the dock when Doggo is already running; `doggo.exe --show` opens search and `doggo.exe --quit` exits it.

See [DOGGO.md](DOGGO.md) for controls and settings, [VALIDATION.md](VALIDATION.md) for verification limits, and [UPSTREAM.md](UPSTREAM.md) for the contribution plan.

## Checks

```powershell
cargo +stable fmt --all --check
cargo +stable test --workspace --locked
cargo +stable clippy --workspace --all-targets -- -D warnings
cargo +stable clippy --workspace --all-targets --features wisp/nightly,wisp-settings/nightly,wisp/perf-trace -- -D warnings
```

## License and attribution

[MIT](LICENSE). Original Wisp code and icons: copyright Sumir Seth and contributors. Upstream persistent dock pinning was contributed by SorbetMango. The upstream project remains [SumirSeth/wisp](https://github.com/SumirSeth/wisp); Doggo is an independent fork.
