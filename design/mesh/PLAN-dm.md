# Plan: `dm` (Mesh)

Status: **APPROVED 2026-11-11**, with the clock-anchored D2 and the intake
measured right after Task 9. Implements
[SPEC-dm.md](SPEC-dm.md) (approved 2026-11-11). No dm code until this plan
is approved.

The code goes in `GloriousBrendon/Mesh-Node` (a new `mesh-dm` crate, a batch
call in `mesh-e2ee`, the helper's side) and Mesh-Voice (Mesh DMs in the chat
view).

## Overview

Pure pieces first, in `mesh-dm`: the event format, folding events into a
chat, the sealed store, the per-device counters and file encryption. Then
the one `mesh-e2ee` change, a batch decrypt. Then the helper: delivery and
acks, own-device sync, the commands, typing, files, and a process test. Then
the Mesh-Voice screens. Hard stops at each checkpoint.

## Architecture decisions

- **`mesh-dm` is pure.** No networking and no vodozemac types. It sees
  opened bodies, the sender's ID and device, and the time the caller passes.
  Its only I/O is its own store directory. `spaces` reuses it.
- **One `Dm` value per unlocked device**, owned by the helper on its main
  loop, beside `E2ee` and the keystore. Opening a chat reads its newest
  segment only (D11).
- **Acks ride back on the `e2ee` stream.** The reader answers each envelope
  in order, one frame each: `ok` or `failed`, after it has opened *and
  stored* the event (D5). The service's frame format gets one new flag bit
  for "answer expected"; test notes don't set it, so they work as now.
- **The send queue is the store.** Nothing waits in memory. When a
  connection to a device opens, the helper asks `mesh-dm` for that
  device's events after its counter, in stored order (D2, D5, D8), encrypts
  them on the current session or chain, and sends them.
- **Typing is its own service**, `typing`, one frame per signal:
  `conversation [16]`. It goes only to connected devices of chat members and
  is never stored (D10).
- **Files move on a `blob` service**: request `hash [32] | chunk u32`,
  answer the sealed chunk. A device answers only members of a chat that
  holds that blob, at most 8 requests in flight per device (D12).
- **No new dependencies.** `postcard`, `serde`, `blake3` and
  `chacha20poly1305` are in the workspace. Mesh-Voice's `image` crate makes
  previews.

## Saving and loading (added 2026-11-11, before Task 9)

Everything lives under `<data>/dm/`, sealed as the store's segments are.
The rule: storing one event appends; nothing that grows with history is
ever rewritten for it.

