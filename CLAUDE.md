# CLAUDE.md

PodStack (native) is a Rust rewrite of the Electron/React PodStack (an EVE Online character manager) for Linux,
built for Arch + Hyprland with GTK4 and a Material You (Material 3) design. The original app lives in `legacy/`
for reference: port behaviour from it, but don't edit it. The 3D ship viewer is intentionally not ported.

## Layout

- `crates/core` (`podstack-core`): no GTK. SSO (`sso.rs`), ESI client with rate limiting (`esi.rs`), tokens
  (`auth.rs`), cached universe lookups (`universe.rs`: names, types, systems, stations, structures, `Place`), market
  prices (`market.rs`), static data (`sde.rs`, from `data/*.json`), training math (`training.rs`), formatting
  (`fmt.rs`), the `Character` model (`character.rs`), per-kind ESI fetches (`domains/`), page calculations and stored
  feature data (`features/`), and `Core` + `Event` + `Settings` (`lib.rs`).
- `crates/app` (`podstack`): GTK4 UI. `main.rs`, `shell.rs` (window, navigation drawer, top bar, snackbar, live
  rebuilds), `ctx.rs` (`Ctx`, `Route`, `CharTab`, `Page`), `widgets/` (Material 3 components and `Table`),
  `theme/` (matugen / faction / custom color schemes → stylesheet), `pages/` (one module per destination;
  `pages/character/` per tab), `services/` (alerts, tray), `login.rs`, `images.rs`, `rt.rs`.
- `data/`: static data JSON. `skills.json`, `ships.json` and friends come from `legacy/resources`
  (`tools/convert_legacy_data.py`). `market_types.json`, `market_groups.json`, `universe.json` and `industry.json`
  are a snapshot of CCP's static data export (`cargo run -p podstack-core --example convert_sde -- <unzipped SDE>`);
  at runtime `static_data.rs` downloads newer CCP builds by itself and swaps them in (`Live<T>` statics:
  `sde::MARKET`, `sde::MAP`, `blueprints::INDUSTRY`). Don't hard-code game numbers that the SDE or ESI carries;
  rules only in patch notes go in `features::manufacturing::Rules` (user-editable).
- `assets/`: icon, fonts (Inter, Material Icons, as TTF: HarfBuzz can't shape WOFF2).

## How data flows

- `Core::load()` reads everything from `$XDG_DATA_HOME/podstack`; `Core::start()` runs the background refresher on
  Tokio (every 15s, fetches whatever is due, 2 characters at a time).
- Each data kind is a `DomainSpec` in a `domains/*.rs` module's `DOMAINS` list: `domain!("key", "Label", seconds,
  fetch_fn)`. `async fn fetch_fn(cx: &Cx) -> DomainResult` calls ESI through `cx.get(cx.char_path("…"))
  .scope("esi-…").json().await?` (or `.all_pages()`), resolves names with `cx.universe()`, and returns
  `Ok(Box::new(move |c| c.field = Some(data)))`. Never hold locks across awaits. Missing scope → return the
  `ScopeMissing` error (`.scope()` does it) and the scheduler stops retrying until re-login.
- `Character` fields are `Option<…Data>` per domain; types live in that domain's module. Feature state that isn't ESI
  data goes in `Character::local` (`c.local::<T>("feature.key")`, `core.update_character(id, |c| c.set_local(...))`)
  or in a document (`store::load_doc` / `store::save_doc`, then `core.emit(Event::Doc("name".into()))`).
- App-wide settings: `core.setting::<T>("key")` / `core.set_setting("key", &value)` (stored in `settings.json`).

## UI conventions (Material 3)

- A page is `pub fn build(ctx: &Ctx) -> Page` (character tabs: `build(ctx: &Ctx, id: i64) -> Page`). Use
  `widgets::page_body()` for the standard padding on top-level pages (character tabs are already padded by the
  character frame: return a plain `w::vbox(16)`).
- `Page::new(title, content)` is scrolled and **live**: the shell rebuilds it ~0.8s after the data it shows changes,
  keeping the scroll position. Pages with big tables use `.no_scroll()` and a `Table` (it scrolls itself). Pages with
  text input or selections keep that state with `ctx.remember("page.key", v)` / `ctx.recall("page.key")` so a rebuild
  doesn't lose it, or use `.static_page()` and update themselves through `ctx.on_event(&widget, |e| …)`.
- Never block the GTK thread on the network: `rt::run(async move { … }, move |result| { …update widgets… })`.
- Build with `widgets` (`card`, `card_with`, `stat_tile`, `tile_row`, `flow`, `chip`, `status`, `badge`,
  `empty_state`, `banner`, `scope_notice`, `button`/`button_icon`/`icon_button` with `Btn::{Filled,Tonal,Outlined,
  Text,Elevated,Danger,DangerFilled}`, `segmented`, `switch_row`, `list_row`, `key_values`, `skill_pips`,
  `search_entry`, `dropdown`, `dialog`, `confirm`, `ticker`, `avatar`/`eve_image`, `Table`). Icons are Material Icons
  ligature names (`w::icon("rocket_launch")`, see fonts.google.com/icons?icon.set=Material+Icons).
- Colors come only from the scheme's roles via the style classes in `theme/css.rs` (`dim`, `primary-text`,
  `error-text`, `success-text`, `warning-text`, `status-*`, `card*`, type scale classes like `title-medium`). Don't
  hard-code colors; for cairo drawing use `theme::rgba("primary")`. A page needing its own styles registers them with
  `crate::theme::add_css(CSS)` (a `const CSS: &str` in the page module, prefixed class names, roles only).
- Empty data: `w::empty_state(icon, title, body)`; data needing a missing scope: `w::scope_notice(scope)` (check
  `c.refreshes.get(key).and_then(|r| r.error.as_deref()) == Some("scope")`).
- Numbers: `podstack_core::fmt` (`isk`, `isk_short`, `int`, `sp`, `duration`, `relative`, `datetime`, `escape` for
  Pango markup).

## Working rules

- `cargo build` and `cargo test` must pass with no warnings in code you touched; `cargo clippy` should be clean.
- Comments explain why, in plain sentences. Match the surrounding style.
- Commit as the repo's configured git identity. Push only when the user says so.
- Run the app against throwaway data with `PODSTACK_DATA_DIR=/tmp/x/data PODSTACK_CACHE_DIR=/tmp/x/cache
  cargo run`; logs via `PODSTACK_LOG=debug`.

## EVE SSO and ESI

- Default login uses PodStack's EVE application (`config::DEFAULT_CLIENT_ID`, callback `https://localhost/callback`):
  the user pastes the address the browser lands on. Users can set their own application with a loopback callback
  (`http://127.0.0.1:<port>/callback`) in Settings, which PodStack listens on.
- Refresh tokens only work with the client id that issued them, so each character stores `client_id`.
- Adding a scope needs it enabled on the EVE application at developers.eveonline.com first.
