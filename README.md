# PodStack (native)

[![license: AGPL v3](https://img.shields.io/badge/license-AGPL%20v3-red.svg)](https://www.gnu.org/licenses/agpl-3.0)

PodStack is a desktop app for monitoring your EVE Online characters, built for ease of use and speed, especially
if you manage a lot of characters. This fork is a native rewrite in **Rust + GTK4** for **Linux (Arch, Hyprland)**,
redesigned around Google's **Material You** (Material 3): dynamic color from your wallpaper, tonal surfaces, a
navigation drawer, cards, chips and data tables.

It's a rewrite of [1234nin4321/PodStack](https://github.com/1234nin4321/PodStack) (Electron + React, itself a fork of
Cerebral). The original code is kept in [`legacy/`](legacy) for reference. The 3D ship viewer isn't part of the
native app.

## Features

- Add characters through EVE SSO v2 (PKCE).
- Overview of all characters: Alpha/Omega, corp/alliance, ISK, SP, skill in training and time left.
- SP farming: farms with a base SP to keep, injectors ready, time to the next one, queue time left, SP/hour, and
  ISK per farm per 30 days from Jita prices after fees, extractors and Omega/MCT costs.
- Queue health across all characters.
- Contracts, market orders, industry jobs and planetary industry across all characters, with drawer badges for
  orders expiring, jobs ready and extractors stopped.
- Asset search across all characters with estimated values.
- Per character: summary, skills, skill plans (remaps, implants, accelerators, skillbooks, import/export, paste
  from EVE/EVEMon, fit planner), clones, mail, wallet (journal, transactions, balance chart), market, contracts,
  assets, industry, research agents, PI, notifications, calendar, standings, contacts, kill log, medals, and ESI
  token/scope health.
- Skill browser and ship browser with time-to-train for any character.
- Desktop alerts (training stopped or low, lapsed to Alpha, injector ready, contract completed, new mail, jump
  fatigue ended, industry job ready, PI extractor stopped) through your notification daemon.
- Tray icon (StatusNotifierItem) with close-to-tray.
- Password-protected backup and restore.

## Theming

Settings → Appearance:

- **Wallpaper (matugen)** — the default when [matugen](https://github.com/InioX/matugen) is installed. If your
  matugen config includes PodStack's template ([`packaging/matugen`](packaging/matugen)), PodStack uses exactly the
  colors the rest of your desktop gets and re-themes live when you change wallpaper. Otherwise it runs matugen on
  your current wallpaper (`~/.cache/current-wallpaper`, hyprpaper or swww), or on an image you pick.
- **EVE factions** — Amarr, Caldari, Gallente, Minmatar, Jove, ORE, CONCORD, EDENCOM, Triglavian, Sansha's Nation,
  Guristas, Angel Cartel, Blood Raiders, Serpentis.
- **Custom** — any seed color.

Every source produces a full Material 3 scheme (dark, light or follow the system), with a choice of scheme variant
(tonal spot, vibrant, expressive, …) and contrast.

## Install (Arch)

```sh
cd packaging/arch
makepkg -si
```

The PKGBUILD builds the `rust-rewrite` branch from GitHub. Optional: `matugen` (wallpaper colors), a notification
daemon (mako, dunst, swaync), and a bar with a tray (waybar, quickshell).

Hyprland rules for the dialogs are in [`packaging/hyprland/podstack.lua`](packaging/hyprland/podstack.lua).

## Build from source

```sh
cargo run --release
```

Needs Rust (stable) and GTK 4.20+. Data lives in `~/.local/share/podstack` (tokens in `auth.json`, mode 0600) and
caches in `~/.cache/podstack`. Set `PODSTACK_DATA_DIR` / `PODSTACK_CACHE_DIR` to use other folders and
`PODSTACK_LOG=debug` for verbose logs.

## Logging in

By default PodStack uses its EVE application, whose callback is `https://localhost/callback`: after you authorize,
the browser can't open that page; copy the address from the address bar into PodStack. To skip that step, register
your own application at [developers.eveonline.com](https://developers.eveonline.com) with the scopes PodStack uses
and a callback like `http://127.0.0.1:8765/callback`, and enter its client id and callback in Settings → EVE login:
PodStack then catches the login itself.

## License

AGPL-3.0-only, like the original. Bundled fonts: Inter (OFL) and Material Icons (Apache 2.0), see
[`assets/fonts/LICENSES.md`](assets/fonts/LICENSES.md).
