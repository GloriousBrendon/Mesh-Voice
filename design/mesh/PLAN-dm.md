# Plan: `dm` (Mesh)

Status: **DRAFT 2026-11-11, plan only, for review.** Implements
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

## Task list

### Phase 1: `mesh-dm` (Mesh-Node)

- [ ] **Task 1: The event format** (D1, D14)
  - What: `crates/mesh-dm`, `event.rs`. Version, event ID, sender time,
    `seq`, `n`, and one of `Text` (with reply and attachments), `Edit`,
    `Delete`, `React`, `Read`, `Group`. The limits as constants: 4,000
    characters, 10 attachments, 20 emoji per message, preview 16 KiB.
  - Accept: round trip for each kind; oversized, unknown and truncated
    events are refused.
  - Verify: `cargo test -p mesh-dm event`. Size S.
- [ ] **Task 2: Order and the `seq` checks** (D2)
  - What: `chat.rs`, the order part. Order by (`seq`, event ID). The three
    checks: too high (over 1,000,000 above the highest held), out of step
    (per sending device, by `n`), late and low (unread, marked late). The
    baseline in an empty chat and for a device's first event. The 5-minute
    display clamp.
  - Accept: the spec's `chat.rs` tests for D2: exactly 1,000,000 above is
    taken and one more refused; an empty chat takes a first event at
    5,000,000; an out-of-step event is refused whichever arrives first,
    while honest events out of order are all taken; late and low counts as
    unread; an old sender time can't move a message up.
  - Verify: `cargo test -p mesh-dm chat`. Size M.
- [ ] **Task 3: Edits, deletes, reactions** (D3, D4)
  - What: `chat.rs`. Only the author edits or deletes; the newest edit
    wins; a delete leaves a tombstone and takes its edits with it; delete
    for me; reactions per (target, person, emoji); anything before its
    target waits for it.
  - Accept: an edit or reaction before its target; a delete beats a late
    copy and a late edit; someone else's edit is ignored.
  - Verify: `cargo test -p mesh-dm chat`. Size M.
- [ ] **Task 4: Group changes** (D7)
  - What: `chat.rs`. Changes chained by `after`; accepted only from a
    member of the version they follow, with the right to make that change;
    a change waits for its `after`; forks by the creator rule.
  - Accept: concurrent add and remove; a non-creator's removal ignored; a
    removed member's back-dated add loses, also when the creator's change
    isn't the branch's first; both branches with creator changes; the same
    winner in any arrival order.
  - Verify: `cargo test -p mesh-dm chat`. Size M.
- [ ] **Task 5: The store** (D11)
  - What: `store.rs`. A folder per chat named by a keyed hash, segments of
    512 sealed events under `mesh/v1/dm-store`, the segment name and
    position as associated data, an index file per chat. Appends are synced
    before they're acknowledged, once per batch. A delete rewrites,
    atomically, each segment holding the message or one of its edits.
  - Accept: append and reopen; opening reads one segment; edit then delete
    leaves neither text on disk (the test greps); a damaged or swapped file
    is refused; a crash mid-append keeps everything before it.
  - Verify: `cargo test -p mesh-dm store`. Size L.
- [ ] **Task 6: Counters** (D5, D8)
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
- [ ] **Task 7: Files** (D12)
  - What: `blob.rs`. A random key per file, 64 KiB chunks sealed with the
    chunk number as associated data, the whole file's blake3 hash and size.
  - Accept: round trip; a bad chunk is refused as it arrives; a cut-short
    file or a wrong whole-file hash is refused; the wrong key fails.
  - Verify: `cargo test -p mesh-dm blob`. Size S.

**Checkpoint A (short report):** `mesh-dm` complete, its tests pass.

### Phase 2: the batch decrypt (Mesh-Node)

