# Contributing to Doggo

Doggo is a personal fork of [Wisp](https://github.com/SumirSeth/wisp). Keep changes small, native and easy to verify. Preserve the MIT license and upstream attribution.

## Changes

- Describe the problem and the resulting behavior.
- Preserve Rust, Win32, Direct2D, the dock animation, tray integration and existing settings.
- Use `feat/`, `fix/`, `docs/` or `perf/` branches and plain-language commit titles.
- Keep unrelated features in separate changes.
- Do not include local configuration, private files or build output in source commits.

## Validation

Run formatting, the workspace tests and both Clippy configurations shown in README.md. Build the release workspace. Test changed behavior on a real desktop, documenting the Windows version, display scaling, monitor layout and dock mode.

Distinguish automated checks from manual observations and untested behavior. UI and timing changes need before/after performance measurements on the same machine. Never exercise real power commands in an automated check.

## Proposing changes to Wisp

Follow the [upstream contribution guide](https://github.com/SumirSeth/wisp/blob/main/CONTRIBUTING.md). Search existing issues, discuss features before opening a pull request, and submit one change per pull request. Keep Doggo branding, installation identity and personal preferences out of upstream patches. See UPSTREAM.md for the proposed split.

Contributions use the [MIT license](LICENSE).
