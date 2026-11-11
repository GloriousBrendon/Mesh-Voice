# Spec: `dm` (Mesh)

Status: **APPROVED 2026-11-11.** Q1–Q4 are decided (as recommended). Plan:
[PLAN-dm.md](PLAN-dm.md); no code until the plan is approved.
*Revised 2026-11-11 with the audit's review (A–G): D2, D3, D5, D7, D8, D10,
D11 and D12 changed. Review C's keys-only record moved to a note for
`mailbox`; dm doesn't need it.* Module 7 of the build order in
[CAPABILITY-MAP.md](CAPABILITY-MAP.md). Depends on
[`social`](SPEC-social.md) (built; Checkpoint B passed) and
[`e2ee`](SPEC-e2ee.md) (built; Checkpoint B passed). The map also lists
`mailbox` as a dependency. That's built after `dm`, so here a message waits
on the sender's devices (see *What `mailbox` changes*). `spaces` reuses this
module's message format, store and attachments.

## Objective

Chat with friends in Mesh the way you do in Discord DMs: 1:1 and small
groups, with replies, edits, deletes, reactions, typing, unread counts,
pictures and files. History is on every one of your devices, including one
you link later. This is the map's first usable milestone: add a friend and
chat while you're both online.

Today two friends can only send each other test notes. They live in memory,
are gone at lock, and a note you send doesn't show on the device that sent
it (e2ee Task 10). After this module, each device keeps an encrypted chat
history, Mesh-Voice shows Mesh DMs in the same chat view as WhatsApp and
Discord, and test notes are retired.

**What this module delivers**

