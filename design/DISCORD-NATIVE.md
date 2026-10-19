# Discord inside Mesh: research and plan (2026-10-19)

Goal: every Discord feature (text, voice, video, servers, friends) from Mesh's
own UI, never seeing a Discord window. Later: no Discord code on the PC at all.
The owner accepts the ban risk.

## Facts that decide the design

- **Voice requires DAVE.** Discord made its end-to-end voice/video encryption
  (DAVE, built on MLS) mandatory on 2026-03-01/02; clients without it can't join
  calls, and there's no unencrypted fallback. A custom voice client must
  implement it.
  [Discord](https://discord.com/blog/bringing-dave-to-all-discord-platforms),
  [support](https://support.discord.com/hc/en-us/articles/38025123604631-Minimum-Client-Version-Requirements-for-Voice-Chat),
  [Help Net Security](https://www.helpnetsecurity.com/2026/05/19/discord-voice-and-video-call-encryption/)
- **DAVE in Rust exists:** `davey` (Snazzah, OpenMLS) is what discord.js voice
  uses; immature API, few docs. (The `dave` crate on docs.rs is a different,
  narrower project.) [davey](https://rust-digger.code-maven.com/crates/davey)
- **A native third-party client with voice exists:** Abaddon (C++/GTK) uses
  Discord's own `libdave` + mlspp. Proof it's doable, and a reference.
  [Abaddon](https://github.com/uowuo/abaddon)
- **The official web client exposes everything internally.** Its webpack module
  registry (`webpackChunkdiscord_app`) holds the Flux dispatcher and stores
  (UserStore, ChannelStore, VoiceStateStore, …) that Vencord-style mods read and
  drive. [Vencord stores](https://gitdab.com/profxadke/Vencord/src/commit/a0a1a4d139b6408419cd564611c26bd0dbade145/src/webpack/common.tsx)
- **From the earlier `rea` pass on the client:** every request carries
  `X-Super-Properties` / `X-Fingerprint`; the client reports `has_client_mods`
  by checking for known mods (Vencord, BetterDiscord, …) by name; voice is
  gateway → voice websocket (`max_dave_protocol_version`) → encrypted Opus over
  UDP → DAVE, and it refuses downgrade (`DAVE_DOWNGRADE_REFUSED`).

## Verified on this machine (2026-10-19)

A process **outside** the discord-voice network namespace controlled a browser
**inside** it over the Chrome DevTools Protocol, through a unix socket on the
shared filesystem (a 30-line relay in the namespace). It loaded discord.com
through the VPN and reached the webpack registry: 717 chunks, 6858 modules.
Store discovery on the logged-out page found only `UserStore`; the rest load
after login (not yet checked against a logged-in session).

## Options

| | A. Mesh skin over hidden official web client | B. Native Rust client in Mesh |
|---|---|---|
| How | discord-voice browser runs hidden; Mesh drives it via CDP + an injected script that reads Flux stores and dispatches actions | Gateway + REST + voice websocket + UDP/Opus + DAVE (davey), in a new `mesh-discord` crate |
| Features | All of them, including video/screen share, because it *is* Discord | Whatever we build; voice/DAVE is the big part |
| DAVE, captchas, new features | Discord's own code handles them | Ours to implement and keep up with |
| Ban risk | Lowest: official client; our script isn't one of the mods it checks for | Highest: different fingerprint |
| Breakage | Store/module names change sometimes (Vencord copes; `rea` bundle diffs can catch it) | Protocol changes break voice until fixed |
| Work | Medium | Large (many sessions) |
| Discord code on the PC | Yes (in the browser) | No |

**Route to "no Discord on the PC":** run option A's browser on the homelab
inside the VPN container instead, and send audio to the PC over the network
(PipeWire `module-pulse-tunnel` / Pulse TCP). *Unverified:* WebRTC mic capture
from a containerised Chromium over a network Pulse source.

## Recommendation

1. **Build A on the PC first.** Biggest feature win, lowest risk, uses what's
   running today. Keep the mautrix-discord bridge for text history in Matrix
   until A covers text too.
2. **Then move A's browser to the homelab**, once the network-audio path is
   proven with a test recording.
3. **B only if Discord starts blocking A.** `davey` + Abaddon are the references.

Use `rea` throughout: diff each new Discord web build's bundles for renamed
stores, actions and voice opcodes, so A (and B) break loudly in a test, not
silently in a call.

## A: first steps

1. `discord-voice --engine`: launch the browser hidden (headed but in the
   scratchpad first; headless only if Discord doesn't flag it) with
   `--remote-debugging-port` and the unix-socket relay.
2. Spike: from Mesh, log store contents for a logged-in session: guilds,
   channels, voice states; join a voice channel and toggle mute via dispatched
   actions. Done when Mesh can join/leave a voice channel and mute with the
   window never shown.
3. Then UI: voice channels under VOICE in the channel pane, who's speaking,
   mute/deafen wired to the existing user panel.

Keep `mesh-core` Matrix-only (design rule): Discord-engine code goes in its
own crate.
