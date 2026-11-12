# Plan: `mailbox` (Mesh)

Status: **APPROVED 2026-11-12** with [SPEC-mailbox.md](SPEC-mailbox.md)
revision 3. *Edited 2026-11-12 after approval, at your request: dropped and
unreachable holders (Task 12, tests 19 and 20) and format numbers (Tasks 1,
3, 6, 11). Checkpoint A is a line in the report, not a stop, unless test 1
fails or misses its 5 s; stops for review after Task 5, after Task 10 and at
Checkpoint B.*

The code goes in `GloriousBrendon/Mesh-Node` (the seed's mail store and
services, the helper's mail path, the mail wrapper in `mesh-e2ee`, the clock
flag) and Mesh-Voice (the checkpoint test only: no screen changes).

## Overview

First the thinnest path that makes acceptance test 1 pass end to end: a
seed that holds one put and hands it back, a helper that mails when nobody
is connected and collects at unlock, and an `ok` receipt. Then the seed
side done properly (claims, storage, limits, mail records), then the helper
side done properly, one spec decision per task, each with the acceptance
tests it closes. Then measurement, the app's test, and last, as its own
step and only after you say so, mail on at the home seed. Hard stops at
each checkpoint.

## Architecture decisions

- **No new crate.** The helper's side is `crates/mesh-node/src/mail.rs`, a
  child module of the helper as `dm.rs` is; the seed's side is
  `mailstore.rs` beside `addrs.rs`. Box, slot, dummy, collect-secret and
  pair-tag derivations are pure functions in `mail.rs`, unit-tested there.
  The wrapper (M7) goes in `mesh-e2ee`'s `envelope.rs` beside `seal_for`,
  as `seal_mail` and `open_mail`: same seal, a longer inside head.
- **Mail records reuse the address store.** `addrs.rs`'s store becomes
  generic over its limits (how far ahead a record may expire, its totals);
  `mrec-put` and `mrec-get` are a second instance of it with mail's limits,
  on disk in `mail/`. Nothing about address records changes.
- **The seed's mail store** (seed D9, as changed): three append-only logs
  under `<data>/mail/`: `claims`, `records`, `envelopes`. Each log's index
  lives in memory (box and slot to claim; lookup and slot to record
  offset; box and slot to envelope offsets and put times); values stay on
  disk. A take or an expiry appends a removal. A log is compacted at start
  and when its dead bytes pass its live ones: written anew, synced,
  renamed, folder synced, as `saved.rs` does. A damaged log is moved aside
  (`.damaged`), as the address file is today. *ponytail: the claim index is
  in memory, about the size of the claims (393 KB a heavy user); fine for
  the beta, measured in Task 16, revisited before a public release.*
- **Sending holds on unlock until mail is collected.** At unlock the
  helper's first seed request is the collect; sends to newly opened
  connections, and the live backlog, wait for it to finish (or
  `HOLDER_WAIT`). That's what keeps test 1's "not mailed again" and "not
  seen twice" true without anything else.
- **One clock.** `mesh_identity::unix_now()` is the clock everything
  already reads. `--clock-offset <secs>` on the helper and the seed sets a
  process-wide offset it adds, once, at start. Test-only, as
  `--record-ttl` is: refused above 60 days.
- **`--cut <seed|live>`**, test-only, on the helper: `seed` drops its seed
  connections and refuses to open new ones until a `cutOff` command clears
  it; `live` does the same for friends' devices. Tests 11, 18 and 19 need
  it (approved 2026-11-12).
- **Choosing your own holder:** `--holder <ticket>` (repeatable, up to 3)
  and `MESH_HOLDERS`, read in `main.rs` exactly as `--seed` and
  `MESH_SEEDS` are, and listed in the device's mail record. Mesh-Voice
  passes the environment through already, so someone running their own
  seed sets `MESH_HOLDERS` before starting Mesh. **No screen in v1:** the
  only person who runs a seed in the beta is you; a screen means a ticket
  field, checking it, saving it per device and syncing it between your
  devices, all for one user. When friends' seeds are real, a screen is a
  small task: the helper side is the same.