- **What's stored per event.** A chat segment's record is the store's
  opaque bytes; the dm layer makes it `received u64 (ms) | sender person
  [32] | sender device [32] | event`. A reopened chat therefore shows the
  same times (D2's display clamp uses `received`) and knows who sent each
  event, since envelopes aren't kept. Reloading goes through
  `Chat::restore`, which replays stored events in stored order without
  judging them again; one still more than 5 minutes ahead of the clock
  waits as before.
- **The `Log` (sync.rs)** is one more append-only stream, in the same
  sealed segment format, under a reserved name: one record of about 110
  bytes per stored event, appended in the same call as the event. At
  unlock it's read once, whole, to rebuild the numbering and the set of
  every event ID held: 100,000 events is about 11 MB to open, and 1.6 MB of
  IDs in memory. That set is what refuses a late copy of any old message,
  deleted or not, before it reaches a chat.
- **`Counters` (sync.rs)** are one small sealed file, written atomically
  after each batch of acks, not each event. Its size depends on the number
  of devices and the acks in flight past a failure (at most a batch), never
  on history.
- **Deletion markers.** The log also records each deleted event ID (16
  bytes each), read with it at unlock. *(First planned as a list per chat;
  one stream is simpler.)* A
  late copy, edit or reaction of a message deleted long ago is refused
  from it (`Taken::Ignored`) and never written, though the message's own
  segment isn't loaded.
- **The index** (D11) also keeps the read marker's position and, per
  sending device, the highest `n` seen with its `seq`. With only the
  newest segment loaded, "late and low" still compares against the real
  marker, and "out of step" checks a new event against that device's
  highest `n` as well as against the loaded window. That's weaker than the
  full check against history (a forged event between two old ones isn't
  caught), and costs nothing honest: an old event arriving late from one
  device still has a lower `seq` than that device's newest.
- **Edits and reactions to old messages** whose segment isn't loaded wait,
  stored, and fold in when that segment is read. One from someone who
  may not make it is wiped then (`take_scrub`), as now.

**Spec change needed: D11's wording only.** It lists segments and an index
per chat. It would add the device-wide `Log` and `Counters` files, the
per-chat deletion list, the stored record's received time and sender, and
the index's read marker and per-device highest `n`. No decision changes.

## Task list

### Phase 1: `mesh-dm` (Mesh-Node)

- [x] **Task 1: The event format** (D1, D14)
  - What: `crates/mesh-dm`, `event.rs`. Version, event ID, sender time,
    `seq`, `n`, and one of `Text` (with reply and attachments), `Edit`,
    `Delete`, `React`, `Read`, `Group`. The limits as constants: 4,000
    characters, 10 attachments, 20 emoji per message, preview 16 KiB.
  - Accept: round trip for each kind; oversized, unknown and truncated
    events are refused.
  - Verify: `cargo test -p mesh-dm event`. Size S.
- [x] **Task 2: Order and the `seq` checks** (D2)
  - What: `chat.rs`, the order part. Order by (`seq`, event ID). The
    sender's next `seq`: one more than the highest held that isn't ahead of
    its clock. The three checks, against the reader's clock and the
    sender's earlier events only: ahead of the clock (more than 5 minutes
    waits, more than a day is dropped, at most 1,000 waiting per sending
    device), out of step (per sending device, by `n`), late and low
    (unread, marked late). The 5-minute display clamp.
  - Accept: the spec's `chat.rs` tests for D2: exactly 5 minutes ahead is
    shown, a millisecond more waits and shows once the clock catches up,
    over a day is dropped; the 1,001st waiting event is dropped; a sender
    ignores a held `seq` ahead of its clock; big jumps sent to everyone but
    one member don't stop that member taking the next honest event; a new
    device takes its first event; an out-of-step event is refused whichever
    arrives first, while honest events out of order are all taken; late
    and low counts as unread; an old sender time can't move a message up.
  - Verify: `cargo test -p mesh-dm chat`. Size M.
- [x] **Task 3: Edits, deletes, reactions** (D3, D4)
  - What: `chat.rs`. Only the author edits or deletes; the newest edit
    wins; a delete leaves a tombstone and takes its edits with it; delete
    for me; reactions per (target, person, emoji); anything before its
    target waits for it.
  - Accept: an edit or reaction before its target; a delete beats a late
    copy and a late edit; someone else's edit is ignored.
  - Verify: `cargo test -p mesh-dm chat`. Size M.
- [x] **Task 4: Group changes** (D7)
  - What: `chat.rs`. Changes chained by `after`; accepted only from a
    member of the version they follow, with the right to make that change;
    a change waits for its `after`; forks by the creator rule.
  - Accept: concurrent add and remove; a non-creator's removal ignored; a
    removed member's back-dated add loses, also when the creator's change
    isn't the branch's first; both branches with creator changes; the same
    winner in any arrival order.
  - Verify: `cargo test -p mesh-dm chat`. Size M.
- [x] **Task 5: The store** (D11)
  - What: `store.rs`. A folder per chat named by a keyed hash, segments of
    512 sealed events under `mesh/v1/dm-store`, the segment name and
    position as associated data, an index file per chat. Appends are synced
    before they're acknowledged, once per batch. A delete rewrites,
    atomically, each segment holding the message or one of its edits.
  - Accept: append and reopen; opening reads one segment; edit then delete
    leaves neither text on disk (the test greps); a damaged or swapped file
    is refused; a crash mid-append keeps everything before it.
  - Verify: `cargo test -p mesh-dm store`. Size L.
