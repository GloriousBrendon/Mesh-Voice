# Plan: `routing` (Mesh)

Status: **APPROVED 2026-11-09** ("continue", with the spec's recommended
answers: Q1 friend-of-friend later, Q2 stranger requests in `social`, Q3 one
record per contact). Implements [SPEC-routing.md](SPEC-routing.md).

The code goes in `GloriousBrendon/Mesh-Node` (pair derivation, records,
contacts, the seed's record store, the helper's side) and Mesh-Voice
(Contacts on Your ID).

## Overview

Pure pieces first, in identity: the pair secret, the address record, and
contacts in the keystore. Then the seed's record store and its two services.
Then the helper: publishing, dial by ID, the contact commands, and a process
test with a seed and two helpers. Then the Mesh-Voice screens. The module
gate needs the homelab seed (seed Checkpoint B) and the Windows PC.

## Architecture decisions

- **Identity owns the cryptography.** `pair.rs` and `addr.rs` are pure and
  unit-tested. The node crate never touches keys directly: it asks the
  keystore for a contact's `PairKeys`.
- **The seed store copies `codes.rs`'s shape**: pure, the caller passes time
  and who's asking, saves when `dirty`, refuses a damaged file.
- **Batched streams**: `addr-put` and `addr-get` read frames until the
  sender finishes, and answer each one. The code services stay one frame
  each, unchanged.
- **The routing task lives in the helper**, next to the code-holding retry,
  on the throwaway seed node (seed D7b).
- **No new crate dependencies** (spiked 2026-11-09).

## Task list

### Phase 1: Identity (Mesh-Node)

- [x] **Task 1: Pair keys** (D1, D2)
  - What: `pair.rs`. `PairKeys::new(&RootKey, &MeshId)` gives the lookup ID
    for (sender, day), the slot for a device key, and the record key.
  - Accept: both sides derive the same keys; A→B ≠ B→A; the day changes the
    lookup; a third key gets different keys.
  - Verify: `cargo test -p mesh-identity pair`.
  - Size XS.
  - Result (2026-11-09): `RootKey::diffie_hellman` (refuses an all-zero
    result) and `pair.rs`; 2 tests. `DeviceKey::sign` added for Task 2.
- [x] **Task 2: The address record** (D3)
  - What: `addr.rs`. `AddrRecord::seal(keys, &DeviceKey, &DeviceList,
    relay, now, ttl)` → bytes; `open(keys, bytes, now)` → the device key,
    relay, created time and the sender's verified `DeviceList`.
  - Accept: round trip; tampering, the wrong pair, expiry, a device not in
    the list and a list from another root are all refused.
  - Verify: `cargo test -p mesh-identity addr`.
  - Size S.
  - Result (2026-11-09): `addr.rs`, fixed big-endian layout like the
    device records; 4 tests, including a removed device whose newer list
    rides in its own record.
- [x] **Task 3: Contacts in the keystore** (D7)
  - What: `contacts.rs` (`Contact { id, name, added, seen, last }`); the
    keystore's contents gain the list, in format v2, which still reads v1.
    `add`, `remove`, `rename`, `mark_seen`, `pair_keys(id)`.
  - Accept: v1 opens with no contacts; contacts survive unlock, password
    change, export and import; adding yourself is refused.
  - Verify: `cargo test -p mesh-identity`.
  - Size M.
  - Result (2026-11-09): no format bump was needed. Contacts follow the
    contents as a second postcard value, only when there are any, so a
    keystore without contacts is byte for byte the old format, and older
    Mesh still opens a new one (postcard ignores trailing bytes). Backups
    carry them. Each contact keeps the newest device list seen, so an old
    list can't bring a removed device back. 49 identity tests pass.

### Phase 2: The seed (Mesh-Node)

- [x] **Task 4: The record store** (D6)
  - What: `addrs.rs`: put (newer `created` replaces older), get (up to 8
    slots), the limits, expiry sweep, save and load, refuse a damaged file.
  - Accept: unit tests for every answer and limit, restart, damaged file.
  - Verify: `cargo test -p mesh-node addrs`.
  - Size M.
  - Result (2026-11-09): `addrs.rs` reuses `codes.rs`'s rate window. The
    seed can't read `created`, so "newer" is a later expiry (every client
    uses the same lifetime). 6 tests.
