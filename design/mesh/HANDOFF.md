# Handover: Mesh (2026-10-22)

This is for a new Claude session on Brendon's personal machine. It continues
work begun on a work laptop. Read this file, then `design/mesh/CAPABILITY-MAP.md`.
Nothing from the earlier session's memory carries over; this file is the
memory.

## Who and what

- **Brendon** owns all of this; GitHub user `GloriousBrendon`.
- **Main goal: Mesh.** A serverless, decentralised Discord replacement in
  which every install is a node. There are no accounts to register, no
  telemetry and no data collection. Privacy is the reason it exists. It has to
  work exactly like Discord, be lightweight, and be "download it and use it":
  no admin rights, no VPN, no port forwarding. **The aim is a fully working,
  deployable app**, not a prototype.
- **Mesh-Voice** (`GloriousBrendon/Mesh-Voice`, this repo) started as a
  stepping stone. It is a Rust/iced desktop client (Windows and Linux) that
  puts Matrix, Discord and WhatsApp into one Discord-style app, with native
  helper processes instead of browsers. It is in daily use and works. Mesh
  becomes another network inside it, with **Mesh-Voice as the app people
  download**.
- **Mesh-Node** (new repo, **private**, `GloriousBrendon/Mesh-Node`, **not
  created yet**: the work laptop's `gh` was signed in to a work account) holds
  the backend/engine. It is one Rust binary with two modes:
  - a helper Mesh-Voice drives over stdio, the same pattern as `mesh-wa` and
    `mesh-dc`;
  - headless, as an always-on seed node.
- **Original Mesh** (`GloriousBrendon/Mesh`) is **reference only; don't work
  in it.** About 4,800 lines of design docs in `docs/` (identity, Spaces,
  voice/video, storage, abuse prevention, presence, a STRIDE threat model) are
  worth reusing where they still hold. Its code stalled on WireGuard, Wintun,
  OpenDHT, mediasoup and Tauri: kernel tunnels needing admin rights. That
  stack is dropped.

## How the network works (agreed)

- **Identity.**
  - Ed25519 keypair. The permanent ID is the public key, carried in QR codes,
    `mesh://` links and copy and paste.
  - The local keystore is encrypted with a password chosen at onboarding,
    together with a username. The password never leaves the device.
- **Friend codes** are what people share by hand, about 8 characters like
  `K7QM-3XPA`.
  - They work like Discord invites: they expire (24 hours by default), can be
    single-use and can be revoked. Lookups are rate-limited.
  - A code resolves to the full ID through the seeds or the BitTorrent
    network's public lookup system (pkarr-style signed records).
  - After a request, both people can compare a safety number.
- **No search or discovery.** You can reach someone only with an ID or code
  they gave you. Anyone can be blocked.
  - The block list is per account, kept on your devices and synced between
    them. You keep blocked IDs.
  - ID rotation is not in version 1.
- **Transport.**
  - QUIC by public key, starting with `iroh`: hole punching, relay fallback,
    published addresses. That handles dynamic IPs without static IPs or port
    forwarding.
  - Relays see only ciphertext. Brendon is fine with writing our own if iroh
    falls short.
- **Reaching someone by ID**, in order:
  1. direct, if they're a friend with a known current address;
  2. a blinded friend-of-friend lookup a few hops deep (each hop checks a
     per-query blinded fingerprint against its own friends, so the hops in
     between can't tell who's being looked for);
  3. seed rendezvous as the fallback, which matters most while the network is
     small.
- **Store-and-forward.** If the recipient is offline, mutual friends or seeds
  hold the encrypted message, with expiry, quotas and sealed sender, and
  deliver it when the recipient comes back. If both people are online, the
  message goes direct.
- **Spaces (servers)** are signed, append-only logs copied among members.
  Every member stores them, and online members serve offline ones. Version 1
  targets up to a few hundred members.
- **Calls.**
  - 1:1 is direct.
  - Groups use a relay tree of the nearest members, with each person getting
    the call from whoever is closest.
  - Media is end-to-end encrypted with SFrame, so relays carry only
    ciphertext.
  - Reuse Mesh-Voice's `mesh-media` crate for camera and screen capture with
    H.264, and Opus for audio.
- **Several devices per person**, if they choose it. Linking a device needs
  one existing device online; there is no server copy.
- **Seeds.**
  - The homelab is the first seed, hardened: only the QUIC/relay port is open
    to the internet, admin is over Tailscale only, it runs in a container, and
    it holds ciphertext only with quotas, expiry and rate limits.
  - A second seed on a VPS comes after testing proves the homelab seed works.
  - Seeds are ordinary nodes that are always on, so they can be phased out
    later.
- **Desktop only for version 1** (Windows and Linux). Mobile comes later as
  light clients, since phones can't stay on as nodes.

## Where it differs from Discord (accepted)

- No search.
- History comes from members who are online.
- Deleting a message is a request to other devices, which a modified client
  can ignore.
- Large Spaces (50,000+ members) are future work.
- Group video is limited by home upload speeds.

## Process (Brendon asked for this)

Use the agent-skills workflow, starting with
`/agent-skills:using-agent-skills`:

- spec-driven-development, gated, with the human approving each phase;
- then planning-and-task-breakdown;
- then incremental implementation in thin slices that can be checked.

**Done so far:** Phase 0, the capability map, is approved. It lives at
`design/mesh/CAPABILITY-MAP.md` and lists 12 modules with a build order:

**identity → transport → seed → routing → social → e2ee → dm → mailbox →
spaces → calls → packaging**

Mesh-Voice UI work comes with each module.

**Next:**
1. Create the private `GloriousBrendon/Mesh-Node` repo:
   `gh repo create GloriousBrendon/Mesh-Node --private`, signed in as
   GloriousBrendon.
2. Write `design/mesh/SPEC-identity.md`, covering the six areas: objective,
   commands, structure, style, testing and boundaries. Include success
   criteria and open questions. Then **stop for approval**.
3. Write each later module's spec only after the one before it is approved.

**Checking:** each module must pass a test between two real machines, for
example the Windows PC and the Linux box on different networks, with the
homelab as the seed.

## Mesh-Voice state (useful context)

`main` is pushed. Everything here works and is installed on Windows; the
Linux build works too.

- **Helpers** (each its own cargo project, excluded from the root workspace):
  - `mesh-wa`: native WhatsApp on whatsapp-rust.
  - `mesh-dc`: native Discord, with gateway, REST, password or QR sign-in,
    and voice through songbird with DAVE.
  - `mesh-media`: camera and screen capture with H.264. Windows uses Graphics
    Capture and Media Foundation; the Linux side is in too.
- **Helper protocol** (copy this for Mesh-Node):
  - stdin: `{"fn":..,"args":[..]}`
  - stdout: `{"binding":..,"payload":"<json string>"}`
  - Logs go to stderr.
  - Mesh uses a helper when the binary sits next to Mesh's own binary.
- **Windows extras:**
  - Inno Setup installer: `installer/build.ps1` produces
    `dist/Mesh-Setup-*.exe`.
  - App icon, a DWM-themed title bar and a tray icon (close hides the
    window).
- **Linux extras:**
  - `installer/install-linux.sh`.
  - A tray icon, and `discord_native_wrapper` to run `mesh-dc` inside the VPN
    network namespace.
  - `design/LINUX-HANDOFF.md`.
- **Lessons:**
  - A gateway bug that reconnected and logged in again every few seconds got
    a Discord token invalidated. Always back off reconnects.
  - Native Discord video and screen share were deliberately not built.

## Boundaries

- **Always:**
  - Encrypt, and never send telemetry.
  - Keep things lightweight, with dependencies justified in a comment.
  - Test between two real machines before calling a module done.
- **Ask first:**
  - Adding infrastructure (VPS, domains).
  - Anything public-facing, such as making a repo public or publishing a
    release.
- **Never:**
  - Central servers that hold user data.
  - Accounts or registration.
  - A public directory or search.
