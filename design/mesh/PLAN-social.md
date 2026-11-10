# Plan: `social` (Mesh)

Status: **APPROVED 2026-11-10**, with the spec ("build Phase 1 onward"). Implements
[SPEC-social.md](SPEC-social.md).

The code goes in `GloriousBrendon/Mesh-Node` (requests, the account state,
the seed's request store, the helper's side) and Mesh-Voice (the Friends page).

## Overview

Pure pieces first, in identity: the sealed request, the account state and its
merge, and the keystore keeping them. Then the seed: codes stop counting a use
on lookup, and the request store and its two services. Then the helper:
requests and the lists, own-device sync, presence, and a process test. Then
the Mesh-Voice screens. The module gate needs the homelab seed (seed
Checkpoint B) and the Windows PC.

## Architecture decisions

- **Identity owns the cryptography and the merge.** `knock.rs` and
  `account.rs` are pure and unit-tested. The node crate gets the keys it needs
  from the keystore.
- **The account state is the source; routing's contacts follow it.** The
  keystore keeps its `Contact` list in step with the `outgoing` and `friend`
  entries after every change and every merge, so `routing.rs` is unchanged.
- **Stored as a third postcard value** after the contents and the contacts,
  as routing did. A keystore from routing opens with its contacts as
  `outgoing`.
- **The request store lives beside the code store** on the seed, because a
  put has to spend a use from it. They're saved to separate files.
- **One `social.rs` in the helper** for the background work: collecting
  requests, dialling friends, presence streams and sync. It reports back to
  the main loop, as routing does.
- **No new crate dependencies.**

## Task list

### Phase 1: Identity (Mesh-Node)

- [x] **Task 1: The sealed request** (D4, D5)
  - What: `knock.rs`. `seal_knock(...)` and `open_knock(root, lookup_id,
    bytes, now)` → the sender's ID, device list, name, note and created
    time. `PairKeys::knock_slot(lookup_id)`.
  - Accept: round trip; the wrong owner, a tampered byte, another lookup ID,
    a device not in the list, too old and too new are all refused.
  - Verify: `cargo test -p mesh-identity knock`.
  - Size S.
  - Result (2026-11-10, Mesh-Node `e87913a`): 4 tests. The throwaway key
    is a fresh `RootKey`, so the seal reuses `diffie_hellman`. Requests are
    capped at 4 KiB like address records (a 16-device list is about 4 KiB);
    the spec says so now.
- [x] **Task 2: The account state** (D8, D11, D13, D14)
  - What: `account.rs`. Entries `{id, kind, nick, name, note, verified,
    changed, by}`, the status, `merge`, and pruning tombstones older than 90
    days.
  - Accept: merge is commutative and idempotent; newer wins; ties go to the
    higher device; tombstones beat older adds and expire.
  - Verify: `cargo test -p mesh-identity account`.
  - Size M.
  - Result (2026-11-10, `b208f3b`): 4 tests. A local change always stamps
    later than the entry's last stamp, so a device whose clock is behind
    still wins with its newest change.
- [x] **Task 3: The keystore keeps it** (D7, D8, D10)
  - What: the account state as a third stored value; the methods (request
    arrived, accept, decline, cancel, add by ID, remove, block, unblock,
    rename, verify, status, merge, connected), each keeping `contacts` in
    step; codes stay listed until dropped (D7).
  - Accept: a routing keystore opens with its contacts as `outgoing`; state
    survives unlock and backup; a blocked ID can't become a contact.
  - Verify: `cargo test -p mesh-identity`.
  - Size M.
  - Result (2026-11-10, `1e55a63`): 59 identity unit tests pass (2 new here). Adding
    an ID that had sent you a request accepts it. A nickname may be empty.
    Codes stay listed for 8 days after they expire. All 99 earlier
    workspace tests still pass.

### Phase 2: The seed (Mesh-Node)

