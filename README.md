# RoJoin

A native Roblox client for Linux and Windows. Rust + Slint, single binary.

Browse and search, join games including sub-places, server browser, friends
with presence, profiles, groups, avatar editor, and a macro tab.

No catalog, no purchases.

## Build

```sh
cargo run
./scripts/build.sh      # both targets, release
./scripts/package.sh    # AppImage + Windows zip
```

Windows cross-compiles from Linux via `x86_64-pc-windows-gnu`.

## Macros

Input is synthesised through a uinput virtual device on Linux and `SendInput`
on Windows. Linux needs access to `/dev/uinput`; the Macros tab prints the
setup commands if it is missing.

Preset timings are starting points, not tested values.