- [x] **Task 5: `addr-put` and `addr-get` on the seed** (D5)
  - What: the two batched services in `seed.rs`, saved every 5 minutes
    and on stop, counts in the hourly line.
  - Verify: `cargo test -p mesh-node` (covered by Task 7's process test).
  - Size S.
  - Result (2026-11-09): batched streams; the client sends and reads at
    once (`seed::ask_many`) so a big batch can't stall on flow control.

### Phase 3: The helper (Mesh-Node)

- [x] **Task 6: Publishing and dial by ID** (D4, D8, D9)
  - What: `routing.rs`: publish every contact's record to every seed on
    unlock, every 30 minutes, when the relay changes, and when a contact is
    added; back off on failure; `published` in `meshNet`. `dial(id)`
    tries an open connection, then the cache, then seed records in
    parallel. The helper commands `contactAdd/Remove/Rename/List` →
    `meshContacts`, and `netPingId`.
  - Verify: `cargo test --workspace`; clippy and fmt.
  - Size M.
  - Result (2026-11-09): `routing.rs`, plus the D9 change you approved:
    transport's `Config::allow` is checked in the handshake before the
    listener shows its certificate, and `Node::disconnect` closes a removed
    contact's connections. Ticket pings (`netPing`) now need the two to be
    contacts too; `tests/protocol.rs` checks the refusal. `--record-ttl`
    (4 s to 2 h) shortens records for tests. A failed publish retries after
    1 minute, doubling to 30.
- [x] **Task 7: Process test** (spec "Process test")
  - What: `tests/routing.rs`: a seed and two helpers. They ping by ID; a
    one-sided contact is `unreachable`; after a removal and a short expiry,
    `unreachable`; after a restart, the new address is found; no ID or key
    in the seed's file or stderr.
  - Verify: `cargo test -p mesh-node --test routing`.
  - Size M.
  - Result (2026-11-09): passes in about 40 s, all as listed. A removed
    contact is refused at once, cached address or not. The seed's file
    holds no ID and no relay URL; its log names only its own relay (its
    ticket). 99 workspace tests pass; clippy and fmt clean. Idle CPU with
    50 contacts (success criterion 5) isn't measured yet.
  - Windows (2026-11-10, Mesh-Node `7034db0`): the seed now stops on
    Ctrl-Break, so the process tests stop it cleanly; all 99 tests pass there.

**Checkpoint A (short report):** Mesh-Node done and tested on Linux.

### Phase 4: Mesh-Voice

- [x] **Task 8: Contacts on Your ID** (by-hand check pending)
  - What: `meshContacts` and `published` in `mesh.rs`; a Contacts list with
    Ping and Remove; Add as contact on Check an ID; "findable by contacts"
    on the Network line.
  - Verify: `cargo test -p mesh-app -p mesh-discord`; by hand with two
    windows (`XDG_DATA_HOME=/tmp/meshB`) and a local seed.
  - Size M.
  - Result (2026-11-09): Contacts section (name, short ID, last seen,
    Ping, Remove), "Save them as" + Add as contact under Check an ID's
    safety number, "findable by contacts" on the Network line. Your ID asks
    for `contactList` on unlock. `mesh-node` reads `MESH_SEEDS`
    (space-separated tickets) when there's no `--seed`, since Mesh-Voice
    passes no flags. Smoke run on n0's real relays with a local seed and
    two helpers: cold ping by ID 0.64 s (direct), warm 0.1 ms, refused at
    once after removal. 45 Mesh-Voice tests pass; no new clippy warnings.

**Checkpoint B (by hand, you):** two windows on Linux with a local seed:
add each other, ping by ID.

```
mesh-node seed --data /tmp/mesh-seed        # prints "ticket mesh://node/…"
MESH_SEEDS='<ticket>' mesh                  # window A
MESH_SEEDS='<ticket>' XDG_DATA_HOME=/tmp/meshB mesh   # window B
```

Each window: Your ID → Check an ID (paste the other's ID or friend code)
→ Save them as → Add as contact. Then Ping in Contacts.

**Checkpoint C (module gate, needs the homelab and Windows):** the spec's
two-machine check. It shares a sitting with seed Checkpoint C, transport
Checkpoint C and identity Checkpoint D.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Clock skew between machines | Low | The getter asks for today and yesterday. Records are 2 hours long, and the seed refuses a record expiring more than 3 hours ahead. |
| Many contacts make publishing heavy | Low | One batched stream per seed. The limits are sized for a few hundred contacts. Q3 revisits this if needed. |
| Keystore v2 breaks old keystores | Medium | v1 reads as v2 (test). The write is atomic, as now. |
| The homelab isn't up for the gate | Medium | Everything up to Checkpoint B runs on a local seed. |

## Open questions

None.
