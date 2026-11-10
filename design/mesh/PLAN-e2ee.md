# Plan: `e2ee` (Mesh)

Status: **APPROVED 2026-11-10**, with the spec (all recommended answers;
`vodozemac` 0.11.2 approved). Implements [SPEC-e2ee.md](SPEC-e2ee.md).

The code goes in `GloriousBrendon/Mesh-Node` (the new `mesh-e2ee` crate,
record v2, the helper's side) and Mesh-Voice (Send a test note).

## Overview

Pure pieces first, in a new `mesh-e2ee` crate: the bundle, the envelope and
its seal, 1:1 sessions, sender chains, and the store. Then identity's record
v2 carries the bundle. Then the helper: bundles into records, the `e2ee`
service, test notes and a process test. Then the Mesh-Voice screens. The
module gate needs the homelab seed and the Windows PC.

## Architecture decisions

- **`mesh-e2ee` owns vodozemac.** Nothing outside the crate names a vodozemac
  type. Its surface is bytes, `MeshId`, `DeviceList` and `VerifyingKey`. Like
  identity, it's pure: the caller passes the time, and the only I/O is its
  own store directory.
- **One `E2ee` value per unlocked device** holds the Olm account, the
  directory of bundles seen (per person: newest `DeviceList`, a bundle per
  device, the one-time key reserved for us and the fallback key), and the
  open conversations. The helper owns it on its main loop, as it does the
  keystore.
- **Olm sessions are stored per person** (own devices under our own ID), and
  sender chains per group conversation. A 1:1 conversation is one person, so
  this is the spec's "one file per conversation" (D12); group key shares ride
  the same per-person sessions.
- **Pickles inside our own seal.** vodozemac's pickles are serialized with
  `serde_json` (already in the tree, and vodozemac depends on it) and sealed
  with XChaCha20-Poly1305 under `mesh/v1/e2ee-store`, written atomically. Its
  own pickle encryption isn't used.
- **The envelope payload names the sender's Olm key.** Inside the seal (or on
  a live connection), an Olm payload is `sender curve key [32] | olm
  message`, and a group payload is `sender curve key [32] | chain ID [32] |
  megolm message`. Without a sender outside, the reader needs it to find the
  session. It's never outside a seal (D8).
- **Replays.** Olm refuses a used message key by itself. A first message on
  the fallback key, and a Megolm index, could be opened twice, so the store
  remembers the first-message base keys and the indices used per chain, and
  advances each chain past the indices it has seen (D13).
- **Test groups carry their member list.** A key share's body is the
  conversation ID, the member IDs with a changed time, and the chain key. A
  device takes the newest list from a member who is in it. `e2eeNote` gains
  an optional `group` (the conversation ID from a `meshNote`) so a note can
  go to an existing group with a new member list: that's how the process
  test drops Carol, which `e2eeNote(ids, text)` alone can't say. `spaces`
  replaces this with the Space's signed log.
- **The `e2ee` service frame** is `flags u8 | envelope`, one per frame. Bit
  0 is "waited", which the reader shows as `path`.
- **Waiting notes are encrypted at send time** and queued per target device,
  in memory, until a connection to that device opens.
- **One new dependency**, `vodozemac` 0.11.2 with `default-features =
  false`, pinned exactly.

## Task list

### Phase 1: `mesh-e2ee` (Mesh-Node)

- [ ] **Task 1: The crate and the bundle** (D1, D3)
  - What: `crates/mesh-e2ee` with vodozemac pinned; `bundle.rs`.
    `E2eeBundle::sign(&DeviceKey, curve, ed25519, now)` and
    `open(bytes, &DeviceList)` → the device key and the Olm keys, checked
    against the list.
  - Accept: round trip; a bundle from a revoked device, another root, or
    with a tampered byte is refused; `cargo tree` shows no second copy of
    our crypto crates.
  - Verify: `cargo test -p mesh-e2ee bundle`.
  - Files: `Cargo.toml`, `crates/mesh-e2ee/*`.
  - Size S.
- [ ] **Task 2: The envelope and the seal** (D2, D8, D9, D10)
  - What: `envelope.rs`. The inside (version, conversation, kind, sender
    list seq, body, padding to 256 B / 1 KiB / 4 KiB / 16 KiB steps up to
    64 KiB), the envelope types, `seal_for(device, address, envelope)` and
    `open_sealed`; the 1:1 conversation ID from the pair secret.
  - Accept: round trips; every padded size; too long refused; the wrong
    device can't open a seal; a damaged byte is refused; a sealed blob holds
    no sender key, chain ID or conversation ID (the test searches the
    bytes); fixed vectors in `tests/vectors.rs`.
  - Verify: `cargo test -p mesh-e2ee`.
  - Files: `src/envelope.rs`, `tests/vectors.rs`; `mesh-identity`'s
    `PairKeys` gains the conversation ID.
  - Size M.
- [ ] **Task 3: 1:1 sessions** (D4, D5, D11)
  - What: `session.rs`. The account: up to 200 one-time keys, a fallback key
    replaced weekly, one key reserved per contact. Learning a person's list
    and bundles; `encrypt_direct(peer, kind, body)` → one ciphertext per
    current device (theirs and our other ones); `decrypt`.
  - Accept: a first message from a one-time key and from the fallback key;
    out-of-order delivery; a replay refused (including a fallback first
    message); a one-time key works once; a device dropped from a newer list
    loses its session and is refused (`revoked-device`).
  - Verify: `cargo test -p mesh-e2ee session`.
  - Files: `src/session.rs`, `src/lib.rs`.
  - Size L.
