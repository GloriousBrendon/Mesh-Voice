# Capability Map: Mesh (serverless Discord)

Status: **APPROVED 2026-10-22.** `identity` and `transport` are built (their Windows checkpoints pending); `seed` is built and deployed home-only (Tasks 6–7, 2026-11-10); `routing` is built (Checkpoint B by hand pending); `social` is built (Checkpoint B passed 2026-11-10); `e2ee` spec approved 2026-11-10 (Olm and Megolm through vodozemac), built (Checkpoint B passed 2026-11-10); `dm` spec drafted 2026-11-10 for review (`design/mesh/SPEC-dm.md`).

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

## Before beta: every check that needs a person

Everything below needs someone at the machines; what a test can check is
already a test. They can mostly share one sitting. Each points at where
its steps are.

1. **A solo look at the chat screens** (PLAN-dm Checkpoint B), on Linux
   and on Windows. The machine test (`cargo test -p mesh-app checkpoint
   -- --ignored`) drives steps 1 to 6 through the app's own state, but
   draws nothing. This look covers what it can't see: how the sidebar
   rows, marks, hover bar, picture preview and lightbox, typing line and
   group panel look and fit (the narrow Accounts panel too); that sounds
   play and desktop notifications show; that clicking `run.sh` really
   opens the file manager on its folder and a PDF really opens its viewer
   (the test checks the decision, not the launch); the composer by
   keyboard (Enter, Shift+Enter, Esc out of a reply or an edit); timing as a person
   feels it (a message in under 1 s, a waiting one within 5 s of
   reopening).
2. **Linking** (PLAN-link task 6; steps in PLAN-dm Checkpoint B, step 7):
   the Windows PC linked from the Linux box at home, with the 40 s wait
   before Link that checks be2d617; the friends and chats arrive; removing
   it keeps it out.
3. **Identity on Windows** (PLAN-identity Checkpoint D): create, export
   and import across the machines, the same safety number, wrong
   passwords, unlock times, Keep unlocked.
4. **Transport on Windows** (PLAN-transport Checkpoint C): ping both ways
   from different networks, forced relay, a network switch, a UDP-blocked
   network if there is one, no admin rights and the firewall prompt, idle
   numbers.
5. **Routing, two windows** (PLAN-routing Checkpoint B): add each other,
   ping by ID. In effect it passed inside social's and e2ee's Checkpoint B
   on Windows (2026-11-10) but was never ticked on its own; confirm or run
   it (a few minutes).
6. **The module gates, two machines on different networks**: seed,
   routing, social, e2ee and dm Checkpoint C (each plan's last
   checkpoint). They need a seed reachable from outside home: the VPS
   (finding 6). One sitting, after it's up. e2ee's check sends a Mesh chat
   message where it says a test note (retired 2026-11-12).
7. **Mail through the home seed, from your own PC** (added 2026-11-13). *Covered by PLAN-packaging Task 5 (2026-11-14): it is done in that sitting, steps 6 to 8, and ticked there.*
   The home seed holds mail since Task 18 of PLAN-mailbox; this checks
   it with a real install. On the home network or Tailscale (the seed is
   home-only):
   1. **Get the new engine.** Quit Mesh. Pull both repos (Mesh-Node at
      `cc482e6` or later, beside Mesh-Voice), then on Linux run
      `installer/install-linux.sh`; on Windows run `installer\build.ps1`
      and then the `dist\Mesh-Setup-<version>.exe` it made. Either way
      `mesh-node` lands next to Mesh, which is where Mesh looks for it.
   2. **Two people on one PC.** Start Mesh (A, your own). Start a second
      copy with its own folder (B): `MESH_DATA=/tmp/meshB mesh` on Linux,
      `$env:MESH_DATA='C:\meshB'; & "$env:LOCALAPPDATA\Programs\Mesh\Mesh.exe"`
      in PowerShell. Create an identity in B and add each other with a
      friend code (Your ID → Friend codes). Leave both open a minute: each
      puts its mail record at the seed when it unlocks.
   3. **Mail.** Quit B (there's no Lock button yet; quitting takes B
      offline just the same). In A, send B a line. Its mark stays at
      "sending".
   4. **Back.** Start B again and unlock it. The line shows in B within a
      few seconds, and A's "sending" mark clears.
   5. **The counts.** Each hourly line covers the hour before it, so read
      the one or two lines after you finished (`docker compose logs seed
      | grep "last hour mail"` in the seed's deploy folder). The seed
      counts requests. Claims and collects are padded with dummies to 64
      boxes a week (up to 64 friends; M13), so what one copy really sends
      is far above the real friends. Against the all-zero lines before
      (*corrected 2026-11-14: the first version gave only the unpadded
      minimums*):

      | Count | Minimum | What one hour of this check really shows |
      |---|---|---|
      | claims | 12 | **about 780**: 384 (64 × 6 weeks) at each copy's first unlock on the new engine, since its daily renewal is due, plus 6 a side for the new friendship (unpadded). B's second unlock adds none: it renewed under a day ago. |
      | mail record puts | 2 | each side's record for the other, again at every unlock (not padded) |
      | mail record gets | 2 | each unlock reads its friends' records (not padded) |
      | puts | 1 | A's line, exactly 1 |
      | collects | 384 | **about 1,150 or more**: 384 (64 × 6 weeks) at each of the three unlocks, plus 128 (64 × 2 weeks) every 30 minutes for each open copy. An unlock before the network is up can add one more 384 when the seeds answer. |
      | takes | 1 | B taking the line, exactly 1 |
      | envelopes held | 0 at the end | 0 |

      If puts moved but takes didn't, the line is still at the seed. If
      claims or mail record puts stay at 0, the install still runs the
      old engine (step 1).

e2ee Checkpoint B's step 4 (a window restored from a backup shows the
notes) was skipped by hand; the dm process test now checks that a device
restored from a backup gets the whole history, so it isn't listed.

## Open questions

- **Move to a new ID?** Settle before `spaces`. Every device holds the root
  secret (identity D2), so a removed device that's still unlocked can sign
  itself a new device key and get back in. The only real fix is a signed
  "I've moved to this new ID" message that friends follow (and Spaces, once
  they exist). Each module spec has its own questions too.
- **Before `spaces` reuses the dm store, it must change how the history
  log is held.** dm reads the device-wide log whole at unlock and keeps it
  in memory: 168 MB and 1.7 s at 1,000,000 events, 16 MB and 170 ms at
  100,000 (PLAN-dm Task 15). Accepted for dm (2026-11-12); a Space's
  history grows far past one person's chats, so `spaces` must not inherit
  it as it is.
