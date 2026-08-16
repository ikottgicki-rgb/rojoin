# RoJoin v4

A native Roblox client for Linux and Windows — everything the Roblox website
and app do, except running the game itself.

Built from scratch in Rust + Slint. Not Electron, not a webview: a single
self-contained executable.

## What it does

- **Sign in through your own browser.** RoJoin shows a code; you approve it on
  roblox.com or in the Roblox mobile app. No password is ever typed into
  RoJoin, and no browser engine is embedded.
- **Browse and search** games, people and groups. Paste a Roblox link and it
  jumps straight there.
- **Join games**, including **sub-places** — the specific place inside a game,
  rather than being dropped in the lobby and made to walk.
- **Server browser** with fullest/emptiest sorting, and join-a-specific-server.
- **Friends** grouped by presence, with pinned friends at the top, and Join
  that lands in your friend's actual server.
- **Direct messages** — conversation list, history, send.
- **Profiles and groups**, with friend/follow and group join/leave.
- **Avatar editor** for items you already own, plus saved outfits.
- **Macros** — timed input sequences with editable steps.

Deliberately absent: no catalog, no marketplace, no prices, no purchases.
RoJoin edits the avatar you have; buying things is Roblox's business.

## Build

```sh
cargo run                # Linux, debug
./scripts/build.sh       # both targets, release, with artifact assertions
./scripts/package.sh     # AppImage + Windows zip
```

The Windows build cross-compiles from Linux via `x86_64-pc-windows-gnu` and
must stay a single `.exe` with no console window. `scripts/build.sh` fails the
build if it gains a mingw runtime dependency or a console subsystem.

### Development helpers

```sh
./scripts/smoke.sh                                  # headless run, fails on panics
ROJOIN_DEMO=1 ROJOIN_SECTION=2 ./scripts/shot.sh out.png
```

Both run the app inside an isolated headless compositor with a throwaway
config directory, so they never touch a real session.

## Macros

A macro is a list of timed input steps, and every step is editable. Input is
synthesised the way a keyboard or mouse would send it — a uinput virtual
device on Linux, `SendInput` on Windows. Nothing reads or writes another
process's memory.

Linux needs permission to create a virtual input device. If it is missing, the
Macros tab prints the exact commands to fix it.

The bundled presets encode plausible timings, **not values verified against a
live game**. Roblox physics changes between updates, so expect to tune them.

