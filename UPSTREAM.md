# Proposing Doggo changes to Wisp

Discuss feature scope first, then submit small pull requests against the latest Wisp main branch. Follow the [upstream contribution guide](https://github.com/SumirSeth/wisp/blob/main/CONTRIBUTING.md) and its pull-request template.

## Suggested order

1. **Taskbar coexistence fixes:** keep native controls reachable, keep the chevron present without a tray host, respect the work area when the native taskbar is shown, and retain settings notifications during reentrant callbacks. Include reproduction steps and the before/after geometry checks.
2. **Pinned-app reordering:** coordinate with [issue #32](https://github.com/SumirSeth/wisp/issues/32), which already has another contributor's implementation. Persistent pinning itself is already upstream; preserve that contributor's work and attribution.
3. **Optional Windows-key dock:** describe standalone-release detection, combination preservation, repeats, focus handling and type-to-search. Include physical desktop tests for Win+E/R/D/L, unlock, rapid input and multiple monitors.
4. **Volume popup:** propose the compact vertical native slider, mute button and keyboard behavior, with screenshots and measurements.
5. **Power menu:** discuss this separately because it is a UI/scope preference. Restart and shutdown must keep confirmation and must not be executed by automated tests.

Do not submit the whole renamed Doggo fork as one patch. Keep Wisp's name, configuration path, startup identity and updater in upstream patches. Leave personal deployment scripts and publication cleanup out of those patches.

## Draft feature discussion

I have been using a personal Wisp fork with native dock enhancements: saved pin reordering, an optional standalone Windows-key dock that becomes search when typing, and a compact vertical volume popup. It retains Rust/Win32/Direct2D and the existing animation. I also found taskbar coexistence issues where the dock could cover native system controls when taskbar hiding was disabled.

Would the maintainers be interested in focused patches for any of these? I would start with the coexistence fixes and coordinate reordering with #32, then keep the optional keyboard and UI features separate.

## Before submitting

Refresh against upstream main, check for newer implementations, and isolate one behavior per branch. Re-run upstream formatting, both Clippy configurations, tests and release build. Add before/after performance measurements and identify physical desktop coverage and remaining limitations. Current automated capture checks are not a substitute for physical Windows-key, mixed-DPI and multi-monitor testing.

No issue, comment or pull request has been submitted by preparing this document.
