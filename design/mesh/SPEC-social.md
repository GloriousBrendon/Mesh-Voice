# Spec: `social` (Mesh)

Status: **DRAFT 2026-11-10, for your review.** Module 5 of the build order in
[CAPABILITY-MAP.md](CAPABILITY-MAP.md). Depends on
[`routing`](SPEC-routing.md) (built; Checkpoint B by hand pending) and
[`seed`](SPEC-seed.md) (built; homelab deploy waiting on Checkpoint B). Gains
offline delivery for presence and "accepted" notices when `mailbox` lands.

## Objective

Turn routing's bare contacts into friends, the way Discord has them. A
stranger who has your ID or a friend code can ask to be your friend. You
accept or decline. Friends see each other's presence, and you can block
anyone. Your other devices keep the same friends and blocks.

Today two people become reachable only when each pastes the other's ID and
clicks Add as contact (routing D9). Neither learns that the other did. After
this module, one side sends a request, the other gets it even if they were
offline when it was sent, and one click on Accept makes them friends on both
sides.

**What this module delivers**

1. **Friend requests through the seed.** A sealed "knock" record, encrypted
   to the recipient's ID, left in a box on the seed that only someone who
   knows the ID can find (routing Q2). It holds the sender's ID, name, device
   list and a short note.
2. **Accept, decline, cancel.** Accept makes the sender a contact, so
   routing's records and the handshake allow list do the rest. Then the
   recipient's device dials the sender to say so.
3. **Friends, pending and blocked lists**, kept in the keystore with the
   contacts routing already stores.
4. **Blocking.** A blocked ID is never a contact. Its requests are dropped
   unread, and its connections are refused (routing D9).
5. **Account sync between your own devices.** Your friends, pending requests,
   blocks and chosen status are the same on every device you've linked.
6. **Presence.** Online, idle, do not disturb, and invisible, sent only to
   friends over live connections.
7. **Mesh in Mesh-Voice's Friends page**: Online, All, Pending and Blocked,
   plus Add Friend taking a `mesh1…` ID, a `mesh://` link or a friend code.

**Not in v1 of this module**

- Custom status text and activities ("Playing …"). See Q5.
- Mutual friends and friend suggestions. Both need friend-of-friend, which is
  routing Q1.
- Offline delivery of anything except requests. Those already wait on the
  seed. Presence is live only. An "accepted" notice for someone offline
  waits until their next dial works, or until `mailbox` lands.
- Reporting or moderation tools. There's nobody to report to. Spaces
  moderation is `spaces`.

**User stories**

- I paste my friend's friend code into Add Friend, check the safety number,
  and send a request with a note. They're offline. When they open Mesh an
  hour later, the request is under Pending, with the note and my name.
- They click Accept. Within a few seconds I see them under Online, and we can
  ping each other by ID.
- Someone I don't want sends me requests. I block them from the request.
  Nothing more from them shows up, on this device or my laptop.
- I set Do not disturb on my desktop. My laptop shows Do not disturb too,
  and so do my friends.
- I go Invisible. My friends see me as offline, but they can still reach me
  (Q4).
- I remove a friend. They drop off my lists on both my devices, and within
  routing's 2 hours they can't find me any more.

### Decisions this spec makes (review these)

