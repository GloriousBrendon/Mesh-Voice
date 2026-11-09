# Spec: `routing` (Mesh)

Status: **DRAFT 2026-11-09, for review.** Module 4 of the build order in
[CAPABILITY-MAP.md](CAPABILITY-MAP.md). Depends on
[`transport`](SPEC-transport.md) (built; Checkpoint C pending on Windows) and
[`seed`](SPEC-seed.md) (built; homelab deploy waiting on Checkpoint B).

## Objective

Turn a `mesh1…` ID into a live connection to that person's devices, without
anyone learning who is looking for whom or where anyone is.

Today two devices meet only by pasting a test ticket (transport D10). After
this module, Mesh dials **an ID**: it finds that person's current devices and
their relays, checks them against the person's signed device list, and
connects. That's the base for everything later: `social` sends friend
requests over it, `dm` sends messages, `mailbox` knows when to hold mail.

**What this module delivers**

1. **Contacts.** A list of IDs you've added, kept encrypted in the keystore
   and synced into the keystore's backup. It's the minimal list routing
   needs. `social` later builds requests, accept/decline and blocking on top
   of it.
2. **Rendezvous through seeds, blinded per pair.** While you're unlocked,
   each of your devices leaves a small encrypted "here I am" record on the
   seeds, **one per contact**. Each record is found and opened with a key
   only you and that contact can work out. The seed sees random lookup IDs
   and ciphertext. It can't tell which people they belong to, or that two
   records belong to the same person.
3. **Dial by ID.** `dial(id)` tries the last address that worked, then the
   seed records, and connects to the person's devices. The device
   certificate must be in the person's current signed device list.
4. **A by-hand test by ID.** You ping a contact by their ID, with no ticket.
   That's the two-machine check for this module.

**Not in v1 of this module** (see Q1 and Q2):

- **The blinded friend-of-friend lookup.** Seeds do the finding while the
  network is small (the map says this is when seeds matter most). The
  friend-of-friend lookup comes after `social`, when there are friends of
  friends to ask.
- **Being reached by someone you haven't added.** You're findable only by
  your contacts. A friend request from a stranger who has your ID or code is
  `social`'s job (Q2).
- Pushing address changes to connected contacts. A failed dial just looks
  again.

**User stories**

- I add my friend's ID (pasted, or from a friend code after Check an ID).
  They add mine. From then on either of us can ping the other by ID, from
  any network, with no ticket.
- Someone who has my ID but isn't in my contacts can't find out whether I'm
  online, or where.
- I move my laptop to a hotspot. My friend's next ping still finds me
  within a few seconds.
- I remove someone from my contacts. Within the hour they can't find me any
  more.
- The seed operator (Brendon, or later someone else) reads every byte the
  seed stores. They learn neither who my contacts are nor which IP or relay
  belongs to whom.

### Decisions this spec makes (review these)

