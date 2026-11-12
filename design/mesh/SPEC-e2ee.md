# Spec: `e2ee` (Mesh)

Status: **APPROVED 2026-11-10** with the recommended answers: vodozemac
(Olm and Megolm), not MLS (Q1); per-device sessions for 1:1 and sender chains
for groups (Q2); post-quantum later (Q3); history before joining as a
per-Space setting, on by default, with the final call in `spaces` (Q4);
backups without message sessions (Q5); encrypted on live connections too
(Q6). `vodozemac` 0.11.2 is approved. Plan: [PLAN-e2ee.md](PLAN-e2ee.md).
Module 6 of the build order in [CAPABILITY-MAP.md](CAPABILITY-MAP.md).
Depends on [`identity`](SPEC-identity.md) (built). It also uses
[`routing`](SPEC-routing.md) records (built; Checkpoint B by hand pending) and
[`social`](SPEC-social.md) friends (built; Checkpoint B passed 2026-11-10).
`dm`, `mailbox`, `spaces` and `calls` build on it.

## Objective

Make every message readable only by the devices it was meant for, wherever
it travels: straight over a connection, held by a friend or a seed, or passed
on by another Space member. Losing a device key later must not unlock old
messages (forward secrecy). Whoever holds mail must not learn who sent it
(sealed sender).

Today only the connection is encrypted. QUIC between two device keys
(transport D5) already hides traffic from relays, with fresh keys per
connection. That covers two online devices talking directly. It does not
cover a message that has to wait somewhere, or reach a device that wasn't
connected, or go to a group. After this module, each device has message
keys, sessions with the devices it talks to, and one envelope format that
`dm`, `mailbox`, `spaces` and `calls` use for everything they send.

**What this module delivers**

1. **Message keys per device**, chained to your ID: root → device
   certificate → a signed e2ee bundle. Checking the safety number once covers
   all of a person's devices, now and later.
2. **First messages to someone offline.** Each device puts its bundle and a
   one-time key into the address record it already leaves for each contact
   (routing D3). Nobody but that contact can open it.
3. **1:1 sessions**: a double-ratchet session between each pair of devices
   (yours, theirs, and your own other devices).
4. **Group and Space sessions**: one sender key chain per sending device per
   conversation, shared with every member device over the 1:1 sessions, and
   replaced when someone leaves.
5. **The envelope**: one format for every path, padded, and sealed so that
   anyone holding mail sees neither sender nor reader.
6. **A secret for calls**: a way to hand a key to a set of devices, which
   `calls` turns into SFrame keys.
7. **A by-hand test**: a "test note" sent to a friend or a small test group,
   delivered when they're next online. That's the two-machine check.

**Not in v1 of this module**

- Chat itself: history, edits, reactions, read state. That's `dm`.
- Mail held by friends or seeds. That's `mailbox`. Here a note waits on the
  sender's device until the reader connects, in memory only.
- Post-quantum key exchange (Q3).
- Spaces of thousands. The group design is sized for a few hundred members
  (the map). MLS is the path for more (Q1).
- Encrypted backups of message keys. A restored device starts fresh (Q5).

**User stories**

- I send a friend a test note while they're offline. When they open Mesh, it
  arrives, readable, on their desktop and their laptop.
- My note also shows on my own laptop, which wasn't involved in sending.
- I remove my lost laptop. Nothing sent after that reaches it, in 1:1 chats
  or in groups, as long as whoever has it can't open its keystore. *Every
  device holds the root secret (identity D2): a removed device which is
  still unlocked can sign itself a new device key and get back in.*
- Someone copies the encrypted mail a seed holds and later steals my
  device. They still can't read the old mail.
- Someone copies every byte a seed holds for me. They see random IDs and
  same-sized blobs. They can't tell who sent them, or whether two blobs came
  from the same person. (Whoever runs the seed can see which sessions put
  and fetched them: seed D10b.)
- I compare the safety number with a friend once. When they add a new
  device, I don't have to check again.

### Decisions this spec makes

