# Doggo validation - November 10, 2026

Upstream base: `4d86a783cf4b9c73202e72398c01b68b54f12267`, verified against GitHub HEAD before implementation. Built with Rust 1.99.1 MSVC on this Windows machine. Development branch: `feat/doggo`. See the repository for publication status.

## Automated checks completed

- `cargo +stable fmt --all --check`: passed.
- `git diff --check`: passed.
- `cargo +stable test --workspace --locked`: passed, 112 launcher tests + 34 settings tests. One upstream Windows Search integration test is ignored. Output: `target/doggo-tests.log`.
- Default Clippy and upstream's nightly/perf-trace feature combination, workspace and all targets, both with `-D warnings`: passed.
- `cargo +stable build --release --workspace --locked`: produced `doggo.exe` and `doggo-settings.exe`.
- Pin persistence checks save and replace an actual temporary pin file, reload after reordered saves, remove a pin and reload, and reject malformed/duplicate entries. Matching tests cover normalized full paths, same basenames in different directories, Store AUMIDs, and duplicate-free pinned/running ordering. Drag-order logic is checked in both directions with bounds checks.
- Windows-key state tests cover either Win key, repeated key-down, orphan/repeated key-up, Win+E/R/D/L, both Win keys, and Ctrl/Alt/Shift or other keys held before Win. Config tests confirm the option defaults off and retains configured combinations.
- `scripts/smoke-doggo.ps1 -SkipBuild`: passed against the optimized instrumented `launcher_demo` built with `perf-trace`. Temporary config has taskbar hiding off. Trace reached `ready`, recorded 26 `content_frame` events, 2 morph starts, 20 morph frames and 12 slide frames, then exited successfully through the existing shutdown timer. Evidence: `target/capture-020369916b8c477aa30d615505f17107/trace.csv` and `result.txt`. This checks the shared launcher source with the demo's separate channel identity; it does not prove the shipped binary's interactive shell behavior.

## Desktop checks completed

- Opened the release Doggo Settings window on the desktop and inspected its General page: the shortcut and Windows key search switch are present. Closed it normally.
- The console diagnostic build reached OLE/renderer initialization, created its launcher window, loaded `%APPDATA%\doggo\config.toml`, and completed launcher initialization. A second invocation with `--quit` shut it down cleanly. Temporary debug tracing was removed from the source afterward; startup error reporting remains.
- An initial external window-discovery smoke probe could not discover the hidden/tool launcher window. It was replaced by the existing instrumented capture check above; that initial probe is not counted as a passing release desktop test.

## Still requires manual testing

Physical Win-key Start suppression and shell shortcuts (especially Win+L/unlock), input on elevated windows and secure desktops; actual desktop/Store app launch matching and icon merging; interactive pin/unpin/reorder across process restarts; tray/Explorer recovery; mixed-DPI and multiple physical monitors. Follow DOGGO.md. The state-machine tests do not inject physical input, and the capture test does not interact with Explorer's Start menu.

No full upstream before/after performance benchmark was run on a quiet desktop. Capture counts demonstrate execution, not a latency or smoothness guarantee.

## Isolation and delivery

Portable install: `dist/Doggo`, containing both executables, upstream license/README/changelog, this report and DOGGO.md. No Wisp executable or config was overwritten. Doggo's config is `%APPDATA%\doggo`; its Run value is `Doggo`. Upstream binary updates are blocked. Startup was not enabled.

The new Doggo config on this machine is initialized for safe testing: Ctrl+Shift+F12, taskbar hiding off, Windows-key search off. Enable the desired options after quitting Wisp and exiting the Windows-key AutoHotkey script. Normal factory config retains Alt+Space.

## Follow-up: Windows key opens the dock first

Windows-key activation now reveals a keyboard-focused dock. The first printable character opens search with that character retained; subsequent queued characters are forwarded in order for the separate-dock layout too. A new query replaces the old query. Escape, another Windows-key tap or focus loss dismisses the keyboard dock. The existing configured shortcut continues to open search directly. The setting is labeled **Windows key dock**, while the saved `windows_key_search` key remains compatible. Existing config values were preserved.

Full workspace tests (146 passed, one ignored), formatting and both Clippy configurations passed again. Optimized scripted checks passed for the shared bottom dock and separate center-position dock: idle activation did not open search; Return did not activate a stale result; the first two characters were retained; Escape dismissed the dock; reopening during a closing morph reset the old animation; Unicode surrogate pairs and a following character were retained. The normal dock/search animation scenario also passed. These assertions use native application handlers through the existing capture harness, rather than physical keyboard input. Physical keyboard focus and Windows-shell behavior remain manual checks.

## Follow-up: launching an existing instance opened search