| # | Decision | Why |
|---|---|---|
| D1 | **A pair secret from the two IDs.** `pair = X25519(my root secret, their root public)`, using ed25519-dalek's `to_scalar`/`to_montgomery` (already a dependency). Then `blake3::derive_key` with separate contexts for the lookup ID, the slot and the encryption key. | Both sides compute it from nothing but the two IDs, so there's no exchange first and nothing to sync between devices: every device already holds the root secret (identity D2). Using the same key for signing and Diffie-Hellman is sound with domain-separated derivation (ed25519-dalek cites eprint 2021/509). |
| D2 | **One record per (my device, contact)** at the seed, under `lookup = H(pair, sender ID, UTC day)` (16 bytes) and `slot = H(pair, device key)` (8 bytes). The sender ID is in the hash so A→B and B→A records differ. The day is in it so a pair's lookups can't be linked across days. | The seed sees random 16-byte IDs that change daily. Slots let a person's several devices each leave a record under the same lookup ID without the seed learning their keys. |
| D3 | **The record** (encrypted with XChaCha20-Poly1305, already a dependency, key from D1, random nonce) holds: the sender's signed `DeviceList`, this device's key, its home relay URL, created and expiry times, all signed by the device key. No IP addresses. The seed sees only `lookup`, `slot`, the expiry (so it can sweep) and the ciphertext. | The `DeviceList` lets the contact check the device is current, which is the revocation hook transport left open (transport D5). A relay URL is enough to connect (transport D3). The seed can replay old records but can't forge or read them. The newest `created` wins. |
| D4 | **Refresh every 30 minutes; records last 2 hours.** Posted on unlock, when the home relay changes and when a contact is added. Locking stops refreshes. The records then expire, and nothing tells the seed you locked. | Being findable within a couple of hours after a change is enough, because a failed dial looks again. The cost is small: 200 contacts × 1 device is 200 records of about 500 bytes per 30 minutes, sent in one stream. |
| D5 | **Seed services `addr-put` and `addr-get`**, from the throwaway identity (seed D7b), batched: one stream carries many frames. `addr-put` replaces the slot's record if it's newer. `addr-get` returns every slot under a lookup ID (at most 8). The getter asks for today and yesterday, to cover midnight and clock skew. | It's the seed's existing pattern (one stream, framed requests). The throwaway identity means the seed can't link puts and gets to anyone. |
| D6 | **Limits on the seed**, all enforced there: per device, 2,000 puts an hour and 120 gets a minute; per source network (/24 or /48), 4,000 puts an hour; 8 slots per lookup ID; 200,000 records in all (about 100 MB, swept as they expire). A full store answers `full` and keeps the records it has. | These are sized for a few hundred contacts per person while the network is small. Nothing stored identifies anyone, so there's nothing worth guessing for. The limits only stop the store being filled. |
| D7 | **Contacts live in the keystore** (`Keystore::contacts`, a new field in the encrypted contents, keystore format v2, which reads v1). Each contact has an ID, a name you chose, the time it was added, and the last address that worked (device key + relay, cached). Backups carry them. | No new file to protect. The keystore is already encrypted, atomic and backed up. `social` reuses this list rather than replacing it. |
| D8 | **`dial(id)` order**: (1) an open connection to any of their devices; (2) the cached last address; (3) the seed records, newest first, every device in parallel. The first to complete the handshake wins. If nothing answers within 10 s, the error is `unreachable`. A certificate that isn't in their current `DeviceList` gives `revoked-device`. | The cache makes the common case one dial. Parallel dialling covers someone with several devices. The cache is updated only after a successful handshake. |
| D9 | **Mutual only.** A device posts records only for its contacts, so you can reach someone only when you've both added each other. Incoming connections from non-contacts are still accepted by transport (`social` decides later what they may do). For now they may only `ping`. | You're findable only by people you chose (the map: "no search or discovery"). Deciding what strangers may send is `social`'s job (Q2). |
| D10 | **Nothing identifying is logged**, on the seed or in the helper's stderr: no IDs, keys, lookup IDs or relay URLs, only counts. Helper errors are fixed text. | Same rule as seed D10 and transport's error handling. |

## Behaviour

### Seed side (D5, D6)

- New store `addrs.rs`, like `codes.rs`: a map of `(lookup, slot)` to
  `(expiry, ciphertext)` in memory, saved to one file (postcard, written
  atomically) every 5 minutes and on SIGTERM. Losing the last 5 minutes only
  means waiting for the next refresh.
- `addr-put` frame: `lookup ‖ slot ‖ expiry ‖ ciphertext` (ciphertext at
  most 4 KiB). Answers `ok`, `slow-down`, `full` or `bad` (it doesn't parse,
  has expired, or expires more than 3 hours ahead).
- `addr-get` frame: `lookup`. Answers the records (0 to 8). "Nothing" looks
  the same whether the ID never existed or has expired.
- The hourly counts line gains puts, gets and records held.

### Client side (the helper)

- **Contacts commands**:
  - `contactAdd(id, name)` → `meshContacts`. If the ID is malformed it
    answers `meshError` with parse's kinds; adding yourself gives `self`.
  - `contactRemove(id)` and `contactRename(id, name)` → `meshContacts`.
  - `contactList` → `meshContacts`:
    `[{"id", "name", "added", "seen": unix|null}]`, where `seen` is the
    last successful handshake.
- **Publishing**: a task that runs while unlocked does D4 against every
  configured seed. On failure it backs off (1 min, doubling to 30 min).
  `meshNet` gains `"published": bool`: true when every contact's record is on
  at least one seed.
- **`netPingId(id)`** → `meshPing` (as for `netPing`, plus which device
  answered), or `meshError` with `fn: netPingId` and a kind:
  `not-a-contact`, `unreachable`, `revoked-device`, `offline`, `locked`,
  plus transport's kinds.
- **Removing a contact** stops their records being refreshed. They lapse
  within 2 hours (D4), and the cache entry is dropped.

### Mesh-Voice

On Your ID:

- **Contacts**: a list with name, ID (shortened, copyable) and "last seen",
  plus Ping (showing direct/relay and ms) and Remove.
- **Check an ID** (an ID or a friend code) gains **Add as contact** next to
  the safety number.
- The Network line shows "findable by contacts" once `published` is true.

## Tech stack

Nothing new. X25519 comes from `ed25519-dalek` alone:
`their.to_montgomery() * mine.to_scalar()`. This was spiked on 2026-11-09: both
sides get the same 32 bytes and a third key doesn't, with no
`curve25519-dalek` import. Hashing is `blake3`, encryption
`chacha20poly1305` and storage `postcard`, all already dependencies.

## Commands

```
cargo test --workspace                       # Mesh-Node
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p mesh-app -p mesh-discord       # Mesh-Voice
```

## Project structure

