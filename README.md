# RoJoin v4

A native Roblox client for Linux and Windows — everything the Roblox website and
app do, except running the game itself.

Built from scratch in Rust + Slint. Not Electron, not a webview: a single
self-contained executable, ~100 MB resident.

## Status

In development. Milestone 1 — foundation, design system, quick-login auth, and
the join pipeline.

## Build

```sh
cargo run                # Linux, debug
./scripts/build.sh       # both targets, release, with artifact checks
```

The Windows build cross-compiles from Linux via `x86_64-pc-windows-gnu` and
must stay a single `.exe` with no console window. `scripts/build.sh` asserts
both properties and fails the build if either regresses.

## Credits

The macro utilities tab is an independent implementation, inspired by
[Spencer Macro Utilities](https://github.com/Spencer0187/Spencer-Macro-Utilities).
No code is shared between the projects.
