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

Mesh wears your desktop. By default it follows your rice:

- **Colours** from `~/.config/themes/active/colors.conf` (`$bg2`, `$bg1`,
  `$surface`, `$accent`, `$text` as `$key = rgba(rrggbbaa)`; override the path
  with `MESH_THEME_FILE`). The other colours (bright and muted text, borders,
  text on accent) are derived, and muted text is lightened until it passes
  4.5:1 contrast.
- **Gaps, border width and rounding** from `~/.config/hypr/hyprland.conf`
  (`general { gaps_in gaps_out border_size }`, `decoration { rounding }`,
  following `source = …` includes). Set `layout_from_hyprland = false` in
  `config.toml` to use the defaults instead.

Both files are watched: edit them and Mesh restyles within a second.
Missing files fall back to the defaults in [design/DESIGN.md](design/DESIGN.md).

Settings → **Rice** (gear icon in the user panel) switches between "Follow my
rice", the built-in presets (Catppuccin Mocha, Nord, Gruvbox Dark) and "Custom
theme.toml", shows every colour token (and which are derived), the layout
values, the message density, and four switches: digital rain, translucent
panes, network tags and plain-text mode (no image downloads). "Export theme"
copies the current look as a theme.toml to the clipboard. Custom theme.toml is
read from `~/.config/mesh/theme.toml` (`%APPDATA%\mesh\theme.toml` on
Windows), see [theme.example.toml](theme.example.toml). The choice persists in
`~/.config/mesh/config.toml` (`%APPDATA%\mesh\config.toml` on Windows).

### Discord voice button

Add `discord_voice_url = "https://…"` to `config.toml` and a 🎧 button appears
in the channel pane that opens it in your browser, e.g. a self-hosted browser
running Discord web, for voice alongside a text-only Discord bridge. Or set
`discord_voice_command = "…"` to have the button run a command instead (say, a
script that shows or hides a voice window); it wins over the URL.

### Discord voice inside Mesh

Mesh can drive a hidden official Discord web client for voice
(`crates/mesh-discord`): it connects to the client's DevTools socket
(`discord_engine_socket` in `config.toml`, default
`$XDG_RUNTIME_DIR/discord-engine.sock`), installs a small script in the page
and reads Discord's own voice state. You get Discord voice channels (with who's
talking) in the channel pane, a call button in bridged Discord DMs, a voice bar
in the chat pane, and the user panel's mic / deafen / leave buttons acting on
the Discord call. Set `discord_engine_command` to start that client when Mesh
starts and it isn't running. On Linux the client itself (a browser behind a
VPN) is set up outside Mesh; see design/DISCORD-NATIVE.md.

On Windows the engine is Edge with DevTools on `127.0.0.1:9222` (the default
there; `discord_engine_socket` also takes any `host:port`). Put this in
`%APPDATA%\mesh\config.toml`, start Mesh, log into Discord once in the window
that opens and link WhatsApp in its second tab (WhatsApp → Linked devices);
minimize it after that, but don't close the tabs:

```toml
discord_engine_command = 'start "" msedge --remote-debugging-port=9222 --user-data-dir=%LOCALAPPDATA%\mesh\discord-engine --app=https://discord.com/channels/@me --no-first-run --autoplay-policy=no-user-gesture-required --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows https://web.whatsapp.com/'
```

WhatsApp chats then show on Home (the `whatsapp` chip filters to them), open
in the chat pane, send, show photos and notify; see design/WHATSAPP-NATIVE.md.
WhatsApp calls work too.

Any local program can use that port to drive the Discord and WhatsApp
sessions in it, so only run it on a PC you trust.

### Native WhatsApp and Discord (no browser)

Two helper programs replace the engine browser: `mesh-wa` (WhatsApp, on
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust)) and `mesh-dc`
(Discord: gateway, REST and voice with DAVE). Each is its own cargo project;
build them with `cargo build --release` in `mesh-wa/` and `mesh-dc/` (they need
`cmake` and `clang`, plus ALSA headers on Linux) and put the binaries next to
Mesh's own. Mesh uses a helper when it finds it there and the engine browser
otherwise. Link WhatsApp from the QR code on the Accounts page; sign in to
Discord there with email and password (or its QR code). Not native yet: video
and screen sharing.

On Linux, where Discord goes through a VPN, have `mesh-dc` run inside the
VPN's network namespace too, or it connects from your own IP. In
`~/.config/mesh/config.toml`, give the command Mesh should put in front of the
helper (it must work without a password prompt: Mesh has no terminal):

