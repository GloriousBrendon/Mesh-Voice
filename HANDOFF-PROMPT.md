# Prompt for continuing Mesh

Copy everything below the line into your AI model, run from the repo root.

---

You are working on **Mesh**, a lightweight Matrix chat client in Rust, located in this repository. The scaffold already compiles and runs; your job is to build it out into a genuinely usable daily-driver client. Do not rewrite the architecture — extend it.

## Current state (verified compiling)

- Cargo workspace with two crates:
  - `crates/mesh-core` — all Matrix logic, zero UI dependencies. Wraps `matrix-sdk` 0.18 (features: `e2e-encryption`, `sqlite`, `markdown`). Provides `MeshClient` with: `login_with_password()`, `restore()` (from a persisted `session.json` + sqlite store), `run_sync()` (long-running sync loop), `rooms()` → `Vec<RoomSummary>`, `send_message()`, `logout_locally()`. Session persistence is in `src/session.rs` (`SessionStore`, plain JSON).
  - `crates/mesh-app` — the GUI, built on `iced` 0.14 (features: `tokio`, `image`) using the function-based API: `iced::application(boot, update, view).theme(..).subscription(..).run()`. Screens: `Booting` (session restore), `LoggedOut`/`LoggingIn` (login form), `LoggedIn` (sidebar room list + message composer). The room list currently refreshes via a crude 3-second `iced::time::every` poll.
  - `crates/mesh-app/src/theme.rs` — `Palette`: 10 named hex colors loaded from TOML (`~/.config/mesh/theme.toml` on Linux, `%APPDATA%\mesh\theme.toml` on Windows), with Catppuccin Mocha default, Nord and Gruvbox presets, and `to_iced_theme()` which builds an `iced::Theme::custom(..)`.

## Design constraints (non-negotiable)

1. **One codebase for Linux and Windows.** Pure-Rust stack only; no GTK/Qt. Everything must build with plain `cargo build` on both platforms.
2. **All Matrix logic stays in `mesh-core`**, UI-free. The iced app talks to it only through `MeshClient` and plain data structs (`RoomSummary`, etc.). Keep it that way so the frontend stays swappable.
3. **Theming stays config-driven.** Every color the UI uses must come from `theme::Palette` — no hardcoded colors in widget code. The user rices Linux by editing `theme.toml`; Windows users pick a built-in preset.
4. `mesh-core` compiles fast; iced pulls a heavy graph. Don't add dependencies casually.

## Priority order — work through these

1. **Timeline rendering.** Show messages for the selected room. Use `matrix_sdk`'s room event access (or add the `matrix-sdk-ui` crate's `Timeline` if it earns its weight) and expose it from `mesh-core` as a plain struct (`TimelineMessage { sender, body }` already exists as a starting point). Render sender display names, message bodies, and timestamps grouped by day. Newest at the bottom, scrolled into view.
2. **Replace the 3-second poll with real push updates.** `matrix-sdk` exposes room/event observers (`add_event_handler`, room info streams). Bridge them to iced via `iced::Subscription`/`Task::stream` with a `tokio::sync::mpsc` channel so the UI updates when sync delivers something, not on a timer.
3. **E2EE verification UX.** Encrypted rooms already work at the store level (sqlite crypto store is wired). Add device verification: show verification requests, support emoji SAS confirm/deny. Without this, encrypted rooms are unreadable in practice.
4. **In-app theme switcher.** Settings screen listing `theme::presets()` + "custom from theme.toml"; persist choice. On Linux also add a "watch theme.toml and hot-reload" toggle (notify crate or simple mtime poll in a subscription).
5. **Room niceties**: unread badges (already have counts), room avatars (`image` feature is enabled), sorting by recency, DM vs room sections, markdown rendering of formatted bodies (`markdown` feature of matrix-sdk is enabled; iced has a markdown widget).
6. **Quality of life**: typing indicators, read receipts, pagination of history on scroll-up, image attachments (view + send), notification on new message (`notify-rust` works on both platforms).

## Verification

- `cargo check --workspace` must stay clean.
- Test flows against a real account on `https://matrix.org` or a local Synapse/Conduit; login, restore after restart, send/receive in both a plaintext and an encrypted room.
- Keep `README.md` honest as features land.

## Style

- Rust 2024 edition, workspace already set. Match existing naming (`MeshClient`, `MeshError`, `Screen`, `Message`).
- Errors: `thiserror` in mesh-core, stringly `Result<_, String>` only at the iced message boundary.
- No `unwrap()` on network/store paths; surface errors to the UI status line.
