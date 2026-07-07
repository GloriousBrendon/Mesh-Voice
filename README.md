# Mesh

A lightweight Matrix client written in Rust. One codebase for Linux and Windows:
pure-Rust rendering via [iced](https://iced.rs), Matrix via
[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk).

## Layout

```
crates/
  mesh-core   Matrix logic: login, session persistence, sync, rooms, timeline,
              push updates, SAS device verification, sending, and 1:1 voice
              calls (WebRTC + Opus, pure Rust). No UI dependencies — any
              frontend can sit on top of it.
  mesh-app    The iced GUI: login screen, room list, timeline, message
              composer, call controls, settings/theme switcher, theme loader.
```

## Theming

Mesh reads a TOML palette at startup:

- Linux: `~/.config/mesh/theme.toml`
- Windows: `%APPDATA%\mesh\theme.toml`

Copy [theme.example.toml](theme.example.toml) there and edit the hex values to
match your rice. Missing or broken file → falls back to Catppuccin Mocha.

The in-app settings screen (gear icon in the sidebar) switches between the
built-in presets (Catppuccin Mocha, Nord, Gruvbox Dark) and "Custom
(theme.toml)"; the choice persists in `~/.config/mesh/config.toml`
(`%APPDATA%\mesh\config.toml` on Windows).

## Build & run

```sh
cargo run -p mesh-app
```

Session data (access token + sqlite store, including E2EE keys) is kept in the
platform data dir (`~/.local/share/mesh` on Linux).

## Windows

The whole stack is pure Rust — build on a Windows machine with
`cargo build --release`, or cross-compile with
`cargo build --target x86_64-pc-windows-gnu` (needs `mingw-w64`).

## Status

Working today:

- Password login and session restore across restarts.
- Room list sorted by recency, split into "Direct messages" and "Rooms"
  sections, with unread badges.
- Timeline for the selected room: sender display names, timestamps, messages
  grouped by day, anchored to the newest message. Loads the latest 50 events.
- Push-driven updates: the UI refreshes when sync delivers events (no polling).
- Sending messages (composed as Markdown — `**bold**` etc. works).
- E2EE device verification: incoming verification requests show a banner with
  accept/decline and the emoji SAS confirm flow. Verify Mesh from another
  client (e.g. Element) to unlock encrypted rooms.
- In-app theme switcher with persisted choice.
- Discord-style layout: three panes (rooms │ chat │ member list), a persistent
  bottom-left user panel (your avatar/name + mic-mute and deafen toggles),
  default initials avatars, and messages grouped by author. The member list
  collapses on narrow windows.
- Animated "digital rain" background (an iced canvas), drawn in the palette's
  colors so it still follows your rice/theme. Side panels are slightly
  translucent so it glows faintly behind them.
- 1:1 voice calls. A "📞 Call" button in the room header rings the other
  member; incoming calls show an accept/decline bar; in-call controls are
  mute and hang up. Media is WebRTC with the Opus codec; ICE uses the
  homeserver's TURN server, so calls traverse NAT. Uses the legacy Matrix
  1:1 VoIP events (`m.call.invite`/`answer`/`candidates`/`hangup`, v1), so it
  interoperates with Element's 1:1 calls.

  Caveats: audio has **no echo cancellation** yet — use headphones or you will
  echo. One call at a time, voice only (no video). Requires a working input
  and output device on both ends.

Not there yet: echo cancellation, video calls, group (Element Call) calls,
history pagination on scroll-up, room avatars, typing indicators, read
receipts, image attachments, notifications.