The second-process path still forwarded normal launches to upstream's search action. It now forwards them to an idempotent dock-open action. A fresh process starts the normal dock without search. The tray Open command reveals the dock; the configured search shortcut and explicit `--show` retain direct search behavior.

A new optimized `launch-dock` capture launches actual child processes against the existing instance. It verifies that startup leaves search closed, a normal second launch reveals only the dock, repeated dock-open is idempotent, explicit `--show` opens search, and another normal launch returns to the dock without a leftover search morph. This passed for both shared bottom and separate center layouts. The dock-typing scenario passed again. These process-level assertions cover the launch path missed by the previous handler-only checks; physical desktop input remains a manual check.

## Follow-up: tray arrow and taskbar visibility

The native Hide the taskbar switch was tested through Doggo Settings. Enabled: Explorer's actual taskbar was invisible with an empty region. Disabled: it became visible with its region restored. The original overlap was measured on the live desktop: Doggo occupied y=655..711 while Explorer's native taskbar occupied y=672..720. Thus the restored taskbar's controls were covered.

The installed fix keeps a chevron in the dock even without a tray host, provides native taskbar access when no copied tray icons are available, and respects Explorer's work area while the taskbar is shown. Search uses that work area too. Reentrant config notifications are deferred instead of discarded. The native peek fallback also works without a taskbar-hiding controller.

All 146 workspace tests pass (one existing test ignored), with updated chevron geometry assertions; formatting and both Clippy configurations pass. The read-only native probe against the installed release passed `--expect-visible --assert-clear`: Doggo y=607..663, Explorer y=672..720, leaving a 9-pixel gap. Hide the taskbar was returned to its original off value after the live toggle test; the user's Keep the dock visible choice was preserved. No further desktop-control actions were performed after Escape stopped Computer Use. The Settings window stayed open during executable replacement; reopening Settings loads the updated companion binary.

The new `tray-access` optimized capture scenario is present but was not run in this follow-up; it should not be counted as a passing check. Other physical monitor layouts and native tray popup interactions still require manual testing.

## Power button and volume popup

A power icon now follows the clock. Its menu exposes only Lock, Restart and Shut down. Restart and Shut down use a Yes/No confirmation with No selected by default, and reuse the existing native power implementation. Unit tests verify the permitted action IDs, cancellation IDs and confirmation requirements. No lock, restart or shutdown command was executed during validation.

Clicking dock volume now opens a 280-by-120-DIP popup using the existing software Direct2D renderer, Core Audio functions and dock palette. It provides a slider, percentage, mute/unmute, mouse capture, keyboard adjustment, Tab focus, external-volume refresh while open and Escape/outside-focus dismissal. No dependencies or UI framework were added. Existing volume-wheel and right-click Sound Settings behavior is retained.

The workspace suite passes 148 tests (114 launcher, 34 settings), with one existing ignored test. Formatting, default Clippy and the optional-feature Clippy checks pass. Optimized dock-controls captures passed in bottom and center layouts: native popup rendering completed, Tab selected the mute control and Escape hid the popup. Those tests did not change audio. Capture evidence: `target/capture-2b5df2acd4f14d2d9d7fa536fe077ac5` and `target/capture-b14e913381e948fb993f5354af85ee8a`. The existing dock-typing scenario passed again (`target/capture-5fb61aab2762482f9cedc0fd8ee472cd`). Actual audio adjustment, pointer dragging and power actions remain manual desktop checks.

## Simplified vertical volume panel

Volume now uses a 64-by-232-DIP panel with the dock's colors and 8-DIP corners. The title and wide text button were removed. A percentage sits above a vertical track; a speaker icon below toggles mute. Dragging upward increases volume and downward decreases it. Keyboard controls and dismissal behavior are retained.

The vertical input mapping and clamping tests pass; all 148 workspace tests pass with one existing ignored test. Both Clippy configurations and formatting checks pass. Optimized bottom and center dock-controls captures verify the actual client dimensions, native rendering, Tab focus and Escape dismissal. They do not change audio or invoke power actions. Pointer interaction and appearance at other physical display scales remain manual checks.

## Always-visible dock overlay

Keep dock visible now leaves the dock floating above normal windows without registering a reserved appbar strip. The installed release passed the native overlay assertion: its visible dock rectangle (333,655)-(947,711) sits inside the unchanged monitor work area (0,0)-(1280,720). Toggling this setting did not change the work area. The original autohide preference was restored afterward.

All 148 workspace tests pass (one existing ignored), formatting and both Clippy configurations pass, and both release executables build successfully. The native check measures window/work-area geometry; visual interaction with maximized windows, fullscreen apps and multiple physical monitors remains a manual check.