- [ ] **Task 4: Sender chains** (D6, D7, D14)
  - What: `group.rs`. A chain per (conversation, sending device); key shares
    over the 1:1 sessions with the member list (see Architecture);
    `encrypt_group(conversation, members, kind, body)` → one ciphertext and
    the shares each device still lacks; lazy rotation on leave, revoked
    device, 1,000 messages or 7 days; a message kept up to 30 days when its
    share hasn't come, and one `key-request`.
  - Accept: a member added later can't read earlier messages; a removed
    member can't read anything after the next send; both rotation limits; a
    share and its message in either order; a non-member's message refused; a
    replayed index refused. An `--ignored` test times one message for 600
    devices and a 600-share rotation (success criterion 4).
  - Verify: `cargo test -p mesh-e2ee group`;
    `cargo test --release -p mesh-e2ee -- --ignored`.
  - Files: `src/group.rs`.
  - Size L.
- [ ] **Task 5: The store** (D12, D13)
  - What: `store.rs`. `<data>/e2ee/account` and one file per person or
    group, named by a keyed hash, sealed under the data key, written
    atomically; saved before ciphertext is returned and before plaintext is.
  - Accept: state survives a restart; a simulated crash between save and
    send never reuses a message key; a damaged or swapped file is refused.
  - Verify: `cargo test -p mesh-e2ee`.
  - Files: `src/store.rs`.
  - Size M.

### Phase 2: Identity (Mesh-Node)

- [ ] **Task 6: Address record v2** (D4)
  - What: `addr.rs`. `seal_addr` takes an opaque e2ee field (at most
    1 KiB) and writes format 2; `open_addr` reads 1 and 2.
  - Accept: v1 records still open, with no field; v2 carries it; too long
    refused.
  - Verify: `cargo test -p mesh-identity addr`.
  - Files: `crates/mesh-identity/src/addr.rs`.
  - Size S.

### Phase 3: The helper (Mesh-Node)

- [ ] **Task 7: Bundles in records, the `e2ee` service** (D4, D11)
  - What: `e2ee.rs` in the helper. Open or create the store at unlock;
    each record carries the bundle, the contact's reserved one-time key and
    the fallback key; bundles from opened records (and own devices') feed
    the directory; the `e2ee` service reads envelopes; `keys` in `meshNet`.
  - Verify: `cargo test --workspace`; clippy and fmt.
  - Files: `crates/mesh-node/src/{e2ee,routing,helper}.rs`.
  - Size M.
- [ ] **Task 8: Test notes** (spec "Client side")
  - What: `e2eeNote(ids, text[, group])` → 1:1 or a test group;
    `meshNote` on the reader and our own other devices; notes for an
    offline device wait in memory and go when it connects; lock drops them.
    Errors `not-a-friend`, `no-keys`, `too-long`, `locked`.
  - Verify: `cargo test --workspace`.
  - Files: `crates/mesh-node/src/{e2ee,helper}.rs`.
  - Size M.
- [ ] **Task 9: Process test** (spec "Process test")
  - What: `tests/e2ee.rs`, everything the spec lists: a note to a locked
    reader, own-device copies, a reply, a group, device removal, member
    removal, a replay, and nothing identifying in the seed's files or any
    stderr. Release binary growth (success criterion 5) recorded here.
  - Verify: `cargo test -p mesh-node --test e2ee`.
  - Files: `crates/mesh-node/tests/e2ee.rs`, `tests/common/mod.rs`.
  - Size M.

**Checkpoint A (short report):** Mesh-Node done and tested on Windows.

### Phase 4: Mesh-Voice

- [ ] **Task 10: Test notes in Mesh-Voice**
  - What: `meshNote` and `e2eeNote` in `mesh.rs`; Send a test note and the
    notes received on a friend's profile; "secure messages ready" per device
    under Linked devices.
  - Verify: `cargo test -p mesh-app -p mesh-discord`; by hand with two
    windows and a local seed.
  - Files: `crates/mesh-discord/src/mesh.rs`, `crates/mesh-app/src/…`.
  - Size M.

**Checkpoint B (by hand, you):** two windows on Windows with a local seed
(`MESH_DATA` for the second), as social's Checkpoint B.

1. A and B are friends. Close B. On A, send B a test note.
2. Open B. The note shows on B within a few seconds, marked as waited.
3. Reply from B. It shows on A at once.
4. A third window C restored from B's backup shows B's notes too.

**Checkpoint C (module gate, needs the homelab and Windows):** the spec's
two-machine check, in the same sitting as the other modules' gates.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| vodozemac's API doesn't fit a piece of the design | Medium | Task 1 adds it and Tasks 3–4 use it before anything else depends on it. A misfit that changes a decision stops the build for your call. |
| Records grow past the seed's 4 KiB limit | Low | The e2ee field is capped at 1 KiB; a 16-device list is about 2.5 KiB. Task 6 tests the biggest record. |
| A reader has no bundle for a sender's new device | Medium | Records refresh every 30 minutes and own devices sync; an envelope from an unknown device is kept and retried when a bundle arrives. |
| The process test gets slow or flaky | Medium | It waits for state (`social_until`-style), not for the next event. Short record lifetimes as routing's test. |

## Open questions

None.