1. **1:1 DMs** with any friend, using e2ee's per-device sessions (e2ee D5).
2. **Group DMs** of up to 10 people (Discord's cap), using sender chains
   (e2ee D6), with add, remove, leave and rename.
3. **Messages you can edit, delete, reply to and react to.** Typing shows
   while it's live. Unread counts and "read up to here" sync between your
   own devices.
4. **Attachments**: pictures shown inline with a preview that travels in
   the message, and other files opened on click.
5. **History on every device you own.** Your devices fill each other's gaps
   whenever two of them connect. A new device gets the whole history (Q3).
6. **A store** that keeps history encrypted under the keystore's data key.
   Opening a chat shows its newest page at once, however long it is.
7. **Mesh-Voice**: Mesh DMs in the sidebar, the existing chat view and
   composer, notifications and sounds. A friend's row on Home opens the DM.

**Not in v1 of this module**

- Holding mail while *both* of you are offline. That's `mailbox`.
- Voice and video calls (`calls`), and Spaces (`spaces`).
- Message search, pins, threads, stickers and embeds/link previews.
  Previews would fetch URLs, which leaks who reads what. They'd need their
  own decision.
- Read receipts shown to the other person (D9).
- Delete for everyone on clients that ignore it (map: "delete is a
  request").

**User stories**

- I message a friend who's online. It shows on their desktop and laptop in
  under a second, and on my own laptop too.
- My friend is offline. I send three messages and close Mesh. Later we're
  both online at the same time, and they arrive in order, marked as sent
  while they were away.
- I fix a typo. Everyone sees the edit, marked "(edited)". I delete a
  message and it disappears on every Mesh client.
- I start a group DM with two friends, name it, and send a photo. Its
  preview shows at once. The full picture loads when someone opens it.
- I link a new laptop. A minute later it has all my chats.
- I read a chat on my desktop. My laptop's unread badge for it clears.

### Decisions this spec makes

| # | Decision | Why |
|---|---|---|
| D1 | **One event format for everything in a chat.** A `dm` event is the body of an e2ee `Kind::Message` (e2ee D2). It has a version, a 16-byte random event ID, the sender's time, and one of: `Text`, `Edit`, `Delete`, `React`, `Read`, `Group` (a group change). It's encoded with `postcard`, which the keystore already uses. | The envelope already authenticates the sender, device and conversation (e2ee D10, D11), so the event doesn't need to repeat them or carry its own signature. One kind of thing to store, sync and replay. `spaces` can reuse it. |
| D2 | **Ordering without a server.** Each event carries two counters. `seq` is per chat and **never runs ahead of the clock**, counted in milliseconds since 1970 (Unix time). A sender uses one more than the highest `seq` it holds in that chat **that isn't ahead of its own clock**, or than its own device's last `seq` there if that's higher: a device never numbers at or below its own last event, even when its clock steps back or it sends twice in a millisecond. `n` is per sending device: its own events in that chat, 1, 2, 3. A chat is shown in order of (`seq`, event ID). The sender's time is for display only; a time more than 5 minutes after it was received shows as the received time. **A reader checks each event against its own clock and the sender's earlier events, never against the highest `seq` it holds:** (1) **Ahead of the clock:** an event with a `seq` more than 5 minutes ahead of the reader's clock waits, stored but not shown, until it's no longer more than 5 minutes ahead; one more than a day ahead is dropped. At most 1,000 events wait per sending device; past that, new ones are dropped. (2) **Out of step:** for one sending device, a higher `n` must have a higher `seq`. An event that breaks that against one already held is refused. Honest clients never break it, whatever order events arrive in. (3) **Late and low:** an event that arrives with a `seq` at or below your read marker (D9) still counts as unread and is marked *arrived late*, so it can't hide in old history. Refused and dropped events are acknowledged `ok` (so they aren't sent again) and counted (D14); waiting ones are acknowledged once stored. **Events go out in the sending device's stored order** within a chat, live (D5) and in own-device sync (D8; chats may go newest first, but each chat's events go oldest first). Edits, deletes and reactions point at their target's event ID. One that arrives before its target is kept and applied when the target comes. *Changed 2026-11-11 (your review): any bound measured against the highest `seq` a reader holds let one member cut another out, by sending big jumps to everyone else; the clock is the one bound nobody else controls.* | Nobody holds the one true order (the map). A counter that only moves forward puts a message after everything its sender had seen, whatever their clock says. Anchoring the limit to the reader's own clock means nothing anyone sends, or withholds, can push a reader out of range, and a new device or member needs no baseline. A forger can move only their own messages, and at most 5 minutes later than they'd otherwise go: honest senders don't take up a `seq` ahead of their clock, so a jump doesn't carry into their events until their clocks pass it. The cost falls on wrong clocks: a reader more than 5 minutes behind holds events that are ahead of it (Mesh-Voice says the clock looks wrong), and a sender behind the others sorts its messages a little early, where rule 3 marks them late. |
| D3 | **Only the author edits or deletes.** An edit replaces the text and marks it "(edited)". The newest edit by sender time wins. A delete removes the text and attachments from the store on every device that gets it, **and every `Edit` that points at it**, in whichever segment each is, leaving a tombstone so a late copy (of the message or an edit) can't bring it back. **Delete for me** removes it on your own devices only. | It's what Discord does in DMs. A tombstone is what makes delete stick when copies arrive late and out of order. Edits hold the new text, often in a later segment, so scrubbing only the original would leave the edited text on disk (review E). |
| D4 | **Reactions** are `React { target, emoji, on }` per person. The newest one by sender time for each (target, person, emoji) wins. Up to 20 different emoji per message. | Each person controls their own reactions, so there's nothing to merge. |
| D5 | **Delivery and own-device sync are one mechanism: events wait, not envelopes.** Every event is in the sealed store (D11) anyway. Each device numbers the events it stores, 1, 2, 3, and so on. For every device it sends to (each friend's device, and your own other devices), it remembers a counter: the number up to which that device has acknowledged everything. When a connection to that device opens, it encrypts what's after the counter on the current session (or chain) and sends it over the `e2ee` service. To a friend's device it sends only your own events in chats with them, for up to 30 days and 2,000 events; past that they're marked as not delivered to that device. To your own devices it sends everything (D8). A friend's device that's new to you starts at the lowest counter of that friend's other devices, so a reinstall or a new device misses nothing. **Acknowledged** means the reader has opened *and stored* the event: it answers each envelope `ok` or `failed`. The counter moves past an event once it and everything before it are `ok`. Anything not `ok` is encrypted again and sent again with the same event ID (event IDs remove duplicates). **When a fresh session arrives from a device** (e2ee D4: the reader couldn't open a first message, and that first message is dropped), the sender at once sends everything that device hasn't acknowledged on the new session. A message shows **sending** until at least one of the other person's devices has acknowledged it `ok`, then **sent**. *Changed 2026-11-11 (review A and B).* | An envelope is tied to one device and one session. While it waits, the reader may add a device, reinstall or rotate keys, and then a stored envelope is useless or can't be made at all. Encrypting when you send avoids that, and one counter then covers delivery, retry and own-device catch-up. "Sent" is honest only if the ack means stored. Friends' devices already dial each other at unlock and at every refresh (social D12), so there's no new dialling. |
| D6 | **Group DMs need everyone to be friends with everyone (Q1).** Creating or adding someone checks that you're friends with them. A member who isn't friends with every other member is refused, and the error names who isn't. Up to 10 members. | Devices take connections only from contacts (routing D9), and members find each other's keys only through contacts' records (e2ee *What later modules get*). Passing messages on for non-friends needs sealed relaying through members, which `spaces` has to build anyway. Building it once there is cheaper than twice. |
| D7 | **Group changes are events** in the group's own chat: add, remove, leave, rename. *2026-11-11: the group's ID is `blake3::derive_key("mesh/v1/dm-group", creator ID ‖ random value)`, and the random value travels in the creation event. A creation counts only if it hashes to the group's ID from its sender, so no member can send a second creation and replace the creator. Since group DMs go over the 1:1 sessions (e2ee D6), dm checks membership: a group event counts only if its sender is a member in the group's current version. Events for a group whose creation hasn't arrived, and events from someone not yet a member, are answered `failed` and not stored: the sender keeps them and sends them again soon after, then on each refresh, until they're taken (*2026-11-11, latest: replaced waiting here, with its cap and expiry*). Each sender sends to the members of the version it holds.* *2026-11-11, later: **each group change is signed with its author's ID key** (over the group, the event's ID, `seq` and `n`, the author's device and the change), and **any member passes the changes on** to members who may lack them, so a new member can join, and everyone ends with the same group, while an earlier change's author is away for good. A change whose signature isn't its claimed author's is refused. A member gets the group's changes whenever they joined, its messages from their add on, and a change reaches everyone in the version before or after it.* Each names the change it follows (`after`; none for the creation), so they form a chain. A change is accepted only if it follows the newest change you hold and its sender is a member in that version, with the right to make it. Anyone can add their friends, leave or rename. Only the group's creator can remove someone. A change whose `after` you don't have yet waits for it. Two changes that follow the same one are a fork. **The branch that contains a change by the creator anywhere wins.** If both do, the branch whose first creator change has the lower (`seq`, event ID) wins: the creator's earlier decision stands. If neither does, the branch whose first change has the lower event ID wins. A fork of more than two branches picks the same way. The choice is made again as changes arrive, so a branch that wins for now can lose once a creator change reaches the other; the creator's devices follow the winner, so it settles. The other branch's changes are dropped, and their senders see them undone. An accepted change calls e2ee's `set_members`, which rotates the chain on the next send (e2ee D7). *Changed 2026-11-11 (review F).* | With "a member at that point" judged by sender time, a removed member with a changed client could back-date an add to just before their own removal. A chain judges membership by which version a change follows, which a sender can't fake. A removed member's change can only fork from before the removal, and the removal is the creator's, so it wins. Every member ends with the same list without anyone in charge. |
| D8 | **Own-device sync is D5 aimed at your own devices.** When two of your devices connect, each sends the other everything after that device's counter, received events included, in batches of 200, newest chat first and each chat's events in stored order (D2). A device that's new to you starts from 0 and gets everything (Q3). A batch is opened with e2ee's batch call (D11). | One mechanism covers a new device's history (e2ee D13: history is re-sent as messages, never as old keys), a laptop that missed a week, and edits made elsewhere. The counter is per device, so nothing has to agree on a global order. |
| D9 | **Read state is yours alone.** "Read up to event X" is a `Read` event sent only to your own devices. The other person never sees it, and typing is the only live signal they get. Unread counts and mentions come from it. | Discord doesn't show read receipts in DMs. It's less data about you on someone else's device. |
| D10 | **Typing goes over the live connection, not through e2ee or any store.** While you type, the helper sends `typing(conversation)` every 5 s on a new `typing` service (a stream of its own) to each member's connected devices. A reader shows it for 7 s. Never stored, never queued. The `presence` stream's frames don't change. *Changed 2026-11-11 (review G).* | The connection is already encrypted between device keys (transport D5) and goes to friends only. Running typing through the ratchet would advance it on every keystroke and save state to disk each time, for something that's thrown away. |
| D11 | **The store.** Each chat is a folder of segment files in `<data>/dm/`. A segment holds up to 512 events, each sealed with XChaCha20-Poly1305 under `blake3::derive_key("mesh/v1/dm-store", data key)`, with the segment's name and the event's position as associated data. New events are appended. **An appended event is synced to disk before it's acknowledged (D5); a batch of appends is synced once.** Received envelopes are opened with a new batch call in `mesh-e2ee`: open many, save the session state once, then hand them all up ("save before handing up" holds per batch). *Changed 2026-11-11 (review D): this changes e2ee's API, not its format. Without it every decrypt rewrites and syncs the per-person state, and 10,000 events is 20,000 disk syncs.* An edit, delete or reaction is an event too, folded in when the segment is read. Opening a chat reads only the newest segment. Scrolling back reads older ones. An index file per chat holds its name, members, last activity and unread count, **its read marker, and each sending device's highest `n` and its `seq`**, so a chat opened from its newest segment alone still checks D2. **Each stored record is `received time | sender person | sender device | event`**, so a reopened chat shows the same times and knows who sent each event. **A device-wide, append-only log** numbers every stored event (D5's numbers) and records each deleted event ID; it's read once at unlock, and refuses a late copy of any old message and a late edit of a deleted one. **The counters** are one small sealed file, rewritten per batch of acks. Storing an event only appends. *Wording added 2026-11-11 (PLAN-dm Saving and loading).* Folder and file names are a keyed hash of the conversation ID. | No new dependency. SQLite would need SQLCipher, which brings OpenSSL, to encrypt. Appending a sealed record is crash-safe without rewriting files. A delete rewrites, atomically, each segment that holds the message or one of its edits. At about 300 B an event, 100,000 messages is about 30 MB. |
| D12 | **Attachments** use e2ee's file scheme: a random 32-byte key per file, XChaCha20-Poly1305 in 64 KiB chunks with the chunk number as associated data. The message carries the key, the encrypted file's blake3 hash, its size, name and type, and for pictures a preview (a JPEG up to 16 KiB, made by Mesh-Voice). The file itself moves on a new `blob` service: a reader asks any connected device in the chat (the sender's, or your own) for chunks by hash. **Each chunk is checked by its own encryption tag as it arrives, the whole file against the blake3 hash at the end, and the size in the message stops a cut-off file.** *Changed 2026-11-11 (review G): a single hash can't check one chunk.* Up to 100 MB per file (Q2). | The preview means a picture shows even when nobody has the full file online. Asking any member's device means a file stays fetchable after its sender goes offline, once one other person has it. |
| D13 | **Keeping files.** A sent file is copied, still encrypted, into `<data>/blobs/`, and so is every file a device downloads. A device keeps up to 10 GB of blobs and drops the oldest-opened first. Files you sent are dropped last. A delete (D3) drops the blob at once. | Somebody has to keep files available for peers. Without a cap that's a disk that fills without telling you. |
| D14 | **Limits**: text up to 4,000 characters (Discord Nitro's limit; well under the 64 KiB envelope), up to 10 attachments a message, a reply quotes one message. A malformed, oversized or unknown event is dropped and counted, never shown. | Named constants. A bad event from a modified client must not break the chat for anyone else. |
| D15 | **Nothing identifying is logged**: no IDs, conversation IDs, names or contents, only counts. Errors are fixed text. | Same rule as routing D10, social D15 and e2ee D15. |

### Options compared: the history store (D11)

| Option | New crates | Encrypted at rest | Notes |
|---|---|---|---|
| **Sealed segment files** (D11) | 0 | Yes, under the data key, the way e2ee's store does it | Only the newest 512 events are read to open a chat. We write the folding of edits and reactions ourselves (small) |
| SQLite through `rusqlite` with `bundled-sqlcipher-vendored-openssl` | rusqlite, libsqlite3-sys, openssl-src and others; a C build on both platforms | Yes | Queries and indexes for free. Adds OpenSSL to a project that has none, plus several MB and a C toolchain on Windows |
| `redb` or `sled` key-value store | 1 to several | No: every value would have to be sealed by hand anyway | Gains little over plain files once every value is sealed |

## Behaviour

### Sending

- `dmSend(conversation, text, reply_to?)`: check the limits, make the
  event (its `seq`, D2), store it (sending), then encrypt it for the chat's
  connected devices plus your own (e2ee D5) and send. The rest get it when
  they connect (D5).
- On the first `ok` from one of the other person's devices, mark it sent. A
  `failed`, or a fresh session from that device, sends it again (D5).
- Edits, deletes and reactions go the same way, to the same devices.
- A 1:1 chat exists as soon as two people are friends. Its ID comes from the
  pair (e2ee D10). A group's ID is random (e2ee D10), and creating it sends a
  `Group` event naming the members.

### Receiving

- e2ee opens the envelope and checks the sender (e2ee *Receiving*). The
  `dm` layer parses the event (dropped if bad, D14), checks it belongs (a
  member, the author for edits and deletes), stores it, syncs it to disk,
  answers `ok` (or `failed` if it couldn't open or store it), and tells
  Mesh-Voice if that chat is open, or notifies.
- A message from someone you've blocked is dropped (social D10). A group
  message from someone you've since unfriended is still shown in that
  group, because you both stay members.
- Removed from a group: the chat stays, read-only, with a system line.

### Sync between your devices (D8)

- On a connection between two of your own devices, each sends the other
  everything after that device's counter (D5), 200 per batch, newest chat
  first. Each event goes through e2ee like any other (Olm, own devices) and
  is acknowledged the same way.
- A linked device is new, so its counter is 0 and it gets everything. On a
  restore from backup, the device key is new (identity), so the same happens.

### Attachments (D12, D13)

- `dmSendFile(conversation, path, preview?)`: encrypt into `<data>/blobs/`,
  hash, then send a `Text` event carrying the file.
- `dmFetch(conversation, event, index)`: ask connected chat devices for the
  blob's chunks. Write the decrypted file to the cache. Answers `meshDmFile
  {path}`, or `meshError {kind: "unavailable"}` when nobody connected has it.
- The `blob` service answers only devices of people in a chat that holds
  that blob, at up to 8 chunk requests at a time per device.

### Client side (the helper)

New commands, JSON lines as now:

| Command | Answer |
|---|---|
| `dmList()` | `meshDmChats {chats: [{id, kind: direct\|group, name, members, last, unread, mentions}]}`, also sent whenever one changes |
| `dmOpen(conversation, before?)` | `meshDmMessages {id, events: [...], more: bool}`, the newest 50 (or 50 before `before`). Sent again while that chat is open when it changes |
| `dmSend`, `dmEdit(event, text)`, `dmDelete(event, everyone)`, `dmReact(event, emoji, on)` | `meshDmMessages` for the open chat |
| `dmRead(conversation, event)` | `meshDmChats` |
| `dmTyping(conversation)` | nothing. The reader gets `meshDmTyping {id, who: [ids]}` |
| `dmGroupCreate(ids, name)`, `dmGroupAdd`, `dmGroupRemove`, `dmGroupLeave`, `dmGroupRename` | `meshDmChats` |
| `dmSendFile`, `dmFetch` | as above |

- A new message in a chat that isn't open: `meshDmNotify {id, from, text}`
  (text cut to 100 characters, as WhatsApp's notify).
- Errors: `not-a-friend` (with `who`), `too-long`, `too-many`, `not-member`,
  `not-author`, `unavailable`, `locked`.
- `e2eeNote` stays in the helper for the process tests. Mesh-Voice's test
  note buttons go (Q4).

### Mesh-Voice

- Mesh DMs follow the path WhatsApp already uses (`web_chat`): the chat list
  under Mesh in the sidebar, the existing timeline and composer, replies,
  edits, reactions, the typing line, unread badges, notification sounds.
- `meshDmMessages` maps onto `TimelineMessage` (`web_id` is the event ID).
  Pictures use the preview first, then `dmFetch`. Other files open on click,
  as WhatsApp's do.
- Home's friend rows and a friend's profile get **Message**. A **New group**
  button picks friends.
- A message shows a small "sending" mark until it's sent.

### What `mailbox` changes

Here a message reaches someone only while one of your devices and one of
theirs are online at the same time. `mailbox` adds friends and seeds that
hold sealed envelopes (e2ee D8). It plugs in at D5: at hand-over time the
sender encrypts the waiting events into envelopes for a holder, and the
reader's ack moves the counter as now. Nothing in the event format or the
store changes.

**A note for the `mailbox` spec: keys for someone who's been away** (review
C, moved here 2026-11-11). dm doesn't need this: with D5, events are
encrypted when a connection opens, and by then the sender holds a fresh
address record with keys. `mailbox` hands envelopes to a holder while the
reader is away, so it does. The idea: each device also publishes a
**keys-only record** per contact: its signed device list, device key, e2ee
bundle and fallback key, no address and no one-time key. It lasts **7
days**, the fallback key's life (e2ee), under a weekly lookup (the pair's
lookup for the current week, as routing D1 derives the hourly one). It's put
again when the fallback key changes, and otherwise once a day. The seed
would need its own limit for how far ahead such a record may expire
(7 days, against 3 hours for address records). Not 30 days: a record
carrying an expired fallback key is no use. This changes routing (a second
record kind) and the seed; those specs change with `mailbox`.

## Tech stack

**No new dependencies.** `postcard`, `serde`, `blake3` and `chacha20poly1305`
are already in the workspace. Mesh-Voice's `image` crate makes previews.

## Commands

```
cargo test --workspace                       # Mesh-Node
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p mesh-app -p mesh-discord       # Mesh-Voice
```

## Project structure

```
Mesh-Node/
  crates/mesh-dm/                           new crate: pure, no networking
    src/event.rs                            the event format, limits, parse
    src/chat.rs                             folding events into a chat: D2–D4, D7
    src/store.rs                            sealed segments and the index (D11)
    src/sync.rs                             per-device counters (D8)
    src/blob.rs                             file encryption and chunking (D12)
  crates/mesh-node/src/dm.rs                the helper side: delivery, waiting, sync, blob service
  crates/mesh-node/src/helper.rs            the dm* commands
  crates/mesh-node/tests/dm.rs              a seed, Alice on two devices, Bob and Carol
Mesh-Voice/
  crates/mesh-discord/src/mesh.rs           the dm events and commands
  crates/mesh-app/src/…                     Mesh DMs in the chat view
```

A separate crate keeps chat logic testable without a network, and lets
`spaces` use it without the helper.

## Code style

As routing, social and e2ee: pure, unit-tested modules, limits as named
constants, the context `mesh/v1/dm-store` as a constant. `mesh-dm` never
names vodozemac types. It sees opened bodies only.

## Testing strategy

- **Unit tests**:
  - `event.rs`: round trip for each event kind. Oversized, unknown and
    truncated events are refused.
  - `chat.rs`: order by `seq` with skewed clocks; an old sender time
    can't move a message up. Ahead of the clock: exactly 5
    minutes ahead is shown, a millisecond more waits and shows once the
    clock catches up, more than a day ahead is dropped, the 1,001st waiting
    event from one device is dropped. A sender ignores a held `seq` ahead of
    its clock. A forger sending big jumps to everyone but one member can't
    keep that member from taking the next honest event. A new device with
    nothing takes the first event it gets. Out of step: a higher `n` with a lower
    `seq` is refused, whichever arrives first, while honest events arriving
    out of order are all taken. Late and low: an event below the read
    marker counts as unread and is marked late. An edit or reaction
    arriving before its target. Only the author edits. A delete beats a
    late copy. Group replay with concurrent add and remove. A removal by a
    non-creator is ignored. A removed member's back-dated add (a fork from
    before the removal) loses to the creator's branch, also when the
    creator's change is not the branch's first. Both branches with creator
    changes: the earlier creator change wins. The same winner whatever
    order the changes arrive in.
  - `store.rs`: append and reopen. Opening reads one segment. Edit a
    message, then delete it: neither text is left on disk (the test greps
    the files). A damaged or swapped file is refused. A crash mid-append keeps
    everything before it.
  - `sync.rs`: two devices with gaps converge. Duplicates are ignored. A
    new device gets everything. The counter stops at a `failed` and moves
    once the resend is `ok`. A friend's new device starts at their lowest
    counter.
  - `blob.rs`: round trip. A bad chunk is refused as it arrives. A file cut
    short, or with a wrong whole-file hash, is refused. The wrong key fails.
- **Process test** (`tests/dm.rs`): a seed, Alice on a desktop and a
  laptop (from her backup), Bob and Carol. All are friends.
  - Alice messages Bob. Bob's device and Alice's laptop get it. Bob replies,
    edits, reacts and deletes. Alice's two devices agree.
  - Bob locks. Alice sends three messages and locks. Both unlock: they
    arrive in order. The status went from sending to sent.
  - Bob's second device can't open Alice's first message (its one-time key
    went to his first device, e2ee D4). It sends a fresh session back, and
    Alice's message arrives on it.
  - Alice's laptop locks for a while, then unlocks. It gets what it missed
    from the desktop (D8).
  - A new device restored from Alice's backup gets her whole history.
  - Alice makes a group with Bob and Carol and sends a 5 MB file. Carol
    fetches it from Bob while Alice is locked. Alice removes Carol, and
    Carol can't read the next message.
  - The seed's files and every helper's stderr contain no ID, conversation
    ID or message text (the test greps them).
- **Two-machine check** (by hand, the module gate, with the homelab seed and
  the Windows PC):
  1. Windows and the Linux box (friends from social's gate) chat 1:1. Each
     message shows in under 1 s.
  2. Edit, delete, react and reply from both sides.
  3. Close Mesh on Linux. Send from Windows. Reopen Linux while Windows is
     still on: it arrives within 5 s.
  4. Send a 20 MB file and a picture from Windows. Its preview shows at
     once and the file opens.
  5. Link a second Linux device: it shows the whole history.
  6. `docker logs` on the seed shows counts only.

## Boundaries

- **Always:**
  - Send every chat event through e2ee (D1). Typing is the only exception
    (D10).
  - Store history only sealed under the data key (D11).
  - Drop, never show, an event that doesn't parse or doesn't belong (D14).
  - Check every blob chunk against its tag before using it, and the whole
    file against its hash and size (D12).
- **Ask first:**
  - Any new dependency.
  - Changing D1–D15.
  - Link previews, or anything else that fetches URLs found in messages.
  - Read receipts shown to others.
- **Never:**
  - Keep plaintext history outside the sealed store, including logs and
    temp files. A fetched file opened in its app is the exception, in the
    cache, as WhatsApp's are.
  - Share old message keys for history (e2ee D13).
  - Relay messages for people who aren't friends before `spaces` designs
    that.
  - Name the homelab in the public Mesh-Voice repo.

## Success criteria

1. `mesh-dm` and the helper changes pass their tests on Linux and Windows.
   Clippy and fmt are clean.
2. The process test passes, covering: live chat, edits, deletes, reactions,
   waiting while both sides lock, own-device catch-up, a restored device's
   history, a group with a file, and member removal.
3. On the gate, a live message shows in under 1 s. A waiting one shows
   within 5 s of the reader connecting.
4. Opening a chat with 100,000 messages shows its newest page in under
   50 ms on the Linux box. A new device takes in 10,000 events in under
   30 s on the LAN. Measured with an `--ignored` test and recorded in the
   plan.
5. The release binary grows by under 300 KB.
6. The seed's files and logs, and the helper's stderr, contain no ID,
   conversation ID or message text.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Before `mailbox`, a message waits until both people are online at once | High | Shown honestly as "sending". Friends dial each other at unlock and every 30 minutes. `mailbox` is next. |
| Clocks that are badly wrong reorder a chat | Low | Order comes from `seq`, not time (D2); a wrong clock only changes the time shown. |
| Group members miss a change and disagree on who's in | Medium | Changes are ordinary events, so they wait and sync like messages, and replay converges (D7). A test covers concurrent changes. |
| A modified client ignores deletes | Low | Accepted (map). Deleting still removes it everywhere honest. |
| Files are unavailable when everyone who has them is offline | Medium | The preview always travels. `unavailable` is shown plainly. `mailbox` may hold small files later. |
| History sync to a new device is heavy for long chats | Low | Newest chat first, in batches (D8), so recent chats appear first. Measured in success criterion 4. |

## Decided questions

All four taken as recommended (2026-11-11).

- **Q1. Must everyone in a group DM be friends with each other?**
  Decided: **yes, for v1** (D6). The alternative is letting a member
  pass messages on to members who aren't your friends. That's sealed
  relaying, which `spaces` must build anyway. Group DMs could then drop the
  rule. Discord lets any friend of the creator in, so until then this is
  stricter than Discord.
- **Q2. The largest file?** Decided: **100 MB.** Discord's free limit is
  10 MB. Ours costs only the senders' and members' upload bandwidth, but a
  bigger file takes much longer from a home connection. 25 MB or 500 MB are
  the other sensible choices.
- **Q3. How much history does a new device get?** Decided:
  **everything** (D8), newest chats first. The alternative is the last 90
  days, which is faster to link but loses old chats on that device.
- **Q4. Remove the test-note buttons from Mesh-Voice once DMs work?**
  Decided: **yes**. The helper keeps `e2eeNote` for the process tests.

## Open questions

None.
