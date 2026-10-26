# Capability Map: Mesh (serverless Discord)

Status: **APPROVED 2026-10-22.** `identity` and `transport` are built (their Windows checkpoints pending); next: the `seed` spec (`design/mesh/SPEC-seed.md`, draft).

**What Mesh is:** Discord without servers. Every install is a node. There is
no account to register: your identity is a keypair, and your ID is your public
key. There is no search. Someone can reach you only if you gave them your ID,
and you can block anyone. Friends relay traffic and hold messages for each
other.

**Where things live:**
- **Mesh-Node** (new repo): all of the networking. It is one Rust binary that
  runs in two ways:
  - as a helper process that Mesh-Voice drives over stdio, the same pattern as
    `mesh-wa` and `mesh-dc`;
  - headless, as an always-on seed node (homelab, VPS).
- **Mesh-Voice** (this repo): the app people download. All planning and
  documentation for Mesh lives under `design/mesh/`.
- **GloriousBrendon/Mesh** is reference only. Its docs carry over where they
  still hold; its WireGuard, Wintun, OpenDHT, mediasoup and Tauri stack does
  not.

## Modules

| Module id | Responsibility | Depends on | Carries over from the original docs |
|---|---|---|---|
| `identity` | Ed25519 keypair; the ID format (`mesh1…`, shown as text, QR code and link); keystore encrypted with the user's password; backup and restore | — | IDENTITY_AND_KEY_MANAGEMENT §1–4 |
| `transport` | QUIC connections dialled by public key (iroh): hole punching, relay fallback, reconnects; no admin rights, no TUN device | identity | replaces TUNNEL_MANAGEMENT and the WireGuard ADR |
| `seed` | Headless node mode: always on, runs our own relay, does rendezvous for IDs, holds mail; deployed with Docker or systemd. These are ordinary nodes that can be phased out later | transport | SYSTEM_ARCHITECTURE "stateless infrastructure" |
| `routing` | Reaching someone by ID: first friends, then a blinded friend-of-friend lookup a few hops deep, then seed rendezvous as a fallback; sharing your current address with friends | transport, seed | new |
| `e2ee` | End-to-end encryption for 1:1 chats, groups and Spaces, with forward secrecy (MLS, or Signal-style ratchets); sealed sender for relayed mail | identity | IDENTITY §1, SECURITY_THREAT_MODEL |
| `mailbox` | Store-and-forward: friends and seeds hold encrypted mail for someone offline, with an expiry and quotas, and deliver it when they come back | routing, e2ee | CONTENT_STORAGE §7, ABUSE §3 |
| `social` | Friend requests (send by ID, accept, decline), friends list, blocking, presence (online, idle, DND, invisible) | routing (works between online peers first; gains offline delivery when `mailbox` lands) | PRESENCE_SYSTEM, ABUSE §5 |
| `dm` | 1:1 and group DMs: send, edit, delete, reply, react, typing, read state, attachments, history sync between devices | social, e2ee, mailbox | CONTENT_STORAGE §4–6 |
| `spaces` | Servers: channels and categories, roles and permissions, invites, signed append-only logs replicated among members, moderation (kick, ban, delete), attachments | e2ee, mailbox, routing | SPACE_ARCHITECTURE §1–5 |
| `calls` | Voice, video and screen share in DMs and voice channels: a direct link 1:1, a relay tree of the nearest members in groups, SFrame E2EE so relays carry only ciphertext. Reuses `mesh-media` and Opus | social, spaces, transport | VOICE_VIDEO_ARCHITECTURE §3–7 (without mediasoup) |
| `client` | Mesh as a native network in Mesh-Voice: the helper protocol, onboarding (create an ID, set a password), add friend, the friends page, Spaces UI, and the Discord-style screens Mesh-Voice already has | each of the above, in step | FRONTEND_BACKEND_NOTES |
| `packaging` | Windows setup and Linux installer bundle `mesh-node`; a seed image; release CI producing signed artifacts | client, seed | ONBOARDING_GUIDE |

## Build order

**identity → transport → seed → routing → social (+ client) → e2ee → dm →
mailbox → spaces → calls → packaging**

Each module ends with a check you can run between two real machines, for
example your PC and the Linux box on different networks with the homelab as
the seed. `client` is not a separate phase: every module ships with its
Mesh-Voice screens.

The first usable milestone falls at the end of `dm`: add a friend by ID and
chat while both of you are online. `mailbox` adds offline delivery.

## Where this can't be exactly like Discord

- **No search or discovery.** You reach people only by an ID they gave you,
  which is deliberate.
- **History comes from other members.** A message posted while every member is
  offline waits on the sender's node, a friend or a seed until someone comes
  online. A new Space member receives history from whoever is online.
- **Delete is a request to other devices.** It is honoured by Mesh clients,
  but can't be enforced against a modified one.
- **Large Spaces are a later goal.** Version 1 targets Spaces of up to a few
  hundred members. Groups of 50,000+ (SPACE_ARCHITECTURE §6) stay future work.
- **Big group video calls are limited by home upload speed.** Voice scales
  fine. Video works for small groups.

## Decisions (2026-10-22)

- **Mesh-Node is the engine**: a helper process for Mesh-Voice, and the
  headless seed. Its repo is `GloriousBrendon/Mesh-Node`, **private**.
- **Desktop only (Windows and Linux) for version 1.** Phones come later as
  light clients.
- **Transport: use whatever is lightweight and works, even our own.** We start
  with iroh: you dial a public key, and it finds the current address itself
  through the relays and published address records. That covers changing home
  IPs, so nobody needs a static IP or port forwarding. Writing our own is the
  fallback if iroh doesn't hold up.
- **Several devices per person, by choice.** One identity, several linked
  devices, in version 1.
- **The homelab is the first seed, hardened.** Only the QUIC/relay port is
  open to the internet. Admin is over Tailscale only. It runs in a container
  that holds nothing readable: ciphertext only, with quotas, expiry and rate
  limits. The `seed` spec covers this.
- **IDs: a long permanent ID plus short friend codes.**
  - The permanent ID is the public key. It travels in QR codes, `mesh://`
    links and copy and paste.
  - Friend codes are what people share by hand, like `K7QM-3XPA`. They work
    like Discord invites: they expire (24 hours by default), can be single-use
    and can be revoked, and lookups are rate-limited.
  - After a request, both people can compare a safety number.
- **Blocking is per account, kept on your devices.** You keep the ID of anyone
  you've blocked, and the list syncs to your other devices. ID rotation is not
  in version 1.
- **Linking a device needs one of your existing devices online.** There is no
  server copy: the new device syncs from your existing one.
- **A second seed on a VPS comes after testing**, once the homelab seed is
  shown to work.

## Open questions

None at the map level. Each module spec has its own.