| # | Decision | Why |
|---|---|---|
| D1 | **Signal-style sessions through `vodozemac` 0.11.2** (Olm: X3DH-style setup plus the double ratchet; Megolm: sender key chains for groups), with default features off. Not MLS. See *Options compared* and Q1. | It is the design Matrix uses for exactly our case: many devices per person, no central server, members offline for days, no global order of messages. It was audited (Least Authority, 2022). It is built on the same crypto crates Mesh-Node already uses (`curve25519-dalek` 5, `ed25519-dalek` 3, `sha2` 0.11, `chacha20poly1305` 0.11), so it adds about 15 small crates and no second copy of any of them. MLS needs someone to put every group change in one order, and Mesh has nobody whose job that is. |
| D2 | **One envelope on every path.** A message is encrypted the same way whether it goes straight over a connection, waits in a mailbox or is passed on by a member. | No "was this relayed?" branch to get wrong. A ratchet that moves forward on every message, whatever the path, stays in step. Encrypting twice on a live connection costs microseconds. |
| D3 | **Each device has one Olm account.** Its keys are signed by the device key in an `E2eeBundle`: version, device key, Olm identity key, Olm signing key, created time, under domain `mesh/v1/e2ee-bundle`. A bundle counts only if its device is current in the person's newest `DeviceList`. | The chain root → device cert → bundle means the safety number (identity D7), which is built from root IDs, vouches for every device. Matrix needed cross-signing to get this. Mesh gets it from identity D1. Olm keys are separate from the device key because vodozemac manages its own keys, and the device key stays a transport key. |
| D4 | **Bundles and one-time keys travel in address records.** Each (device, contact) record (routing D3) gains a field: the device's `E2eeBundle` and one one-time key reserved for that contact, plus the device's current fallback key. **Only the contact's first device** (the first in their device list) uses the one-time key; their other devices start on the fallback key. A first message that still can't be opened (another device used its one-time key) is answered, at most once an hour per sending device, with a fresh session back, and the sender resends what wasn't acknowledged (dm). Your own devices get theirs through the self pair (social D11). *This changes routing D3: address record format v2. The field is opaque to `mesh-identity`, at most 256 bytes: the seed's 4,096-byte record limit leaves about 300 after the rest. Changed 2026-11-11 (audit finding 1): a one-time key per reading device doesn't fit.* | No new seed service, and nothing new the seed can see. A one-time key in a record only one contact can open can't be used up by strangers, which is the classic prekey-draining attack. A used one-time key is replaced at the next refresh (30 minutes). The fallback key covers that gap, as Signal's last-resort key does. |
| D5 | **1:1 conversations use pairwise sessions.** A message goes once to each of the other person's current devices and each of your own other devices, each over its own Olm session. At most 2 × 16 devices (identity's `MAX_DEVICES`). | The double ratchet heals after a key leak (post-compromise security) on every reply. Fan-out to a handful of devices is cheap. Your own devices see what you sent, so `dm` doesn't need a separate copy-to-self path. |
| D6 | **Groups and Spaces use sender key chains.** *Changed 2026-11-11 (dm, your call): group DMs don't. dm sends each device what it hasn't acknowledged (dm D5), one device at a time, and a chain encrypts for every member device at once; so group-DM events go over the 1:1 sessions, device by device, with the group named inside. Every group-DM member is a friend of every other (dm Q1), so those sessions exist, and a group DM has at most 10 people. dm checks membership itself (dm D7). Spaces keep sender chains.* Each sending device has one Megolm session per conversation. Its key is sent to every current member device over the 1:1 sessions before (or with) the first message that uses it. A message is encrypted once and fanned out. A new member or device gets the chain from the next message on, never earlier ones. | A message to a Space of 300 people with 600 devices is encrypted once, not 600 times. The cost moves to the key shares, which happen only at the start and at each rotation. |
| D7 | **Rotation.** A sending device starts a new chain when a member leaves or is removed, when a member's device is revoked, after 1,000 messages, or after 7 days. It rotates lazily: on its next send, not at the moment of the change. | A removed member must not read anything sent after they left. Lazy rotation means a removal in a busy Space costs each active sender one round of key shares, spread over time, not one storm. 7 days and 1,000 bound how much one leaked chain exposes. These are Matrix's defaults. |
| D8 | **Sealed sender for anything a third party holds.** Mail and messages passed on by others are sealed again to the reader's device: a throwaway key, `X25519(throwaway, reader's device key)` → `blake3::derive_key("mesh/v1/seal-key")` → XChaCha20-Poly1305. The outside shows only the address the carrier needs (`mailbox` defines it, blinded per pair as routing D2), and a padded length. On a live connection there's no outer seal: the connection already says who you are. | It's the request seal (social D4), aimed at a device key instead of a root key. No new dependencies. Without it an Olm first message would show the holder the sender's Olm identity key, and a Megolm message its session ID, both of which link messages to a sender. |
| D9 | **Padding.** Envelope plaintext is padded to 256 B, 1 KiB, 4 KiB, then multiples of 16 KiB, up to 64 KiB. Attachments aren't sent in envelopes (see *What later modules get*). | Length is what's left to see once everything else is sealed. Buckets hide "short text" against "long text" at little cost. |
| D10 | **Conversation IDs.** A 1:1 conversation's ID comes from the pair secret (routing D1, context `mesh/v1/dm-conversation`), so both sides know it without asking. A Space's ID is 16 random bytes chosen by its creator. *A group DM's ID is a hash of its creator's ID and a random value carried in its creation event, checked on receipt, so no one else can make a creation for it (changed 2026-11-11, dm D7).* The ID, the kind and the sender's device list `seq` are inside the encryption, never outside. | Nobody outside can group messages by conversation. Deriving the 1:1 ID means two people can't end up with two different chats with each other. |
| D11 | **Device list rules.** Encrypt only to devices in the newest `DeviceList` seen for each person (from routing records, handshakes or social sync). When a newer list drops a device, its sessions are deleted and D7 rotation is due. A message from a device not in the sender's current list is refused (`revoked-device`). A device that newly appears gets sessions from its bundle. | Revocation is identity's job, and routing already carries the lists. e2ee only has to follow the newest one. |
| D12 | **Session state is stored per conversation**, in `<data>/e2ee/`: one file for the Olm account and one per conversation. Each is encrypted with a key derived from the keystore's data key (`mesh/v1/e2ee-store`, identity D6) and written atomically. File names are a keyed hash of the conversation ID. State is saved **before** a ciphertext leaves the device and **before** a decrypted message is handed up. *2026-11-11 (dm D11): `decrypt_batch` opens many envelopes and saves the state once, before handing any of them up; a failed save hands up nothing.* | A ratchet that rolls back after a crash can reuse a message key. Saving first prevents that. One file per conversation keeps each write small: a 300-member Space is a few hundred KB, not the whole account. |
| D13 | **Keys are deleted once used; history is kept as plaintext by `dm`**, under the data key. A device or member that should see old messages gets them as messages, re-sent by a device that has them, over a current session. Old message keys are never shared. | That's what keeps forward secrecy real: no key for an old message exists after it's read. It matches the map's "history comes from other members". |
| D14 | **Missing keys.** Key shares go in the same delivery as the first message that needs them. A device that still gets a message it can't open keeps it for 30 days and asks the sender's devices once (`key-request`). A device answers only for conversations it sends in, and only to current member devices. | "Unable to decrypt" is Matrix's best-known failure. Most of it comes from key shares and messages taking different routes. Bundling them removes that cause. The request covers the rest. |
| D15 | **Nothing identifying is logged**: no IDs, keys, conversation IDs or message contents, only counts. Errors are fixed text. | Same rule as routing D10 and social D15. |

### Options compared

Crate facts checked on 2026-11-10 with `cargo info` and `cargo tree` in a
scratch project. "New crates" counts crates not already in Mesh-Node's
`Cargo.lock`.

| Option | Crate, licence | New crates | Multi-device | Offline and mailbox | Groups of a few hundred | Maturity |
|---|---|---|---|---|---|---|
| **MLS** | `openmls` 0.9.2, MIT | about 68, including a second `curve25519-dalek` (4), `ed25519-dalek` (2), `sha2` (0.10), `p256` and `aes-gcm` | Each device is a member. Natural | Key packages could ride in records. But every change to a group (add, remove, key update) must be applied in one agreed order, and two members changing the group at once fork it | Best: a removal costs about log₂(n) work | RFC 9420. Audited by SRLabs (one high finding, fixed); several advisories in 2025, fixed |
| **MLS** | `mls-rs` 0.57, Apache-2.0 or MIT (AWS) | about 36, same crypto majors as ours | As above | As above | As above | Used in AWS products; no public audit found |
| **Olm + Megolm** (D1) | `vodozemac` 0.11.2, Apache-2.0 | about 15, same crypto majors as ours | Built for it: Matrix has many devices per person | Built for it: Matrix servers federate with no global order, and members go offline for weeks | Fine: a removal costs one key share per member device, per active sender | Least Authority audit (2022, 10 findings, 8 fixed during the audit). Matrix's production library |
| **Hand-rolled** X3DH + double ratchet + sender keys | none (Mesh-Node's `blake3`, `chacha20poly1305`, X25519 from `ed25519-dalek`) | 0 | Ours to design | Ours to design | As Megolm | Unaudited, ours to get right |

MLS is the better protocol on paper, and the right one for 50,000-member
Spaces. Its weak spot is the one Mesh is built around: there is no server to
decide the order of group changes, and members are offline a lot. A Space
could name its owner as the one who orders changes, but then nobody can be
removed while the owner is offline. Olm and Megolm need no order at all. The
hand-rolled version would be the lightest, but this is the one place where
writing our own is the risk the rest of the design exists to avoid.

## Behaviour

### Keys (D3, D4)

- At unlock, the device opens or creates its Olm account in
  `<data>/e2ee/account`. It keeps up to 200 unused one-time keys and one
  fallback key, replaced weekly.
- Publishing an address record (routing D4) now also takes, for that
  contact: the bundle, one unused one-time key reserved for them, and the
  fallback key. The reservation is kept until the contact uses the key, or
  the next refresh replaces it.
- Opening a contact's record checks the bundle's signature and that its
  device is current. A bad bundle drops the whole record (`AddrError::Bundle`).

### Sending

- **1:1**: for each target device, use the open session, or start one from
  the device's bundle and its one-time key (else the fallback key). Encrypt
  the envelope once per device.
- **Group**: if the sending chain is due for rotation (D7) or missing, start
  a new one and queue its key share for every current member device. Encrypt
  the envelope once with the chain. Each device's delivery carries the key
  share it still lacks (D14).
- Save state (D12), then hand the ciphertexts to the caller with the
  devices they're for. The caller (`dm`, `spaces`, `mailbox`) moves them.

### Receiving

- Open the outer seal if there is one (D8). Decrypt with the session or
  chain. Check the sender device is current (D11), and that the inside's
  conversation ID is one this device is in.
- Refuse a replayed message (the ratchet index is already used), a device
  not in the list, and a group message from someone who isn't a member.
- Save state, then hand up `(conversation, sender ID, sender device,
  kind, body)`.

### The envelope (D2, D8, D9, D10)

Hand-written layout, as every signed or sealed format in Mesh-Node:

```text
envelope = version u8 | type u8 | payload
  type 1 = olm first message, 2 = olm message, 3 = group message
