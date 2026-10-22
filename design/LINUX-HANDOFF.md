# Linux handover (2026-10-22)

For the Claude CLI agent on the Linux box. The Windows side moved Mesh off
the hidden Edge engine onto two native helper programs. Your job: build
everything here, install it next to Mesh, keep Discord behind the VPN, and
report what works. Read README.md first; this covers what changed and
what to watch.

## What changed since the last Linux build

- **`mesh-wa/`**: native WhatsApp helper on whatsapp-rust. It covers chats,
  media, reactions, receipts, archive/delete/forward, and 1:1 and group audio
  calls. Live-tested on Windows: chats and an audio call (including switching
  audio devices mid-call). Camera and screen share go through `mesh-media`,
  which captures nothing on Linux yet (see below).
- **`mesh-dc/`**: native Discord helper. Covers the gateway, REST, QR or
  email+password sign-in (with 2FA), and voice through songbird with DAVE.
  Live-tested on Windows: sign-in, servers, DMs, messages and joining voice.
  No video or screen share; those commands are accepted and do nothing.
- **`mesh-media/`**: shared capture and H.264 crate, used by mesh-wa. On Linux
  `list_cameras` and `list_sources` return nothing, and `Capture` returns "not
  supported on this platform". It builds, and its tests pass in a `rust:1.98`
  Debian container.
- **Each helper is its own cargo project**, excluded from the root workspace:
  whatsapp-rust's SQLite can't link next to matrix-sdk's. Mesh talks to each
  over stdio, one JSON object per line. Mesh uses a helper only when the
  binary sits **next to Mesh's own binary** (`mesh-wa`, `mesh-dc`, no
  extension). Otherwise it falls back to the old engine browser
  (`discord_engine_socket` / `discord_engine_command`).
- **Windows-only additions** are gated with `cfg(windows)` and do nothing on
  Linux:
  - the exe icon and version info (`build.rs`, winresource)
  - the DWM-themed title bar (`crates/mesh-app/src/titlebar.rs`)
  - the tray icon (`crates/mesh-app/src/tray.rs`, tray-icon with no default
    features)
  - the Inno Setup installer (`installer/`)

  On Linux, closing the window still quits.
- New `discord_native_wrapper` config key (see Discord below).

## Build

Build dependencies, on top of what Mesh already needed: `cmake`, `clang`,
ALSA headers, `pkg-config`, dbus, openssl, and xkbcommon/wayland/x11. Package
names:

- Debian: `pkg-config libasound2-dev libdbus-1-dev libssl-dev libxkbcommon-dev libwayland-dev libx11-dev cmake clang`
- Arch: `pkgconf alsa-lib dbus openssl libxkbcommon wayland libx11 cmake clang`

```sh
cargo build --release -p mesh-app                     # target/release/mesh-app
(cd mesh-wa && cargo build --release)                 # mesh-wa/target/release/mesh-wa
(cd mesh-dc && cargo build --release)                 # mesh-dc/target/release/mesh-dc
cargo test -p mesh-app -p mesh-discord -p mesh-core
(cd mesh-wa && cargo test --release); (cd mesh-dc && cargo test --release); (cd mesh-media && cargo test --release)
```

Expected test counts: mesh-app 20, mesh-discord 14, mesh-core 9, mesh-wa 9,
mesh-dc 11, and mesh-media 3 plus 1 ignored (the webcam test). If whatsapp-rust
fails to fetch, retry: the git fetch has been flaky once.

## Install

Find how Mesh is installed on this machine now: binary location, launcher,
any `.desktop` entry. Copy `mesh-wa` next to that binary first, and leave
`mesh-dc` out until the VPN wrapper is configured and verified.

## Discord: keep it behind the VPN (important)

On this box Discord deliberately goes through a VPN: the engine browser runs
in the `discord-voice` network namespace (see design/DISCORD-NATIVE.md).
`mesh-dc` run plainly would connect from the real IP. Before installing
`mesh-dc`:

1. Work out how the existing `discord-voice` launcher enters the namespace:
   `ip netns exec`, `nsenter`, a firejail profile or similar.
2. Put the equivalent command prefix in `~/.config/mesh/config.toml`. Mesh
   appends `<mesh-dc path> --data <dir>` to it. For example:
   ```toml
   discord_native_wrapper = ["sudo", "-n", "ip", "netns", "exec", "vpn", "sudo", "-n", "-u", "<user>"]
   ```
   The prefix must not prompt: Mesh has no terminal. For `sudo` that means a
   sudoers rule. If the wrapper fails, Discord stays offline: it fails closed
   and never goes direct.
3. **Verify the exit IP before signing in.** Run
   `<wrapper…> curl -s https://ifconfig.me`, compare it with the plain
   `curl -s https://ifconfig.me`, and show the user both.
4. Only then install `mesh-dc` and have the user sign in from Accounts → Discord
   (email + password; or QR). Voice uses UDP, so check the namespace allows UDP
   out as well.

**Caution:** an earlier bug made the gateway reconnect and log in again every
few seconds, and Discord invalidated the token. It's fixed: big zlib messages
now inflate whole, reconnects back off up to 5 minutes, and only close code
4004 forgets the token. Even so, if you see repeated "gateway:" warnings in
the helper's stderr, stop `mesh-dc` at once rather than letting it loop. To
watch it, run it by hand:
`sleep infinity | <wrapper…> ./mesh-dc --data /tmp/dc-test`. It logs to stderr
and speaks the protocol on stdout.

## WhatsApp on Linux

- Each machine is its own WhatsApp linked device. The Linux Mesh needs its own
  QR link from Accounts → WhatsApp (phone: Linked devices → Link a device).
  That doesn't affect the Windows link.
- The helper keeps its data in `~/.local/share/mesh/whatsapp/`
  (`whatsapp.db`, `chats.json`). Discord's goes in
  `~/.local/share/mesh/discord/token`.
- Camera and screen share in WhatsApp calls won't work on Linux until
  `mesh-media` gets a Linux backend. Audio calls should work: cpal over ALSA,
  which PipeWire and Pulse provide.

## Privacy rules for this project

The whole point of Mesh is that WhatsApp and Discord don't collect telemetry
or personal data on the user.

- Never call Discord's `/science` or tracking endpoints.
- whatsapp-rust's WAM analytics plugin is not compiled in; keep it that way.
- Prefer sign-in flows that don't involve the vendors' phone apps where
  possible. Discord uses email + password.
- Native Discord video and screen share are deliberately out of scope. Don't
  build them.

## Possible follow-ups (ask the user first)

- A Linux backend for `mesh-media`: PipeWire screencast portal for screens,
  v4l2 for the camera.
- A `.desktop` file with `crates/mesh-app/assets/mesh.png` as the icon.

## Report back to the user

Report:

- the build and test results
- where things were installed
- the wrapper used and both exit IPs
- what works on a real run: WhatsApp link, chats, an audio call; Discord
  sign-in, chats, voice behind the VPN

Say plainly what wasn't tested. Commit any fixes to `main` with clear
messages and push.
