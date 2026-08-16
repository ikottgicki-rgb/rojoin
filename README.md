# RoJoin

A desktop app for Roblox on Linux and Windows. Browse and search games, join them
(including sub-places), see servers, manage friends and groups, edit your avatar,
and run input macros. One small native binary, not a browser in a box.

It does not replace the game client itself, and there is nothing to buy in it.

## Get it

Grab the latest [release](https://github.com/ikottgicki-rgb/rojoin/releases):

- **Linux** — `RoJoin-x86_64.AppImage`, mark it executable and run it
- **Windows** — `RoJoin-windows-x64.zip`, unzip and run `RoJoin.exe`

Sign in with the code it shows you, approved on roblox.com or in the Roblox
mobile app. You never type your password into RoJoin.

## Macros

The Macros tab types and clicks for you on a timer. On Linux it needs access to
`/dev/uinput`; the tab tells you what to run if it is missing.

## Build

```sh
cargo run
./scripts/build.sh
./scripts/package.sh
```

MIT licensed.