sealed   = address [carrier-defined] | throwaway key [32] | nonce [24]
         | XChaCha20-Poly1305(envelope) + 16 tag
inside   = version u8 | conversation [16] | kind u8 | sender list seq u64
         | body len u32 | body | padding
  kind 1 = message (the body is the caller's), 2 = key share,
       3 = key request, 4 = secret (calls), 5 = test note
```

### Client side (the helper)

- **`e2eeNote(ids, text)`**: one ID sends a 1:1 test note. Several make a
  test group of those IDs (all must be friends) and send to it. A note to
  someone offline waits on this device, in memory, and goes when a
  connection to any of their devices opens. Locking drops it (`mailbox`
  fixes that). Errors: `not-a-friend`, `no-keys` (no bundle seen for them
  yet), `too-long`, `locked`.
- **`meshNote`** on the reader:
  `{"from","device","group":null|[ids],"text","at","path":"live|waited"}`.
  It also arrives on the sender's own other devices.
- **New transport service `e2ee`** carries envelopes, one per frame,
  length-capped at 64 KiB plus the seal.
- `meshNet` gains `"keys": bool`: true when this device's one-time keys
  are in its published records.

### Mesh-Voice

- A friend's profile gains **Send a test note** and a short list of notes
  received from them. `dm` replaces both with the chat view. *(Retired
  2026-11-12, PLAN-dm Task 19: the button and list are gone; the helper
  keeps `e2eeNote` for its tests.)*
- Your ID's Linked devices shows "secure messages ready" per device once its
  bundle is published.

### What later modules get from e2ee

The `mesh-e2ee` crate's surface, which later specs build on:

- **`dm`**: `encrypt_direct(peer, kind, body) → [(device, ciphertext)]`,
  `encrypt_group(conversation, members, kind, body)`, and
  `decrypt(bytes) → Opened`. 1:1 conversation IDs (D10). Own devices are
  always included (D5), so sent messages sync for free. History for a new
  device is `dm` re-sending plaintext over these sessions (D13).
  **Attachments**: a random 32-byte key per file, XChaCha20-Poly1305 in
  64 KiB chunks with the chunk number as associated data. The key and the
  file's hash go in the message. The file itself moves outside envelopes.
- **`mailbox`**: `seal_for(device, envelope) → sealed` and `open_sealed`.
  Mailbox chooses the outer address (blinded per pair, as routing D2) and
  where mail lives. It sees only sealed, padded bytes.
- **`spaces`**: group conversations with up to 500 members (D6), and
  `members_changed(conversation, members)` to trigger rotation (D7). Spaces
  must give e2ee each member's `DeviceList` and bundles, because Space
  members aren't necessarily friends and so don't have each other's routing
  records. The likely way is the Space's signed log carrying each member's
  device list and bundle. That's for the `spaces` spec.
- **`calls`**: `share_secret(conversation, devices, secret)` sends a
  random key to each device as kind 4. `calls` derives SFrame keys from it
  (`mesh/v1/sframe`) and calls it again on every join and leave.

## Tech stack

One new dependency, approved 2026-11-10:

| Crate | For |
|---|---|
| `vodozemac` 0.11.2, Apache-2.0, `default-features = false` | Olm sessions and Megolm chains (D1). MSRV 1.89, below ours (1.95). It brings `aes`, `cbc`, `hmac`, `hkdf`, `hpke`, `prost`, `matrix-pickle`, `base64`, `arrayvec` and a few build-time crates. Everything else it uses is already in the tree. Its licence goes into packaging's third-party notices. |

Sealing (D8) reuses X25519 from `ed25519-dalek`, `blake3` and
`chacha20poly1305`, as social's requests do.

## Commands

```
cargo test --workspace                       # Mesh-Node
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p mesh-app -p mesh-discord       # Mesh-Voice
```

## Project structure

```
Mesh-Node/
  crates/mesh-e2ee/                         new crate: no networking, no I/O but its store
    src/bundle.rs                           E2eeBundle: sign, check against a DeviceList
    src/session.rs                          1:1 sessions over vodozemac Olm
    src/group.rs                            sender chains, key shares, rotation
    src/envelope.rs                         the layout, padding, seal and open
    src/store.rs                            per-conversation files under the data key
    tests/vectors.rs                        fixed vectors for the envelope and seal
  crates/mesh-identity/src/addr.rs          record v2: an opaque e2ee field
  crates/mesh-node/src/e2ee.rs              bundles into records, the e2ee service, notes
  crates/mesh-node/src/helper.rs            e2eeNote, meshNote, the keys flag
  crates/mesh-node/tests/e2ee.rs            a seed, three people, one with two devices
Mesh-Voice/
  crates/mesh-discord/src/mesh.rs           meshNote, e2eeNote
  crates/mesh-app/src/…                     Send a test note on a friend's profile
```

A separate crate keeps `mesh-identity` free of vodozemac, and lets `dm` and
`spaces` depend on e2ee without pulling in the helper.

## Code style

The same as routing and social: pure, unit-tested modules, limits as named
constants, derivation contexts and domains as constants
(`mesh/v1/e2ee-bundle`, `mesh/v1/seal-key`, `mesh/v1/dm-conversation`,
`mesh/v1/e2ee-store`, `mesh/v1/sframe`). vodozemac's types stay inside
`mesh-e2ee`. Nothing outside it names them.

## Testing strategy

- **Unit tests**:
  - `bundle.rs`: a bundle from a revoked device, another root, or with a
    tampered byte is refused.
  - `session.rs`: a first message from a one-time key and from the fallback
    key. Out-of-order delivery. A replay is refused. A one-time key works
    only once.
  - `group.rs`: a member added later can't read earlier messages. A removed
    member can't read anything after the next send. Rotation after 1,000
    messages and after 7 days. A key share and its message arriving in
    either order.
  - `envelope.rs`: seal and open round trip. The wrong device can't open
    it. Every padded size. A sealed blob contains no sender key, session ID
    or conversation ID (the test searches the bytes).
  - `store.rs`: state survives restart. A crash between save and send never
    reuses a message key (simulated). A damaged file is refused.
  - `addr.rs`: v1 records still open. v2 carries the field.
- **Process test** (`tests/e2ee.rs`): a seed, Alice on two devices (the
  second from her backup), Bob and Carol.
  - Bob locks. Alice sends him a note. Bob unlocks: the note arrives with
    `path: waited`. Alice's second device shows it too.
  - Bob replies. Both of Alice's devices get it.
  - Alice, Bob and Carol in a test group: each one's note reaches the other
    two people's devices.
  - Alice removes her second device. Her next 1:1 note and group note
    reach neither it nor anything it holds.
  - Carol is dropped from the group. Bob's next group note isn't readable
    with Carol's state.
  - A note copied off the wire and replayed is refused.
  - The seed's files and every helper's stderr contain no ID, key or note
    text (the test greps for them).
- **Two-machine check** (by hand, the module gate, with the homelab seed).
  *Since 2026-11-12 the app has no test notes: send a Mesh chat message
  instead wherever this says a test note.*
  1. Windows and the Linux box are friends (from social's gate). The Linux
     box locks. Windows sends a test note.
  2. The Linux box unlocks. The note shows within 5 s of connecting.
  3. A second Linux device (a backup restored into another data folder)
     shows the note too, and Windows's reply.
  4. Windows makes a test group with the Linux identity (and a third friend,
     if there is one). Notes from each side reach every device.
  5. Remove the second Linux device on the first. A note from Windows
     doesn't reach it.
  6. `docker logs` on the seed shows counts only.

## Boundaries

- **Always:**
  - Encrypt every message with e2ee, on every path (D2).
  - Seal anything a third party holds (D8).
  - Save ratchet state before ciphertext leaves and before plaintext is
    handed up (D12).
  - Check sender devices against the newest device list (D11).
  - A test vector for every envelope and seal format.
- **Ask first:**
  - Adding `vodozemac`, or any other dependency.
  - Changing D1–D15.
  - Sharing old message keys for history, in any form (D13).
  - Turning on a post-quantum exchange (Q3).
- **Never:**
  - Put a sender, conversation or session ID outside a seal on a third
    party's path.
  - Encrypt to a device not in the newest device list.
  - Log or keep keys, IDs or contents outside the encrypted store.
  - Hand-roll a ratchet beside vodozemac.
  - Name the homelab in the public Mesh-Voice repo.

## Success criteria

1. `mesh-e2ee`, record v2 and the helper changes pass their tests on Linux
   and Windows. Clippy and fmt are clean.
2. The process test passes, covering: a note to a locked reader, own-device
   copies, a group, device removal, member removal and replay.
3. On the gate, a waiting note shows within 5 s of the reader connecting. A
   live note shows in under 1 s.
4. Encrypting one group message for a 300-member, 600-device Space takes
   under 1 ms. A full rotation (600 key shares) takes under 1 s on the Linux
   box. Measured with an `--ignored` test and recorded in the plan.
5. The release binary grows by under 1.5 MB.
6. The seed's files and logs contain no ID, key or note text.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| A stolen, unlocked device holds the root key (identity D2). Removed, it can sign itself a new device key and get back in, and get future messages | High | Removing a device keeps it out only while its keystore stays locked (password, Keep unlocked off). Linked devices list shows every device, so a device that comes back is visible. The old messages stay safe (D13). Moving to a new ID is an open question before `spaces` (map). |
| A chain leaks, and everything on it until rotation can be read | Medium | Rotation every 7 days or 1,000 messages and on every leave (D7). 1:1 chats heal on each reply (D5). |
| Messages that can't be opened, because a key share went missing | Medium | Shares go with the message (D14). One key request. Kept 30 days. The process test delivers shares and messages in both orders. |
| vodozemac's API or upkeep changes | Medium | Pinned version. Its types stay inside `mesh-e2ee`, so swapping it touches one crate. Matrix and Element depend on it. |
| Megolm uses AES-CBC with HMAC, an older construction than ours | Low | It's encrypt-then-MAC, audited, and used at Matrix's scale. Our own seals stay XChaCha20-Poly1305. |
| First messages on the fallback key have weaker forward secrecy | Low | Only until the first reply turns the ratchet. One-time keys are refilled every 30 minutes. The fallback key is replaced weekly. |
| Space members who aren't friends can't find each other's bundles | Medium | Left to `spaces` on purpose (see *What later modules get*). Groups in this module are among friends. |

## Resolved questions (2026-11-10)

- **Q1. Olm and Megolm (vodozemac), or MLS?** vodozemac (D1). Revisit MLS
  when Spaces grow past a few hundred members.
- **Q2. 1:1 chats: per-device sessions, or the group design for
  everything?** Per-device sessions for 1:1, sender chains for groups (D5,
  D6).
- **Q3. Post-quantum protection now?** Later, when vodozemac or a
  well-reviewed Rust PQXDH ships it. The envelope's version byte leaves room.
- **Q4. Should a new Space member see messages from before they joined?**
  Yes, as a per-Space setting that's on by default. The final call is in
  `spaces`. e2ee supports both, since history comes as re-sent messages
  (D13).
- **Q5. Should backups carry message sessions?** No. A restored device starts
  new sessions, and its history comes from your other devices.
- **Q6. Encrypt messages on a live connection too?** Yes (D2).

## Open questions

None.