```
Mesh-Node/
  crates/mesh-identity/src/pair.rs        pair secret, lookup/slot/key derivation
  crates/mesh-identity/src/contacts.rs    Contact, the list; keystore v2 field
  crates/mesh-identity/src/addr.rs        the signed, encrypted address record
  crates/mesh-node/src/addrs.rs           the seed's record store and limits
  crates/mesh-node/src/seed.rs            addr-put / addr-get services
  crates/mesh-node/src/routing.rs         publisher task, dial(id), the cache
  crates/mesh-node/src/helper.rs          contact* commands, netPingId
  crates/mesh-node/tests/routing.rs       a seed and two helpers, by ID
Mesh-Voice/
  crates/mesh-discord/src/mesh.rs         meshContacts, published flag
  crates/mesh-app/src/mesh_ui.rs          Contacts; Add as contact
```

## Code style

The same as seed: small modules, typed errors, limits as named constants
next to what they limit, and derivation contexts as constants:

```rust
/// blake3 context for a pair's lookup ID (D2).
const LOOKUP_CONTEXT: &str = "mesh 2026 routing lookup v1";
```

## Testing strategy

- **Unit tests**:
  - `pair.rs`: both sides derive the same secret. A→B differs from B→A.
    The day changes the lookup ID. A third ID can't derive it.
  - `addr.rs`: seal and open round trip. Any tampered byte, the wrong pair,
    an expired record, or a device not in the list is refused.
  - `addrs.rs`: put/get, newer replaces older (older is ignored), the slot
    cap, expiry sweep, every limit, surviving a restart, a corrupt file
    refused loudly.
  - Keystore: v1 opens as v2 with no contacts. Contacts survive a password
    change, export and import.
- **Process test** (`tests/routing.rs`): a seed and two helpers on the local
  relay.
  - With Alice and Bob each other's contacts, Bob's `netPingId(alice)`
    succeeds with no ticket.
  - If Alice hasn't added Bob, Bob gets `unreachable`.
  - Once Alice removes Bob, after the record lapses (a test-only short
    expiry), Bob gets `unreachable`.
  - Once Alice restarts on a new relay, Bob's ping finds the new one.
  - The seed's file and stderr contain neither ID nor any device key (the
    test greps for them).
- **Two-machine check** (by hand, the module gate, with the homelab seed):
  1. The Windows PC and the Linux box on a phone hotspot add each other by
     friend code → Check an ID → Add as contact.
  2. Each pings the other by ID: it works, direct or through the relay.
  3. The Linux box switches network. A ping from Windows finds it again
     within 10 s of the next refresh, or straight away if the cached
     address still works.
  4. Windows removes the Linux box. Within 2 hours the Linux box's ping
     answers `unreachable`.
  5. `docker logs` on the seed shows counts only.

## Boundaries

- **Always:**
  - Every seed request goes through the throwaway identity.
  - Every record is encrypted and signed, and checked against the current
    `DeviceList`.
  - Counts-only logs.
- **Ask first:**
  - Any new dependency.
  - Changing D1–D10.
  - Starting the friend-of-friend lookup (Q1) or stranger requests (Q2).
- **Never:**
  - Publish a record that someone not in your contacts can open.
  - Put IP addresses in records.
  - Log or keep IDs, keys, lookup IDs or relay URLs on the seed.
  - Name the homelab in the public Mesh-Voice repo.

## Success criteria

1. Pair derivation, records, the seed store and contacts pass their tests on
   Linux; clippy and fmt are clean.
2. Two helpers that are each other's contacts ping by ID through a seed in
   the process test. A one-sided contact, or a removed one, can't.
3. On real machines on different networks (the gate), a ping by ID answers
   in under 3 s from a cold start, and under 1 s with a warm cache.
4. The seed's stored file and logs contain no ID, device key or relay URL
   (test plus a by-hand look).
5. Idle cost while unlocked with 50 contacts stays under 0.2% CPU on Linux
   (transport measured 0.07% before).

## Open questions

- **Q1. Friend-of-friend lookup now or later?** I recommend **later**,
  after `social`. It needs friends of friends, which don't exist yet, and
  the blinded protocol is the largest piece of design here. Pairwise seed
  records already keep the seed blind. When seeds go away (map: "phased out
  later"), friend-of-friend takes over.
- **Q2. How does a stranger with your ID or code reach you for a friend
  request?** I recommend leaving it to `social`. My suggestion there: a
  sealed "knock" box on the seed, under a lookup ID derived from your ID,
  holding requests encrypted to you, with tight limits. Routing v1 stays
  mutual-only.
- **Q3. One record per contact (D2) vs. one shared "address key" given to
  every contact.** The shared key costs one record per device instead of one
  per contact. But you'd have to hand the key out first (there's no channel
  for that before routing), and rotate it to everyone when you remove
  someone. I recommend **per contact** for v1. Revisit if people have
  thousands of contacts.