```toml
discord_native_wrapper = ["sudo", "-n", "ip", "netns", "exec", "vpn", "sudo", "-n", "-u", "you"]
```

If the wrapper can't start, Discord stays offline; it never falls back to a
direct connection.

## Build & run

```sh
cargo run -p mesh-app
```

Session data (access token + sqlite store, including E2EE keys) is kept in the
platform data dir (`~/.local/share/mesh` on Linux).

## Windows

`installerbuild.ps1` builds Mesh, both helpers and `dist\Mesh-Setup-<version>.exe`
(needs [Inno Setup 6](https://jrsoftware.org/isinfo.php):
`winget install JRSoftware.InnoSetup --scope user`). The setup installs per user
without an admin prompt, adds Mesh to the Start menu and Settings → Apps, and
can add a desktop shortcut and start Mesh at sign-in. Uninstalling keeps your
accounts and chats in `%APPDATA%\mesh`.

The whole stack is pure Rust — build on a Windows machine with
`cargo build --release`, or cross-compile with
`cargo build --target x86_64-pc-windows-gnu` (needs `mingw-w64`).

## Status

Working today:

- Password login and session restore across restarts.
- Room list sorted by recency, split into "Direct messages" and "Rooms"
  sections, with unread badges.
- Timeline for the selected room: messages grouped by day and by author,
  anchored to the newest message, from the latest 50 events. Fenced code blocks
  and `inline code`, images (downloaded and decrypted, with a filename · size
  caption), reaction pills (click to add yours or take it back), joins, leaves
  and renames as small system lines, and "… is typing" under the composer.
  Three densities: `cozy` (default), `compact` (no avatars, one line per
  message) and `irc` (`[14:02] <nick> text`), set with `density = "compact"`
  in `config.toml`.
- Push-driven updates: the UI refreshes when sync delivers events (no polling).
- Sending messages (composed as Markdown — `**bold**` etc. works).
- E2EE device verification: incoming verification requests show a banner with
  accept/decline and the emoji SAS confirm flow. Verify Mesh from another
  client (e.g. Element) to unlock encrypted rooms.
- Secure Backup (server-side key backup + recovery). The Settings screen shows
  backup status, sets it up (and shows a recovery key to save once), and can
  restore your message keys from an existing backup with that recovery key —
  so keys survive across sessions instead of being lost.
- Settings with a left nav: Accounts & bridges (your account, key backup
  state, log out, Discord bridge status and a one-time warning about Discord's
  rules), Privacy & E2EE (Secure Backup), Rice, Keybinds, Voice & audio
  (devices). `Esc` goes back.
- Add friend (top bar, or Settings → Friends): paste a Matrix ID or a
  Discord username and Mesh works out which it is (or force matrix/discord),
  with specific errors for `name#1234` and `@name` without a server. For
  Matrix it looks up the profile and opens an encrypted DM invite (Matrix has
  no friend list; the DM is recorded in `m.direct`). The pending list shows
  incoming DM invites (accept / ignore) and invites you sent (cancel).
- Tiled layout like a Hyprland desktop: top bar, space rail, channel pane,
  chat pane and member pane, separated by your `gaps_out` with your border
  width and rounding. The focused pane gets the accent border. A user panel
  (avatar, MXID, mic, deafen, settings) sits under the channel list. The member
  pane toggles from the chat header and hides on narrow windows. `Esc` closes
  settings. IBM Plex Sans and JetBrains Mono are bundled.
- Spaces: rooms are grouped by their `m.space`. The rail shows Home (DMs and
  rooms outside any space) and one tile per space with its mention count;
  `Alt+1..9` switches space. `Ctrl+K` opens a jump palette over all rooms and
  spaces (type, then Enter or click).
- Network tags: Mesh spots Discord rooms bridged by mautrix-discord (their
  `m.bridge` state event) and Discord users (bridge ghosts, `@discord_<id>:…`),
  and tags them `dc`; everything else is `mx`. Tags show on space tiles,
  message authors (`mx · server`), members and the top bar's connection chips.
  In bridged rooms the composer says the message goes to Discord via the
  bridge. Mesh only ever talks Matrix; Discord is reached through the bridge.
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
history pagination on scroll-up, room avatars, sending our own typing
notices, read receipts, sending attachments (the 📎 button is disabled),
adding a new reaction (only existing pills can be clicked), notifications,
Discord friend requests (mautrix-discord can't send them; the button is
disabled with an explanation), a separate `org.mesh.contacts` list (DMs in
`m.direct` serve as the contact list), joining or creating spaces, voice
channels, multiple accounts, choosing a third-party bridge, rebinding keys,
and sharing a theme to a room.