| # | Decision | Why |
|---|---|---|
| D1 | **Knock boxes on the seed.** Each ID has a box per UTC day: `box = H("mesh/v1/knock-box", recipient ID, day)` (16 bytes). Anyone who knows the ID can put a request in. Only the recipient can read one. The recipient reads today's box and the 6 days before. | A stranger has nothing but your ID, so the box has to be derivable from the ID alone. Changing it daily means the seed can't link one day's box to the next without already knowing the ID. Someone who has your ID can see how many requests you have waiting (not who sent them). That's the cost of reachability, and it's listed under Risks. |
| D2 | **A sealed knock.** The sender makes a throwaway X25519 key and seals to the recipient's root key: `X25519(throwaway, recipient root)` → `blake3::derive_key("mesh/v1/knock-key")` → XChaCha20-Poly1305. Inside: the sender's `DeviceList`, the recipient ID, the day, the sender's display name (≤ 64 bytes), a note (≤ 280 bytes) and created time, all signed by the sender's device key (domain `mesh/v1/knock`). The seed sees only box, slot, expiry and ciphertext. | It reuses routing's machinery: `RootKey::diffie_hellman` already takes an Edwards public key, so the throwaway key is just a fresh `DeviceKey`. Nothing new to depend on. The recipient ID and day inside stop the seed, or anyone else, replaying a knock into another box. The `DeviceList` proves the sender's ID and gives routing their devices. |
| D3 | **Seed services `knock-put` and `knock-get`**, batched like `addr-*` and sent from the throwaway identity (seed D7b). Slot = the pair's slot (routing D1's pair secret, context `mesh/v1/knock-slot`, 8 bytes), so a resend from the same sender replaces their earlier knock rather than adding one, and the seed still can't tell two senders apart. Knocks last **7 days**, the same as a default friend code. | The same shape as routing D5, so the store code is a copy of `addrs.rs` with different limits. Seven days covers someone who's away for a week. |
| D4 | **Seed limits for knocks**: per device, 20 puts an hour and 60 gets a minute; per source network (/24 or /48), 60 puts an hour; **32 knocks per box**, and the oldest is dropped when it's full; 2 KiB per knock; 100,000 knocks in all. | Keys are free, so the per-network limit is the one that stops a flood. A full box drops the oldest knock, so a flood buries itself rather than locking out a real request that comes later. |
| D5 | **Friends are mutual contacts.** Each contact gains a state: `friend`, `outgoing` (I asked, they haven't accepted) or `incoming` (they asked; held as a request, **not** allowed through the handshake). Sending a request makes the recipient `outgoing`. That publishes routing records for them and lets their devices in, so their Accept can reach me. | Routing already does reachability through the contact list and the allow list (D9). An `outgoing` contact is exactly routing's one-sided contact, so nothing in routing changes. An `incoming` request mustn't open your device to the sender until you accept. |
| D6 | **Accept, decline, cancel.** Accept turns `incoming` into `friend`, which publishes records and allows the sender. Then it dials the sender and sends `accepted` on a new `social` stream. The sender's side turns `outgoing` into `friend`. If the dial fails, the sender finds out the next time either side's dial works (routing refreshes every 30 minutes). **Decline is silent**: the request is dropped, the sender stays `outgoing`, and nothing tells them (Q2). Cancel drops my `outgoing` contact. | Discord doesn't tell you you've been declined, and that keeps it free of pressure. Silent decline also leaks nothing. |
| D7 | **Blocking.** `block(id)` removes any contact or request for the ID, adds it to a block list in the keystore, and closes its connections. Requests from a blocked ID are opened only to read the sender, then dropped and never shown. Unblocking doesn't restore the friendship. **At most 10,000 blocks.** | Per account, kept on your devices (map decision). Because a blocked ID is never a contact, routing D9 already refuses its connections and never publishes records for it. Blocking adds only the request filter and the sync. |
| D8 | **Account sync between your own devices.** Friends (with nicknames and verified flags), pending requests, blocks and your chosen status form one **account state**. It's a last-writer-wins map keyed by ID: each entry carries `(state, changed_at, device)`, and removals stay as tombstones for 90 days. Your devices find each other through routing under the **self pair** (`PairKeys::new(root, own ID)`), which every device can derive. They connect (own devices are already allowed by routing D9), and swap state on a `sync` stream: at connect, then each change is pushed to the devices that are online. Q3 asks whether to sync contacts as well as blocks. | Without contact sync, a friend added on the desktop can't reach the laptop, because only the desktop publishes records for them. The self pair gives own-device rendezvous with no new mechanism. LWW with tombstones is enough for one person editing their own lists, and it converges whatever order the merges happen in. |
| D9 | **Presence over live connections, to friends only.** Unlocked, the helper keeps a connection to each friend it can reach. It dials them at unlock and retries the unreachable ones at each refresh. On a `presence` stream it sends `{status, devices_online}` on change, and a heartbeat every 60 s. Missing two heartbeats means offline. | Presence has to be live, and seed records refresh every 30 minutes, which is too slow. Friends are few (tens, not thousands), and an idle QUIC connection costs almost nothing (transport measured 0.07% CPU). Only friends: not `outgoing`, not `incoming`. |
| D10 | **The status model.** You pick one status for the account: online, idle, DND or invisible. It's synced to your other devices (D8). Each device also goes **auto-idle** after 10 minutes without input in Mesh-Voice, reported to the helper. A friend sees your best device: DND, if you chose it, beats everything; then online, then idle. **Invisible** sends `offline` to friends, but you stay connected and reachable (Q4). | That's Discord's model, and people already know it. Auto-idle has to be per device, because it's per keyboard. |
| D11 | **Names.** A friend's display name comes from their request or their presence stream, and is only a hint (identity: "self-chosen, not unique"). Your nickname for them is the contact name routing already stores, and it wins when set. **Verified** is a flag you set after comparing the safety number. It's synced (D8), and it's cleared if their `DeviceList` root ever changes. | Identity left showing "verified" to `social`. |
| D12 | **Nothing identifying is logged** on the seed or in the helper's stderr: no IDs, box IDs, names or notes, only counts. | Same rule as routing D10. |

## Behaviour

### Seed side (D3, D4)

- New store `knocks.rs`, a copy of `addrs.rs`'s shape: `(box, slot)` →
  `(expiry, ciphertext)`, saved to `knocks.bin` every 5 minutes and on stop.
  A damaged file is refused.
- `knock-put` frame: `box ‖ slot ‖ expiry ‖ ciphertext`. Answers `ok`,
  `slow-down`, `full` (the whole store; a full box drops its oldest instead)
  or `bad` (it doesn't parse, has expired, or expires more than 8 days
  ahead).
- `knock-get` frame: `box`. Answers 0 to 32 knocks. An empty box and a box
  that never existed look the same.
- The hourly counts line gains knock puts, gets and knocks held.

### Client side (the helper)

- **Requests**:
  - `friendRequest(id, note)` → `meshSocial`. It seals a knock and puts it on
    every seed, then makes the ID an `outgoing` contact. Errors: parse's
    kinds, `self`, `blocked` (you blocked them; unblock first),
    `already-friend`, `slow-down`, `no-seed`.
  - The helper reads its boxes at unlock and every 5 minutes. New requests
    appear in `meshSocial`. A request from someone who is `outgoing` for you
    (you both asked) becomes a friendship straight away, on both sides.
  - `friendAccept(id)`, `friendDecline(id)`, `friendCancel(id)`,
    `friendRemove(id)` → `meshSocial`.
- **Blocking**: `block(id)` and `unblock(id)` → `meshSocial`. Blocking
  someone works whether or not they're a contact.
- **Status**: `statusSet(online|idle|dnd|invisible)` and
  `activity(active|idle)` (Mesh-Voice sends `idle` after 10 minutes without
  input, and `active` on the next input).
- **`meshSocial`** replaces routing's `meshContacts` (Mesh-Voice moves over
  in the same change), and `contactList` becomes `socialList`:

  ```json
  {"friends":  [{"id","name","nick","verified","presence":"online|idle|dnd|offline","seen"}],
   "incoming": [{"id","name","note","at"}],
   "outgoing": [{"id","nick","at"}],
   "blocked":  [{"id","nick","at"}],
   "status": "online|idle|dnd|invisible"}
  ```

  It's sent after every command above, on a presence change (at most once a
  second), and after a sync.