- [x] **Task 4: Codes and the request store** (D1, D3, D6)
  - What: `codes.rs`: `get` stops counting a use, and `spend` uses one for a
    request. `knocks.rs`: put (needs a live code), get (needs its revoke),
    the limits, sweep, save and load.
  - Accept: unit tests for every answer and limit, a single-use code taking
    one request, requests outliving the code, restart, damaged file.
  - Verify: `cargo test -p mesh-node codes knocks`.
  - Size M.
  - Result (2026-11-10, Mesh-Node `928f066`, with Task 5): 6 new store
    tests; the code tests moved from counting lookups to `spend`.
    `CodeRevoke::tag` compares in constant time.
- [x] **Task 5: `knock-put` and `knock-get` on the seed**
  - What: the two batched services in `seed.rs`, `knocks.bin` saved every 5
    minutes and on stop, counts in the hourly line. `tests/seed.rs` follows
    the change to lookups.
  - Verify: `cargo test -p mesh-node`.
  - Size S.
  - Result (2026-11-10, `928f066`): the batched services share one handler.
    `tests/seed.rs` checks that a lookup no longer uses a code up, that a
    request does, and that only the owner can read it.

### Phase 3: The helper (Mesh-Node)

- [x] **Task 6: Requests and the lists** (D1, D2, D7–D10)
  - What: the `friend*`, `block`, `unblock` and `socialList` commands →
    `meshSocial`, replacing `contact*`; collecting requests at unlock and
    every 5 minutes; dialling friends and `outgoing` contacts at unlock and
    refresh; a connection with an `outgoing` contact makes them a friend.
  - Verify: `cargo test --workspace` (`tests/protocol.rs` and
    `tests/routing.rs` move to the new commands); clippy and fmt.
  - Size M.
  - Result (2026-11-10, `c51fadd`):
    - `meshSocial` carries `fn`, the command it answers, or null when
      something changed on its own (a request came in, someone connected).
      The test driver sets the unprompted ones aside, the same way it does
      `meshNet`.
    - A successful `netPing` by ticket counts as a connection too.
    - Found and fixed: routing's dial never tried an already open
      connection first (routing D8 step 1). It does now, through
      `Node::connected(id)`.
- [x] **Task 7: Own-device sync** (D11)
  - What: records under the self pair; finding and dialling your other
    devices; the `sync` stream at connect, at refresh and on change.
  - Verify: `cargo test --workspace`.
  - Size M.
  - Result (2026-11-10, `72dbfa1`):
    - A device pushes its whole state. The other merges it, and answers with
      its own only if they still differ, so they settle after one exchange.
    - Only our own ID may open `sync`.
    - The state has to fit in one 1 MiB frame, about 10,000 entries. This
      is marked in the code for when lists get that long.
- [x] **Task 8: Presence and status** (D12, D13)
  - What: the `presence` stream; `statusSet` and `activity`; each friend's
    best device; offline after 150 s of silence or when their connections
    close.
  - Verify: `cargo test --workspace`.
  - Size M.
  - Result (2026-11-10, `99efab2`):
    - One stream from each side per friend device: a byte on change, and a
      heartbeat every 60 s.
    - Found in testing: the first presence frame could arrive before our own
      dial's answer and be dropped, so a friend showed only after the next
      heartbeat (60 s). Presence now counts as a connection, and it shows
      at once.
- [x] **Task 9: Process test** (spec "Process test")
  - What: `tests/social.rs`, everything the spec lists.
  - Verify: `cargo test -p mesh-node --test social`.
  - Size M.
  - Result (2026-11-10, built alongside Tasks 6–8): 3 tests, about 30 s
    together. They cover everything the spec lists:
    - a request while the owner is locked, and a spent code refused;
    - accept, then ping by ID;
    - silent decline, and a blocked sender's next request never showing;
    - add by ID on both sides;
    - a backup-made second device syncing both ways;
    - DND, idle, invisible-but-reachable, and offline on lock;
    - nothing identifying in the seed's three files or its log.

**Checkpoint A (2026-11-10):** Mesh-Node done and tested on Linux. 118
workspace tests pass; clippy and fmt are clean. Not measured yet: idle CPU
with 50 friends (success criterion 4).

### Phase 4: Mesh-Voice