- [x] **Task 6: Counters** (D5, D8)
  - What: `sync.rs`. Each stored event's number; per target device, the
    number up to which it acknowledged everything; what to send it next, in
    stored order; for a friend's device only your own events in chats with
    them, up to 30 days and 2,000 events; a friend's new device starts at
    their lowest counter.
  - Accept: two devices with gaps converge; duplicates are ignored; a new
    own device gets everything; the counter stops at a `failed` and moves
    once the resend is `ok`; a friend's new device starts at their lowest
    counter; past the limits, events are marked not delivered to that
    device.
  - Verify: `cargo test -p mesh-dm sync`. Size M.
- [x] **Task 7: Files** (D12)
  - What: `blob.rs`. A random key per file, 64 KiB chunks sealed with the
    chunk number as associated data, the whole file's blake3 hash and size.
  - Accept: round trip; a bad chunk is refused as it arrives; a cut-short
    file or a wrong whole-file hash is refused; the wrong key fails.
  - Verify: `cargo test -p mesh-dm blob`. Size S.

**Checkpoint A (short report):** `mesh-dm` complete, its tests pass.

Result (2026-11-11, Mesh-Node `c4f9d05` to `7fbcba8`): reached. 40 tests in
`mesh-dm`, all workspace checks pass. One change from the plan: group changes
live in their own `group.rs`, which `chat.rs` calls, to keep `chat.rs` to
order and folding. A Read clears every late mark (it can't say which ones
its device had seen); noted in the code.

### Phase 2: the batch decrypt (Mesh-Node)

