# Lucid

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)

**Keep your Mac awake — straight from the menu bar.**

Lucid is a tiny macOS menu-bar app that stops your Mac from going to sleep — handy
when you're presenting, downloading, compiling, or just reading and don't want the
screen to dim. Click once to stay awake until you turn it off, or pick a timer
(15 minutes up to 8 hours) and it switches itself off. While it's active the
menu-bar icon glows and shows a live countdown — no dock icon, no window, no fuss.
It also has a built-in **Pomodoro** cycle that keeps you awake through each focus
block and chimes when it's time for a break.

> ⚠️ **Status:** v0.2 — working. macOS only.

![Lucid's menu — an amber dot and a live countdown in the macOS menu bar](docs/screenshot.png)

<sub><i>Awake with a 2-hour timer running — the menu-bar icon glows amber and counts down beside it; click to toggle, or pick a timer.</i></sub>

## Why it's tiny and trustworthy

Lucid doesn't reinvent power management — it drives the **`caffeinate`** utility
that already ships with macOS (`caffeinate -d -i` to keep the display and system
awake, or just `-i` when you let the display sleep). It simply spawns and stops
that process for you, with a friendlier face:

- **No background daemon, no kernel extensions** — just one short-lived child
  process that exists only while you're keeping awake (launch-at-login is
  optional, and off until you turn it on).
- **Idle CPU ≈ 0** — there's nothing running until you flip it on.
- **The whole UI is the native menu** — no web view is ever shown.

## Features

- **Toggle** keep-awake on/off from the menu bar
- **Timers** — keep awake for 15 min, 30 min, 1, 2, 4, or 8 hours, then it switches off automatically
- **Pomodoro** — a built-in focus timer (Classic 25 / 5 or Deep work 50 / 10): it stays awake through each block and **chimes + notifies** on every focus⇄break switch, with a long break after 4 rounds
- **Keep display on — or not** — leave the screen on, or let it sleep while the system stays awake (handy for a long download)
- **Launch at login** — optional, so Lucid is waiting in the menu bar when you sign in
- **Live countdown** — the menu-bar icon glows amber and counts down beside itself while active
- **Stays out of the way** — menu-bar only, no dock icon, no window
- **Cleans up after itself** — quitting Lucid ends the keep-awake immediately

## Install / run from source

Prerequisites: [Node.js](https://nodejs.org) 18+ and the
[Rust toolchain](https://www.rust-lang.org/tools/install)
([Tauri prerequisites](https://tauri.app/start/prerequisites/)).

```bash
git clone https://github.com/growthforging/lucid.git
cd lucid
npm install

# run in development
npm run tauri dev

# build a distributable .app
npm run tauri build
```

When it launches, look for the icon in your **menu bar** (top-right) — there's no
dock icon or window by design.

## Tech

- [Tauri v2](https://tauri.app) — native tray + tiny footprint (Rust)
- macOS [`caffeinate`](x-man-page://caffeinate) — the actual sleep prevention
- macOS `osascript` + `afplay` — Pomodoro notifications and the break chime
- The logic lives in [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs)

## License

[MIT](LICENSE)