- **No new dependencies.** `blake3` (`keyed_hash`, `derive_key`),
  `chacha20poly1305`, `postcard` and `vodozemac` are in the workspace; the
  logs are `std::fs`.

## Task list

Tests first in every task: the named acceptance tests (in a new
`tests/mailbox.rs`, with `tests/common`'s `Helper`) and the unit tests are
written and seen to fail before the code. One commit per task. Each task
passes `cargo test --workspace`, `cargo clippy --workspace --all-targets --
-D warnings` and `cargo fmt --all --check`.

### Phase 1: the thin path (Mesh-Node)

- [ ] **Task 1: Acceptance test 1, end to end, as thin as it can be**
  - What, the least that makes test 1 pass:
    - Seed: `--mail` turns on `mail-put`, `mail-get` and `mail-take`, held
      **in memory**, by box and slot, no claims, no limits. *ponytail: any
      put into any box is taken; Tasks 2 to 4 close that. `--mail` is off
      by default, so no seed takes mail before then.*
    - Helper: after storing an event, and at lock (with `MAIL_FLUSH`), each
      event still waiting for a device with no open connection is
      encrypted on the existing session, sealed with the mail wrapper
      (format 1, sender device, kind, `mseq`; no pair tag yet: Task 8 adds
      it before anything ships, so the format stays 1) and put into this
      week's box on the first `--seed` seed. At unlock, collect this week,
      open, store, take, and put an `ok` receipt into the pair's box; the
      sender, collecting it at its unlock, moves its counter.
    - Collect-first at unlock (above).
  - Test 1 runs with a session already open between Alice and Bob (they've
    written once, live). Without one, Alice needs Bob's mail record: that's
    Task 6 and test 17.
  - Accept: test 1 (the 5 s timed from the helper's `online` line); the dm
    process test still passes; a wrapper with a newer format is refused
    (unit).
  - Size M.

**Checkpoint A (a line in the report, not a stop, unless test 1 fails or
misses its 5 s):** test 1 passes; the 5 s measured on loopback.

### Phase 2: the seed (Mesh-Node)

- [ ] **Task 2: Claims, slots and the collect secret** (M6)
  - What: `mail-claim` (claim or renew `(box, slot, collect tag,
    expiry)`); `mail-put` only into a claimed slot (`no-box` otherwise);
    `mail-get` and `mail-take` only with the collect secret that hashes to
    the tag. Still in memory.
  - Accept: unit tests over the store; test 6's first part (a stranger's
    put answers `no-box`); test 1 still passes (the helper claims this week
    at unlock, thinly, until Task 7).
  - Size M.
- [ ] **Task 3: Storage and expiry** (seed D9 as changed; test 7)
  - What: the three logs, each opening with its format number (1),
    indexes, compaction, `.damaged`; the hourly
    sweep (envelopes after `MAIL_LIFE`, claims after `CLAIM_LIFE`);
    `--clock-offset` on the seed.
  - Accept: restart keeps claims and mail; a crash mid-append keeps
    everything before it; a damaged log is moved aside and the seed starts;
    compaction leaves the same contents; a log with a newer format stops
    the seed with an error rather than being moved aside; test 7.
  - Size L.
- [ ] **Task 4: Limits, `full` and closed seeds** (M14, M18; test 6)
  - What: the seed rates in *Limits*, per throwaway key, network and relay,
    as `addrs.rs` counts them; per-box `BOX_MAX_BYTES` and
    `BOX_MAX_ENVELOPES`; `MAIL_STORE_MAX` (`--mail-max`) over all three
    logs; `--mail-token` and `&mail=` in the seed's ticket.
  - Accept: test 6 in full; each limit at its edge in unit tests.
  - Size M.
- [ ] **Task 5: Mail records on the seed** (routing D3, D5, D6 as changed)
  - What: `addrs.rs`'s store generic over its limits; `mrec-put` and
    `mrec-get` as a second instance (7 days ahead, its own rates, counted
    in `MAIL_STORE_MAX`), in `mail/records`.
  - Accept: the address-record tests pass unchanged; a mail record 7 days
    ahead is taken and one 7 days and a minute is refused; it sweeps.
  - Size S.

**Stop for review after Task 5: the seed done.**

### Phase 3: the helper (Mesh-Node)

- [ ] **Task 6: The mail record, read ahead and remembered** (M11; tests 9,
  10, 17)
  - What: put at unlock, daily and on change; read every contact's at
    unlock and daily (this week, then last week for misses); the
    remembered records' sealed file (format 1, with the dropped holders of
    Task 12); mail from the newest record, read or
    remembered, starting a session from its fallback key when there's
    none; `--clock-offset` on the helper.
  - Accept: tests 9, 10 and 17; a read that finds nothing keeps the memory
    (unit); a remembered-records file with a newer format is refused and
    left on disk (unit).
  - Size L.