- **Streams**: two new transport services. `social` carries `accepted` and
  `removed` (sent as a courtesy, so the other side drops you from Online
  straight away). `presence` carries status and heartbeats. Plus `sync`
  between own devices. All three are framed postcard, with a length cap.

### Mesh-Voice

- **Friends page** for Mesh, as for the other networks: Online, All, Pending
  (incoming with Accept / Decline / Block, outgoing with Cancel) and Blocked
  (Unblock). The presence dot comes from `presence`.
- **Add Friend** gains a Mesh mode: an ID, `mesh://` link or friend code
  resolves (as Check an ID does), then shows the name hint and safety
  number, an optional note and Send request.
- **Your status** menu: Online, Idle, Do not disturb, Invisible.
- **A friend's profile**: nickname (editable), short ID, safety number with
  Mark as verified, Remove, Block.
- Your ID's Contacts list (routing) goes away. The Friends page replaces it.
  Ping stays on the profile as a diagnostic.

## Tech stack

Nothing new. The knock seal reuses `RootKey::diffie_hellman` (routing D1) with
a throwaway `DeviceKey`, plus `blake3`, `chacha20poly1305` and `postcard`, all
already dependencies.

## Commands

```
cargo test --workspace                       # Mesh-Node
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p mesh-app -p mesh-discord       # Mesh-Voice
```

## Project structure

```
Mesh-Node/
  crates/mesh-identity/src/knock.rs       box derivation, seal and open a knock
  crates/mesh-identity/src/account.rs     account state: LWW map, merge, tombstones
  crates/mesh-identity/src/contacts.rs    contact state (friend/outgoing/incoming), verified
  crates/mesh-identity/src/keystore.rs    blocks, status, account state
  crates/mesh-node/src/knocks.rs          the seed's knock store and limits
  crates/mesh-node/src/seed.rs            knock-put / knock-get services
  crates/mesh-node/src/social.rs          request polling, accept, presence, sync
  crates/mesh-node/src/helper.rs          friend*, block, status commands; meshSocial
  crates/mesh-node/tests/social.rs        a seed, two people, one with two devices
Mesh-Voice/
  crates/mesh-discord/src/mesh.rs         meshSocial, the new commands
  crates/mesh-app/src/…                   Friends page, Add Friend Mesh mode, status
```

## Code style

The same as routing: pure, unit-tested identity modules, a seed store shaped
like `codes.rs`, limits as named constants, and derivation contexts as
constants (`mesh/v1/knock-box`, `mesh/v1/knock-key`, `mesh/v1/knock`).

## Testing strategy

