# WhatsApp inside Mesh: how WhatsApp Web works, and the plan (2026-10-20)

Goal: WhatsApp chats, voice and video calls in Mesh's own UI, the same way
Discord works today (DISCORD-NATIVE.md, option A): the official WhatsApp Web
client runs hidden in the engine browser and an injected agent reads its
state and calls its own actions. No bridge, no protocol reimplementation.

## Why not a bridge or a native client

- **Bridge (mautrix-whatsapp, built on whatsmeow):** an unofficial protocol
  client, against WhatsApp's terms; the linked number can be banned. It
  doesn't carry calls at all.
- **Native client in Rust:** Noise XX handshake over a WebSocket, Signal
  protocol with multi-device keys, a binary stanza format, app-state sync,
  and a proprietary WASM call stack (below). Months of work, a one-of-a-kind
  client fingerprint, and the highest ban risk.
- **Hidden official client:** WhatsApp sees WhatsApp Web in a real browser.
  WhatsApp Web has had voice and video calls (1:1 and group) since July 2026.

## Findings (REA page capture + live module registry, logged in)

**Module system.** Meta's Haste modules (`__d` / `require`), served from
`static.whatsapp.net` (about 21 scripts, 30 MB before login). 16,023
modules, 9,742 named `WAWeb…`, and `require("__debug").modulesMap` lists
them all. Modules are required **by name**, so an agent is simpler and more
stable than Discord's (minified webpack ids found by shape).

**Storage** (REA `inspect-web-page`): IndexedDB `signal-storage` (Signal
keys), `model-storage`, `wawc`, `wawc_db_enc` (encrypted local message
store); localStorage `WANoiseInfo` (Noise session).

**Chats and messages.**
- `WAWebChatCollection.ChatCollection`: every chat (`getModelsArray()`); a
  chat has `id` (a `…@lid` / `…@c.us` / `…@g.us` wid), `formattedTitle`,
  `unreadCount`, `t` (last activity, seconds), `archive`, `pin`,
  `muteExpiration`, and `msgs` (its loaded messages, a collection).
- `WAWebMsgCollection.MsgCollection`: all loaded messages. A message has
  `id`, `type` (`chat`, `image`, `ptt`, `video`, `document`, …), `body`,
  `t`, `from`, `to`, `author` (groups), `ack` (0–3, ticks), `isNewMsg`,
  `quotedMsg`, and media fields (`mediaKey`, `directPath`, `mimetype`,
  `size`, `width`, `height`, `mediaData`).
- Collections are Backbone-style: `on` / `off` / `once` / `trigger`,
  `get`, `find`, `filter`, `where`, `forEach`. Live updates come from
  `MsgCollection.on("add" | "change", …)`.

**Sending.** `WAWebSendTextMsgChatAction`: `sendTextMsgToChat`,
`addAndSendTextMsg`, `createTextMsgData`. `WAWebSendMsgChatAction`:
`addAndSendMsgToChat`. Signatures still to confirm (test on the "Message
yourself" chat).

**Media.** `WAWebDownloadManager`, `WAWebMediaDownloadMsg` (download and
decrypt); messages carry `mediaData` and thumbnails. Profile pictures:
`WAWebProfilePicThumbCollection` (an `eurl` per contact).

**Calls.** `await require("WAWebVoipStackInterface").getVoipStackInterface()`
(type `web`) has `startCall`, `startGroupCall`, `acceptCall`, `rejectCall`,
`endCall`, `joinOngoingCall`, `setCallMute`, `setCallVideoMute`,
`startScreenShare` / `stopScreenShare`, `getCallInfo`, `inviteToCall`, and
`callbacks` (`onVoipReady`, `onCallEvent`, capture hooks). UI-level starters
live in `WAWebVoipStartCall` (`startWAWebVoipCall`, group variants,
`joinOngoingCallByCallId`). State: `WAWebCallCollection` (`lastActiveCall`,
`pendingOutgoingCall`, `isInConnectedCall`, `on`/`off`) and
`WAWebVoipOngoingCallCollection`.

**The call stack is not WebRTC.** No WebRTC modules at all. Calls run on
WhatsApp's own VoIP engine compiled to WASM (`WAWebVoipWasm…`), connected
to WhatsApp relays over WebTransport with an SCTP data-channel fallback
(`WAWebVoipWebTransport…`, `WAWebVoipSctpDataChannel…`). Audio goes
through AudioWorklets (`WAWebVoipAudioCaptureWorklet`,
`…PlaybackWorklet`); video through a worker renderer
(`WAWebVoipVideoRendererRegistry`, `…RendererWorkerResource`,
`WAWebVoipVideoFrameCtor`, WebGPU capture). Consequences:
- Voice works from the hidden page as is: its audio plays on the PC's
  output and the mic is granted by `--use-fake-ui-for-media-stream`.
- Video in Mesh hooks WhatsApp's renderer: every frame passes through
  `videoRendererRegistry.onVideoFrameWasmToJs(source, …)`; for each new
  source the agent registers one more (off-screen) canvas with
  `registerVideoCanvas` + `assignSourceToCanvas({canvas, mirror, source})`
  and sends Mesh a JPEG of it at ~15 fps.
- Call controls, as WhatsApp's call screen uses them: `acceptCall(withMic,
  withCamera, entryPoint)` (after `WAWebCallRingtone.stopCallRingtone()` and
  `WAWebVoipCallAudioUnlock.prepareCallAudioElement()`), `rejectCall()`,
  `endCall(EndCallReason.Self, true)`, `setCallMute(bool)`,
  `setCallVideoMute(bool)`; placing one:
  `WAWebVoipStartCall.startWAWebVoipCall(wid, isVideo, 0, 0, null,
  {entryTrust: "user_gesture"})`. State: `WAWebCallCollection.activeCall`
  (`getState()` read with `WAWebVoipCallStateUtils`, `isVideo`,
  `selfMicMuted`, `selfVideoState`).

**REA notes.** `inspect-web-page` worked. `analyze-web-bundle` runs out of
memory on the 30 MB bundle even with a 12 GB heap (and once raced a
collected script id); worth reporting upstream. The module registry made
it unnecessary.

## Plan

1. **Generalise the engine.** `mesh-discord`'s CDP transport (socket or
   host:port, page lookup, bindings, reconnect) is platform-neutral; let it
   attach one agent per page (match by URL), so Discord and WhatsApp share
   one engine browser.
2. **WhatsApp agent** (`whatsapp-agent.js`): snapshot of chats (id, name,
   unread, last activity, picture); `open(chat)` streaming its messages;
   `send(chat, text)`; media and profile pictures through the page;
   notifications from `MsgCollection` `add` (`isNewMsg`, not ours); calls
   through the VoIP stack interface, with `callbacks.onCallEvent` wrapped
   (keeping WhatsApp's own handler) to stream call state.
3. **Mesh UI:** a WhatsApp network (tag, Home chip, rail folder); chats on
   Home; the existing chat pane, notifications and call bar.
4. **Video:** frames from WhatsApp's renderer into Mesh (research first).

Open questions: exact `sendTextMsgToChat` / `startCall` signatures (confirm
on the "Message yourself" chat and a test call), and whether WhatsApp flags
an agent-driven client (none seen so far; the agent only uses WhatsApp
Web's own functions).