- [ ] **Task 7: Claims from the helper** (M6, M12, M13; test 5)
  - What: claim this week and `CLAIM_WEEKS_AHEAD` for every contact and the
    self pair, padded to `CLAIM_BUCKET` with dummies; renew daily; collect
    real and dummy boxes alike; on `no-box`, claim again and move `since`;
    a sender that reads a later `since` mails again what it put there
    without an `ok`.
  - Accept: test 5; unit tests that a seed's view of claims, renewals and
    collects is the same with 70 contacts as with 127.
  - Size M.
- [ ] **Task 8: The wrapper's tag and receipts** (M7, M9; test 11)
  - What: the pair tag in `seal_mail`/`open_mail`; wrappers that fail it
    dropped; receipts with `ok`, `failed` and couldn't-open, sealed with no
    Olm; `failed` mailed again at once, then on `FAILED_BACKOFF`.
  - Accept: test 11 (with `--cut seed`, Task 14's switch, brought forward
    here); unit tests for a wrong tag, a non-contact's device, and the
    backoff.
  - Size M.
- [ ] **Task 9: Opening order** (M8; tests 12, 13, 16)
  - What: Olm in `mseq` order per sender device; the opened events handed
    to `History::receive` in D2's order across senders (for each chat,
    sorted by (`seq`, event ID), then given in runs of one sender, since
    `receive` takes one sender); "not a member yet" once more at the end of
    the batch, then `failed`; mail that can't be opened dropped and listed.
  - Accept: tests 12, 13 and 16; the dm process test still passes (live is
    unchanged).
  - Size M.
- [ ] **Task 10: A fresh session by receipt** (M16; test 14)
  - What: on couldn't-open, ignore what has an `ok`; mail the rest again on
    a fresh outbound session; at most one per device per
    `FRESH_SESSION_MIN`, never twice from one fallback key.
  - Accept: test 14; unit tests for both limits, and that test 13's
    couldn't-opens start nothing.
  - Size M.
**Stop for review after Task 10.**

- [ ] **Task 11: The saved marks** (M15; test 15)
  - What: per event and reader device, holder, put time and `mseq`, in the
    counters file, written once per batch of puts; read at unlock. The
    counters file goes to format 2; format 1 still opens, with no marks.
  - Accept: test 15; a crash between a put and the save mails it once more
    at most (unit); a format 1 counters file written by today's code opens
    with every counter as it was (unit, from saved bytes).
  - Size S.
- [ ] **Task 12: Holders: full, dropped and unreachable** (M3, M10, M12;
  tests 4, 19, 20)
  - What: `--holder` and `MESH_HOLDERS`; the record's holder list (default
    the built-in seeds); an unreachable holder (refused or failed
    connection, unknown service) skipped at once; the next holder on
    `full`, `no-box`, `slow-down` or `HOLDER_WAIT` after connecting;
    nothing taken means the event waits (D5). A record no longer listing
    the holder put to: mail again at once to the holders listed now. A
    dropped holder kept, and collected from, until its claims end.
  - Accept: tests 4, 19 and 20; a reader listing two holders, the first
    full, gets its mail from the second.
  - Size M.