- [x] **Task 10: `meshSocial` in Mesh-Voice** (by-hand check pending)
  - What: `mesh.rs` events and commands; the Friends page for Mesh (Online,
    All, Pending, Blocked); Add Friend's Mesh mode (code → request, ID → add
    by ID); the status menu; the Contacts list on Your ID goes.
  - Verify: `cargo test -p mesh-app -p mesh-discord`; by hand with two
    windows and a local seed.
  - Size L.
  - Result (2026-11-10): built on **Your ID**, not as a separate Friends
    page. Mesh-Voice has no Friends page (Online / All / Pending / Blocked)
    for any network, only the Matrix/Discord Add Friend dialog, so the spec's
    "as for the other networks" had nothing to join. Whether to build a
    Discord-style Friends page in the shell is your call.
    - A Friends section replaces Contacts:
      - your status (Online, Idle, Do not disturb, Invisible);
      - requests, with the sender's name and note: Accept, Decline, Block;
      - friends, with presence, verified and last seen: Ping, Remove,
        Block;
      - the ones you asked: Cancel;
      - blocked: Unblock.
    - Check an ID with a **code** offers a note and Send friend request.
      With an **ID** it offers Add by ID, saying they need to add yours
      too. For a friend it offers Mark as verified.
    - **Not wired:** auto-idle after 10 minutes without input. Mesh-Voice
      doesn't track input yet; the helper's `activity` command is ready for
      it.
    - 45 Mesh-Voice tests pass; no new clippy warnings (53 before and
      after).

**Checkpoint B (by hand, you):** two windows on Linux with a local seed.
The app runs the `mesh-node` next to its own binary, so build both and put
`mesh-node` beside `mesh-app`:

```
cd ~/Documents/GitHub/Mesh-Node  && cargo build --release
cd ~/Documents/GitHub/Mesh-Voice && cargo build --release -p mesh-app
cp ~/Documents/GitHub/Mesh-Node/target/release/mesh-node target/release/
~/Documents/GitHub/Mesh-Node/target/release/mesh-node seed --data /tmp/mesh-seed   # prints "ticket mesh://node/…"
MESH_SEEDS='<ticket>' target/release/mesh-app                          # window A
MESH_SEEDS='<ticket>' XDG_DATA_HOME=/tmp/meshB target/release/mesh-app # window B
```

1. On B, under Your ID → Friend codes, make a code. Close window B.
2. On A, paste the code into Check an ID. Compare the safety number, add a
   note, then Send friend request.
3. Open B again. The request is under Friends, with A's name and note.
   Accept it. Within a few seconds both show the other as Online.
4. Change status on B: A shows Do not disturb, Idle, then Offline for
   Invisible. Ping still works while B is invisible.
5. Block A on B, then Unblock. Remove the friend.

**Checkpoint B result (2026-11-10, on Windows instead of Linux):** passed.
Two windows (the second with `MESH_DATA`, added for this) and a local seed:
request through a single-use code, accept, both Online, Ping direct in about
1 ms, DND, Idle, Invisible shown as Offline with Ping still answered, block
and unblock (which leaves them removed, as designed). Fixed on the way:
whoever sent the request saw the other as a bare ID (Mesh-Node `d8fe7ff`:
the username now rides in the presence frame); Block cut off in a narrow
panel and a `MESH_DATA` window sharing the first one's settings (Mesh-Voice
`5b60b03`).

**Checkpoint C (module gate, needs the homelab and Windows):** the spec's
two-machine check, in the same sitting as the other modules' gates.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Changing `code-get` breaks Check an ID or the seed tests | Low | Task 4 changes the store and its tests together; Task 5 updates `tests/seed.rs`. |
| Sync merges flap the contacts list, so routing republishes often | Low | The contacts follow the merged state only when it changes; publishing is already batched. |
| A presence stream per friend hits the transport's stream limit | Low | One stream each way per friend connection, well under the 32 streams allowed per connection. |
| The Friends page in Mesh-Voice is shaped around Matrix | Medium | Turned out there isn't one (Task 10). The Mesh lists are on Your ID until you decide on a shell page. |

## Open questions

None.