- **Unit tests**:
  - `knock.rs`: seal and open round trip. The wrong recipient, a tampered
    byte, a knock replayed into another box or day, and a device list from
    another root are all refused.
  - `account.rs`: merge is commutative and idempotent. The newer change wins.
    A tombstone beats an older add and loses to a newer one. Tombstones
    expire after 90 days.
  - `knocks.rs`: put and get, a resend replacing the old knock, a full box
    dropping its oldest, every limit, restart, a damaged file.
  - Keystore: an old keystore opens with no blocks and every contact as
    `friend`. Blocks and status survive unlock and backup.
- **Process test** (`tests/social.rs`): a seed, Alice, and Bob on two devices
  (the second from Bob's backup).
  - Alice requests Bob while Bob is locked. Bob unlocks, sees the request and
    accepts. Both show each other as friends and online, and ping by ID.
  - Bob's second device shows Alice as a friend without doing anything.
  - Bob sets DND. Alice sees `dnd`. Bob goes invisible. Alice sees `offline`
    but can still ping him.
  - Alice locks. Bob sees her offline within 2 minutes.
  - Bob blocks Carol from her request, on device 1. Carol's next request
    shows on neither device.
  - Decline: Carol stays `outgoing` and gets no event.
  - The seed's files and log contain no ID, name or note.
- **Two-machine check** (by hand, the module gate, with the homelab seed):
  1. Windows sends the Linux box a request with a friend code, while the
     Linux box's Mesh is closed.
  2. The Linux box opens Mesh, sees the request and accepts. Both show
     Online within 5 s.
  3. Status changes show on the other machine within 2 s.
  4. Block and unblock from the other side, and remove.
  5. `docker logs` on the seed shows counts only.

## Boundaries

- **Always:**
  - Every seed request goes through the throwaway identity.
  - Every knock is sealed to the recipient and signed by the sender's device.
  - Presence goes to friends only.
  - Counts-only logs.
- **Ask first:**
  - Any new dependency.
  - Changing D1–D12.
  - Custom status text, or anything that would show presence to non-friends.
- **Never:**
  - Let an `incoming` request through the handshake before it's accepted.
  - Show a blocked ID's request, or tell them they're blocked.
  - Tell a sender they were declined.
  - Log or keep IDs, names or notes on the seed.
  - Name the homelab in the public Mesh-Voice repo.

## Success criteria

1. Knocks, the account state, the seed store and the keystore changes pass
   their tests on Linux and Windows. Clippy and fmt are clean.
2. The process test passes: a request sent while the recipient is locked,
   accept, sync to a second device, presence, block, decline.
3. On the gate, accept → both Online in under 5 s, and a status change shows
   on the other side in under 2 s.
4. Idle while unlocked with 50 friends, 10 of them online, stays under 0.5%
   CPU on Linux (live presence connections cost more than routing's 0.2%).
5. The seed's knock file and logs contain no ID, name or note.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Someone with your ID floods your box | Medium | Per-network limits (D4). A full box drops its oldest, so a later real request still lands. Q1's friend-code-only option would close this off. |
| Someone with your ID can see how many requests you have waiting | Low | It shows a count, never the senders. It changes daily, and can't be linked across days without the ID. |
| Clock skew puts a knock in a different day's box | Low | The recipient reads 7 days of boxes, and the sender uses its own day, so at most a day's offset. |
| Two devices edit the same entry at once | Low | LWW by `changed_at`, ties broken by device key. One person rarely races themselves. |
| Many live presence connections on a big friends list | Low | Only friends who are online, and idle QUIC is cheap. Criterion 4 measures it. |

## Open questions

- **Q1. Who can send you a request?** (a) **Anyone who has your ID,
  recommended.** That's the knock box as above, and it's how Discord works
  with a username. (b) Only someone holding one of your live friend codes:
  the box would be derived from the code, so a leaked ID alone can't reach
  you, but you'd have to make a code for every new friend. I recommend (a)
  for v1, and (b) as a later "only requests via friend code" toggle if spam
  shows up.
- **Q2. Decline: silent or told?** I recommend **silent** (D6): the sender
  stays "pending" until they cancel. That's Discord's behaviour, and it leaks
  nothing. The alternative is an explicit `declined` notice.
- **Q3. Sync contacts as well as blocks between your devices?** The map only
  says blocks sync. I recommend **syncing friends, pending requests, blocks
  and status** (D8). Otherwise a friend you add on the desktop can't reach
  the laptop, because only the desktop publishes routing records for them.
  The cost is the account-state merge (`account.rs`, size M).
- **Q4. Invisible: reachable or fully offline?** I recommend **reachable**
  (D10): friends see you offline, but your records stay up and calls and
  messages still reach you, as in Discord. The alternative is to stop
  publishing and refuse connections while invisible, which is a real "off"
  but means nobody can reach you.
- **Q5. Custom status text now?** I recommend **later**. It's small, but it's
  more text going to every friend, and nothing else in v1 needs it.