- [ ] **Task 13: Groups and own devices** (spec delivers 2 and 3; dm D8 as
  changed; tests 2, 3)
  - What: group events mailed per member device, as live; own devices get
    by mail only events authored here and read markers.
  - Accept: tests 2 and 3.
  - Size S.
- [ ] **Task 14: Refreshes and the week's edge** (test 18)
  - What: every 30 minutes, collect this week and last; `--cut live`.
  - Accept: test 18.
  - Size S.
- [ ] **Task 15: Nothing readable** (M17, M19; test 8)
  - What: counts-only log lines for mail on the seed and the helper; the
    greps of test 8 over the seed's `mail/` and every helper's stderr.
  - Accept: test 8.
  - Size S.

**Checkpoint B (stop for review):** acceptance tests 1 to 20 pass in `tests/mailbox.rs`;
the dm, routing, social and e2ee process tests still pass.

### Phase 4: measuring (Mesh-Node)

- [ ] **Task 16: Unlock collect time and seed disk at 1,000 contacts**
  - What: `--ignored` tests, release build, run three times:
    1. A helper with 1,000 contacts (made in the test, no network for
       them) unlocks against a seed holding its 6,144 claims and one
       envelope: time from `online` to the envelope stored, on loopback,
       and the bytes up and down.
    2. A seed filled as 100 people of 1,000 contacts and 1,000 of 100 would
       fill it (claims and records for a week, mail at 10 envelopes a
       person a day): disk per log, memory held, start time (compaction),
       and a sweep's time.
  - Accept: results recorded here, against the spec's table (about 1 s
    more than test 1 at 1,000 contacts; about 2.6 MB a person on disk).
  - Size S.

### Phase 5: Mesh-Voice

- [ ] **Task 17: The checkpoint test gains test 1 through the app.** In
  `crates/mesh-app/src/checkpoint.rs`: B locks, A sends and locks, B
  unlocks, the line shows on B, and A's "sending" mark clears when A
  unlocks. No screen changes. Size S.

**Checkpoint C (module gate, needs you; goes in CAPABILITY-MAP's *Before
beta* list):** the spec's test 1 between the Windows PC and the Linux box
through a seed with mail on. Across networks it needs the VPS seed, like
every other module's gate.

### Phase 6: the home seed (asks you first)

- [ ] **Task 18: Mail on at the home seed. I stop and ask before this.**
  - What, once you say so: the homelab's compose file gains `--mail
    --mail-max 2147483648` for the seed container; the image is rebuilt
    and the container restarted; then I read the next hourly lines (claims,
    puts, collects, bytes held: counts only).
  - Until then, the home seed runs without `--mail` and refuses every mail
    service.
  - Size S.

## Order and size

Task 1 first; Tasks 2 to 5 in order; Task 6 before 7 to 15, which can go in
any order except that Task 8 brings in Task 14's `--cut seed`, and Task 10
follows Task 8; Task 16 after Checkpoint B; Task 17 after Task 16; Task 18
only on your word. Roughly 3,000 lines in Mesh-Node and 60 in Mesh-Voice.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| A moved clock breaks something that isn't mail (records, certificates, the relay) | Medium | Every helper and the seed in a test move together; iroh's handshake uses raw keys with no certificate times. Task 6 runs the routing process test with an offset once to see. |
| The seed's memory grows with claims | Low for the beta | Task 16 measures; the index is a known shortcut (*ponytail* above). |
| Unlock gets slower at 1,000 contacts | Medium | Collect goes first and alone; the record read and puts go after it. Task 16 measures. |
| Mail opened in D2 order misses an Olm order case | Low | Olm order and dm order are separate steps (spec *Opening mail* 2 and 4); test 13 covers the Olm side, 16 the dm side. |
| The process tests get slow | Medium | They wait for state, not time; the clock offset replaces every long wait. |

## Needs a new dependency

None.

## Open questions

None. (The pair tag and `--cut`: yes, 2026-11-12.)