- [x] **Task 8: `E2ee::decrypt_batch`** (D11) *Done 2026-11-11 (Mesh-Node
  `2225959`): 1,000 envelopes, one save; a failed save hands nothing up.*
  - What: open many envelopes, save each person's state once, then hand them
    all up. "Save before handing up" holds per batch. `decrypt` stays, as a
    batch of one.
  - Accept: 1,000 envelopes from one sender cause one state write (counted
    in a test); a crash between save and hand-up loses nothing (the store
    test's style); the existing tests pass unchanged.
  - Verify: `cargo test -p mesh-e2ee`. Size M. SPEC-e2ee gets a line
    saying so; no format change.

### Phase 3: the helper (Mesh-Node)

- [x] **Task 9: Delivery and acks** (D5)
  - What: `crates/mesh-node/src/dm.rs`, a child module of the helper as
    `social.rs` is. Store the event, encrypt for connected devices, send;
    on a connection opening, send what's after that device's counter; the
    reader answers `ok`/`failed` after storing; a fresh session from a
    device resends everything it hasn't acknowledged; sending then sent.
  - Accept: unit tests over the frame and ack logic; the e2ee test notes
    still pass; a chat reopened from disk shows the same times and refuses
    a late copy of a message deleted before the reopen (*Saving and
    loading*).
  - Size L.
  - Result (2026-11-11, Mesh-Node `7b8f71f`, `dd4427d`, `608d34d`): built
    for 1:1 chats; groups and own-device sync come with Tasks 10 and 11.
    The history lives in `mesh-dm`'s `history.rs`. Deletion markers went
    into the device-wide log rather than a list per chat: one stream, read
    once at unlock. `mesh-e2ee` gained `encrypt_device_batch` (one device,
    many bodies, one save). A "fresh session" is recognised as an envelope
    that opens to nothing; it triggers a resend to that device.
- [x] **Task 9b: Measure the intake** (criterion 4, early)
  - What: the `--ignored` intake test from Task 15, run now: one device
    takes in 10,000 events from another (target under 30 s on the LAN),
    so a miss is known before Tasks 10–14. Result recorded here.
  - Size S.
  - Result (2026-11-11, Ryzen 7 7700X, NVMe, release build): **1.5 s** for
    10,000 events in all (the sender reading them back, encrypting, framing
    and taking the acks), 0.8 s of it the reader opening and storing them.
    Criterion 4 is 30 s. Three runs within 0.05 s of each other.
    - It's measured as a new device of our own, which gets everything. A
      friend's device would get only the newest 2,000 (D5); the test caught
      that. Own-device encryption comes with Task 10, so the envelopes use
      the pair's session: the same work per event.
    - The network isn't in it: 10,000 frames are about 3 MB.
    - On `/tmp` (RAM here) it read 1.1 s; the files are now on the real
      disk.
    - Seen on the way: **storing a sent message costs about 3 ms** (two log
      syncs and the chat's small index rewritten per send). Fine for a
      person typing; a batch of sends should share one sync (Task 11).
    - A debug build takes 33 s: unoptimised crypto. Only release counts.
- [x] **Task 10: Own-device sync** (D8) *Done 2026-11-11 (Mesh-Node
  `97a8f9b`, `31a9582`): over the self pair's conversation, each event
  forwarded with its chat, author and first arrival; "read" and "delete for
  me" never go to friends.*
  - What: the same path aimed at your own devices: everything after the
    counter, 200 a batch, newest chat first, stored order within a chat,
    opened with the batch call.
  - Size M.
- [x] **Task 11: Commands** (spec *Client side*) *Done 2026-11-11
  (Mesh-Node `4fe4e4c` 1:1, `39060e1` groups; process tests in
  `tests/dm.rs`). Group DMs go over the 1:1 sessions (your call, e2ee D6).
  Two rules found while building, for your review: a member added later
  gets the group's messages from their add on (its changes always, to know
  who's in it); and a change goes to everyone in the version before or
  after it, so a removal reaches the person removed.*
  - What: `dmList`, `dmOpen`, `dmSend`, `dmEdit`, `dmDelete`, `dmReact`,
    `dmRead`, `dmGroupCreate/Add/Remove/Leave/Rename`; the events
    `meshDmChats`, `meshDmMessages`, `meshDmNotify`; the errors. Group
    membership checks friendship with every member (D6, Q1).
  - Size M.
- [x] **Task 12: Typing** (D10) *Done 2026-11-11 (Mesh-Node `27b0798`):
  its own `typing` service, at most one signal per device every 2 s.*
  - What: the `typing` service; `dmTyping` and `meshDmTyping`, shown 7 s.
  - Size S.
- [x] **Task 13: Files** (D12, D13) *Done 2026-11-11 (Mesh-Node `7eee4b2`).
  Two gaps, both noted in the code: a fetch asks devices one after another,
  not 8 at once; and the preview travels as hex in `meshDmMessages`.*
  - What: the `blob` service; `dmSendFile`, `dmFetch`, `meshDmFile`; the
    10 GB cache, oldest-opened first, files you sent last; a delete drops
    the blob.
  - Size M.
- [x] **Task 14: The process test** (`tests/dm.rs`, `the_process_test`, about 80 s)
  - What: the spec's process test: live chat, edits, deletes, reactions;
    waiting while both lock; Bob's second device getting a fresh session
    and then Alice's message; own-device catch-up; a restored device's
    history; a group with a 5 MB file fetched from a member; member
    removal; no ID, conversation ID or text in any log or the seed's files.
  - Size L.
- [x] **Task 15: Measurements** (criteria 4, 5)
  - What: the `--ignored` tests: open a 100,000-message chat (target under
    50 ms), the intake again (Task 9b) with sync in place, and the release
    binary's growth (target under 300 KB). Also **unlock time and memory
    with 100,000 and 1,000,000 events in the history**: the log is read
    whole at unlock and held in memory (about 150 bytes an event by
    estimate, so 15 MB and 150 MB). Results recorded here.
  - Size S.
  - Results (2026-11-11, Ryzen 7 7700X, NVMe, release build; each run
    three times, within 1%):

    | Measure | Target | Result |
    |---|---|---|
    | Newest page of a 100,000-message chat | under 50 ms | **0.6 ms** |
    | Intake of 10,000 events, sync in place | under 30 s | **1.48 s** (0.82 s the reader); unchanged from Task 9b |
    | Release binary's growth (stripped, before Task 1 to now) | under 300 KB | **+691 KB** (24.93 to 25.63 MB): over the target, accepted as the result (2026-11-12), no build-setting changes |
    | Unlock with 100,000 events | — | **170 ms**, 16 MB held (about 170 bytes an event), 34 MB on disk |
    | Unlock with 1,000,000 events | — | **1.7 s**, 168 MB held, 347 MB on disk |

    - The growth is mesh-dm itself (258 KB), the helper's dm and files
      code (+134 KB), and std, tokio and serde code made for the new types
      (+285 KB). No outside dependency came in.
    - Unlock and memory grow in step with the history: the log is read
      whole and held. 1,000,000 events is years of heavy use. Accepted for
      dm (2026-11-12); `spaces` must change it before reusing the store
      (noted in CAPABILITY-MAP and SPEC-dm).
    - The measures: `at_scale` in mesh-dm (`MESH_EVENTS=1000000 cargo
      test --release -p mesh-dm at_scale -- --ignored --nocapture`, run
      twice: the first builds the history under `target/`), and the intake
      test in mesh-node.

*Checkpoint B moved to after Task 17 (2026-11-11): without the screens it
could only be done by typing JSON commands into two helpers, which checks
nothing the process test doesn't.*

### Phase 4: Mesh-Voice

- [x] **Task 16: `mesh.rs`**: the dm commands and events, with parse
  tests. Size S.
- [x] **Task 17: Chats**: Mesh DMs in the sidebar under Mesh, opening in
  the existing timeline and composer (`TimelineMessage`, `web_id` = event
  ID), replies, edits, reactions, the typing line, unread badges and
  sounds, a "sending" mark. Size L.
**Checkpoint B (by hand, you), after Task 17:** two windows on one machine with the home
seed. Live chat both ways, edit, delete, react, a picture, and a message
sent while the other window is closed.

- [ ] **Task 18: Files and entry points**: pictures from the preview
  first, then `dmFetch`; **Message** on friend rows and profiles; **New
  group**. Size M. **Clicking a received file never runs it:** pictures
  and other known-safe types (images, audio, video, PDF, plain text) open
  in their app; anything else is shown in its folder. The check is on
  the file's type as Mesh reads it, not the name the sender gave. Test:
  a received `.exe`, `.bat`, `.sh` or `.lnk` is shown in its folder and
  never opened.
- [ ] **Task 19: Retire test notes** (Q4): the buttons go; the helper keeps
  `e2eeNote` for its tests. Size S.

**Checkpoint C (module gate, needs you):** the spec's two-machine check on
the Windows PC and the Linux box. Across networks it needs the VPS seed
(finding 6), like every other module's gate.

## Order and size

Tasks 1–7 can go in any order after Task 1; Task 8 before Task 9; Task 9b
straight after Task 9; Tasks 10–15 in order; Phase 4 after Task 15, with Checkpoint B after Task 17. Roughly 4,000 lines in
Mesh-Node and 1,200 in Mesh-Voice.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| A reader's clock is badly wrong | Low | Events ahead of it wait instead of being lost, up to a day; Mesh-Voice says the clock looks wrong while any wait. |
| Criterion 4 misses with Olm's per-message cost | Medium | Task 8 removes the disk cost first; Task 15 measures before Phase 4. |
| Segment rewrites on delete race appends | Low | One writer: the helper's loop owns the store. Task 5 tests a delete between appends. |
| Acks change the `e2ee` service | Low | A flag bit, unset for test notes; Task 9 keeps their tests passing. |
| The process test gets slow | Medium | It waits for state, not the next event, with short record lifetimes, as the earlier ones do. |

## Open questions

None. (Group-DM encryption: decided 2026-11-11, (b).)