- [ ] **Task 8: `E2ee::decrypt_batch`** (D11)
  - What: open many envelopes, save each person's state once, then hand them
    all up. "Save before handing up" holds per batch. `decrypt` stays, as a
    batch of one.
  - Accept: 1,000 envelopes from one sender cause one state write (counted
    in a test); a crash between save and hand-up loses nothing (the store
    test's style); the existing tests pass unchanged.
  - Verify: `cargo test -p mesh-e2ee`. Size M. SPEC-e2ee gets a line
    saying so; no format change.

### Phase 3: the helper (Mesh-Node)

- [ ] **Task 9: Delivery and acks** (D5)
  - What: `crates/mesh-node/src/dm.rs`, a child module of the helper as
    `social.rs` is. Store the event, encrypt for connected devices, send;
    on a connection opening, send what's after that device's counter; the
    reader answers `ok`/`failed` after storing; a fresh session from a
    device resends everything it hasn't acknowledged; sending then sent.
  - Accept: unit tests over the frame and ack logic; the e2ee test notes
    still pass.
  - Size L.
- [ ] **Task 10: Own-device sync** (D8)
  - What: the same path aimed at your own devices: everything after the
    counter, 200 a batch, newest chat first, stored order within a chat,
    opened with the batch call.
  - Size M.
- [ ] **Task 11: Commands** (spec *Client side*)
  - What: `dmList`, `dmOpen`, `dmSend`, `dmEdit`, `dmDelete`, `dmReact`,
    `dmRead`, `dmGroupCreate/Add/Remove/Leave/Rename`; the events
    `meshDmChats`, `meshDmMessages`, `meshDmNotify`; the errors. Group
    membership checks friendship with every member (D6, Q1).
  - Size M.
- [ ] **Task 12: Typing** (D10)
  - What: the `typing` service; `dmTyping` and `meshDmTyping`, shown 7 s.
  - Size S.
- [ ] **Task 13: Files** (D12, D13)
  - What: the `blob` service; `dmSendFile`, `dmFetch`, `meshDmFile`; the
    10 GB cache, oldest-opened first, files you sent last; a delete drops
    the blob.
  - Size M.
- [ ] **Task 14: The process test** (`tests/dm.rs`)
  - What: the spec's process test: live chat, edits, deletes, reactions;
    waiting while both lock; Bob's second device getting a fresh session
    and then Alice's message; own-device catch-up; a restored device's
    history; a group with a 5 MB file fetched from a member; member
    removal; no ID, conversation ID or text in any log or the seed's files.
  - Size L.
- [ ] **Task 15: Measurements** (criteria 4, 5)
  - What: an `--ignored` test: open a 100,000-message chat (target under
    50 ms), take in 10,000 events (target under 30 s), and the release
    binary's growth (target under 300 KB). Results recorded here.
  - Size S.

**Checkpoint B (by hand, you):** two windows on one machine with the home
seed. Live chat both ways, edit, delete, react, a picture, and a message
sent while the other window is closed.

### Phase 4: Mesh-Voice

- [ ] **Task 16: `mesh.rs`**: the dm commands and events, with parse
  tests. Size S.
- [ ] **Task 17: Chats**: Mesh DMs in the sidebar under Mesh, opening in
  the existing timeline and composer (`TimelineMessage`, `web_id` = event
  ID), replies, edits, reactions, the typing line, unread badges and
  sounds, a "sending" mark. Size L.
- [ ] **Task 18: Files and entry points**: pictures from the preview
  first, then `dmFetch`; other files open on click; **Message** on friend
  rows and profiles; **New group**. Size M.
- [ ] **Task 19: Retire test notes** (Q4): the buttons go; the helper keeps
  `e2eeNote` for its tests. Size S.

**Checkpoint C (module gate, needs you):** the spec's two-machine check on
the Windows PC and the Linux box. Across networks it needs the VPS seed
(finding 6), like every other module's gate.

## Order and size

Tasks 1–7 can go in any order after Task 1; Task 8 before Task 9; Tasks
9–15 in order; Phase 4 after Checkpoint B. Roughly 4,000 lines in
Mesh-Node and 1,200 in Mesh-Voice.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| The `seq` checks refuse honest events in a group with a forger (see the decision below) | Medium | Decide before Task 2; the check is one function there. |
| Criterion 4 misses with Olm's per-message cost | Medium | Task 8 removes the disk cost first; Task 15 measures before Phase 4. |
| Segment rewrites on delete race appends | Low | One writer: the helper's loop owns the store. Task 5 tests a delete between appends. |
| Acks change the `e2ee` service | Low | A flag bit, unset for test notes; Task 9 keeps their tests passing. |
| The process test gets slow | Medium | It waits for state, not the next event, with short record lifetimes, as the earlier ones do. |

## Decision for you (before Task 2)

**A `seq` case D2 still gets wrong.** In a group, each member sends only its
own events (D5), so a reader gets each sender's events on its own stream,
interleaved differently from how others saw them. If a forger pushes `seq`
up by close to 1,000,000, everyone who has seen that event continues from
it, honestly. A reader that gets an honest member's next event *before* the
forger's is more than 1,000,000 ahead, and refuses it for good. So the
forger can make honest messages vanish for some readers. Own-device sync
can cause the same order. Recommended: **hold, don't refuse.** A too-high
event waits, like an edit before its target, until the reader's highest
comes within range (the forger's event arrives), and is dropped after 30
days or past 200 waiting per chat. The counter still can't be run out,
because a waiting event doesn't move the highest. The alternative keeps the
spec as it is and accepts the loss in that case.

## Open questions

The decision above.
