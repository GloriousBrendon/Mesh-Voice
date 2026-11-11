# Spec: `social` (Mesh)

Status: **APPROVED 2026-11-10**: requests only through a live friend code
(Q1, your answer), silent decline, friends synced between your devices,
invisible but reachable, custom status text later. Plan:
[PLAN-social.md](PLAN-social.md). Module 5 of the build order in
[CAPABILITY-MAP.md](CAPABILITY-MAP.md). Depends on
[`routing`](SPEC-routing.md) (built; Checkpoint B by hand pending) and
[`seed`](SPEC-seed.md) (built; homelab deploy waiting on Checkpoint B).

## Objective

Turn routing's bare contacts into friends, the way Discord has them. Someone
holding one of your live friend codes can ask to be your friend; nobody else
can. You accept or decline. Friends see each other's presence, and you can
block anyone. Your other devices keep the same friends and blocks.

Today two people become reachable only when each pastes the other's ID and
clicks Add as contact (routing D9), and neither learns that the other did.
After this module, one side sends a request with your code. You get it even if
you were offline when it was sent, and one click on Accept makes you friends
on both sides.

**What this module delivers**

1. **Friend requests through a live code.** A sealed request, encrypted to
   the code's owner, left on the seed under the code. The seed takes it only
   while the code is live, and it uses up one of the code's uses. Only the
   owner can read requests back.
2. **Add by ID**, for two people who already have each other's IDs (D2).
3. **Accept, decline, cancel, remove.** Accept makes the sender a contact.
   Routing's records and allow list do the rest, and the first connection
   tells both sides they're friends.
4. **Friends, pending and blocked lists**, kept in the keystore with the
   contacts routing already stores.
5. **Blocking.** A blocked ID is never a contact. Its requests are dropped
   unread, and its connections are refused (routing D9).
6. **Account sync between your own devices.** Your friends, pending requests,
   blocks and chosen status are the same on every device.
7. **Presence.** Online, idle, do not disturb and invisible, sent only to
   friends over live connections.
8. **Mesh in Mesh-Voice's Friends page**: Online, All, Pending and Blocked,
   plus Add Friend taking a friend code (a request) or an ID (add by ID).

**Not in v1 of this module**

- Custom status text and activities (decided: later).
- Mutual friends and suggestions. Both need friend-of-friend (routing Q1).
- Offline delivery of anything except requests, which already wait on the
  seed. Presence is live only. An Accept made while the sender is offline
  reaches them when either side next connects (at most routing's 30-minute
  refresh), or through `mailbox` later.
- Telling a removed friend they were removed. Their side shows you offline
  until they remove you too.
- Reporting and moderation. There's nobody to report to. Spaces moderation
  is `spaces`.

**User stories**

- My friend gives me a friend code. I paste it into Add Friend, check the
  safety number, and send a request with a note. They're offline. When they
  open Mesh an hour later, the request is under Pending, with my name and
  note.
- They click Accept. Within a few seconds I see them under Online, and we can
  ping each other by ID.
- Someone has my ID from a screenshot but no live code. They can't send me
  anything, and they can't tell whether I'm online.
- A friend and I already have each other's IDs. We each Add by ID, and we're
  friends as soon as our devices connect.
- Someone I don't want sends me a request. I block them from the request.
  Nothing more from them shows up, on this device or my laptop.
- I set Do not disturb on my desktop. My laptop shows Do not disturb too,
  and so do my friends.
- I go Invisible. My friends see me as offline, but they can still reach me.
- I remove a friend. They drop off my lists on both my devices, and within
  routing's 2 hours they can't find me any more.

### Decisions this spec makes

| # | Decision | Why |
|---|---|---|
| D1 | **Requests only through a live friend code** (Q1). A request is put on the seed under the code's `lookup_id`, which only someone who has the code can derive (Argon2id, identity D4). The seed takes it only while that code is held there and live: not expired, revoked or used up. **Each request uses up one of the code's uses**, so a single-use code (the default) admits one request. Looking a code up (`code-get`) no longer uses it up. *This changes seed D7, where a lookup counted a use.* | Your answer: "that's why we built it to expire." Having an ID alone gets a stranger nothing. A code's expiry, use limit and revoke now control who can ask, which is what they were for. Counting the request rather than the lookup means Check an ID (resolve, then compare the safety number) doesn't burn the code before the request is sent. |
| D2 | **Two people with only IDs become friends by each adding the other** (Add by ID). Adding an ID makes it an `outgoing` contact. That publishes routing records for them and lets their devices in. When both have done it, the first connection between them makes them friends. Nothing is sent to someone who added you by ID alone. They find out only if they add you too. | With D1, an ID can't carry a request, so the only way in by ID is the mutual one routing already has. It's like swapping phone numbers: each of you saves the other. No request goes to the other side, so a leaked ID still gets a stranger nothing. |
| D3 | **Only the owner reads requests.** `knock-get` carries the code's `CodeRevoke` (lookup ID and revoke secret, identity D5). The seed checks it against the code's revoke tag, the same way `code-revoke` does, and answers that code's requests and whether the code is still live. | Nobody else, including other code holders, can see or count your requests. Reading shows the seed the revoke secret. That only lets the seed delete the code, which it could do anyway. |
| D4 | **A sealed request.** The sender makes a throwaway key and seals to the owner's root key: `X25519(throwaway, owner root)` → `blake3::derive_key("mesh/v1/knock-key")` → XChaCha20-Poly1305. Inside, signed by the sender's device key (domain `mesh/v1/knock`): the sender's `DeviceList`, the owner's ID, the code's lookup ID, the sender's display name (≤ 64 bytes), a note (≤ 280 bytes) and the created time. The seed sees the lookup ID, an 8-byte slot and ciphertext. | It reuses routing's machinery: `RootKey::diffie_hellman` already takes an Edwards public key, so the throwaway key is a fresh `RootKey`. No new dependencies. The owner ID and lookup ID inside stop the seed, or anyone else, replaying a request under another code. The `DeviceList` proves the sender's ID and gives routing their devices. |
| D5 | **The slot is per sender**: the pair's slot (routing D1's pair secret, context `mesh/v1/knock-slot`, mixed with the lookup ID). A resend replaces the earlier request, and still uses a use. | The seed can't tell two senders apart, but one sender can't stack requests. |
| D6 | **Seed limits for requests**: per device, 20 puts an hour and 60 gets a minute; per source network (/24 or /48), 60 puts an hour; at most 32 requests per code, dropping the oldest; 4 KiB per request (as address records, since both carry a device list); 100,000 in all. A request is kept **7 days**, even after its code is gone, so the owner can still collect it. | The code's own uses already bound requests per code, and the per-network limit bounds what one place can send. Seven days covers someone away for a week. |
| D7 | **The owner's device collects requests** for every code it made, at unlock and every 5 minutes. It keeps collecting for a code until the seed says the code is gone and it has read that code's requests. Then it drops the code from its list. *Changed from today, where a used-up code leaves the list as soon as a seed says so: the last request would be lost.* Codes are per device (identity), so the device that made a code collects for it. The requests then reach your other devices through sync (D11). | No request is lost because of when the code ran out. |
| D8 | **Friends are mutual contacts.** Each person in your account state has a kind: `outgoing` (you asked, or added their ID), `friend`, `incoming` (they asked; **not** let through the handshake), `blocked`, or `removed` (a tombstone for sync). Routing's contacts are exactly the `outgoing` and `friend` entries. Any connection that passes the handshake with an `outgoing` contact makes them a `friend`, because their device let you in, so they've added you too. | Routing's allow list already makes a connection mean "we've both added each other". Accept, and add-by-ID on both sides, both end up as such a connection, so there's no separate "accepted" message to lose. |
| D9 | **Accept, decline, cancel, remove.** Accept turns `incoming` into `friend`, publishes records, and dials the sender. Their side becomes `friend` on that connection (D8). If the sender is offline, it happens the next time either side dials (routing's refresh). **Decline is silent**: the request is dropped, the sender stays `outgoing`, and nothing tells them. Cancel drops your `outgoing` entry. Remove turns a friend into `removed` and closes their connections. | As in Discord, a declined sender isn't told, which keeps it free of pressure and leaks nothing. |
| D10 | **Blocking.** `block(id)` makes the ID `blocked`, drops any contact or request for them, and closes their connections. A blocked sender's requests are opened only to read who sent them, then dropped and never shown. Unblocking makes them `removed` (it doesn't bring back a friendship). At most 10,000 blocks. | Per account, kept on your devices (map decision). Because a blocked ID is never a contact, routing D9 already refuses its connections and never publishes records for it. Blocking adds only the request filter and the sync. |
| D11 | **Account sync between your own devices.** The account state is a last-writer-wins map keyed by ID. Each entry has a kind, your nickname for them, a verified flag, their display name and note (for requests), and `(changed_at, device)`. `removed` tombstones are kept for good: the newest 1,000, the oldest going first past that. *Changed 2026-11-11 (audit finding 14): with a 90-day life, a device unused that long brought back friends removed since, and filtering old entries instead kept a newly linked or restored device from taking friendships older than 90 days.* Your chosen status is synced the same way. Your devices find each other through routing under the **self pair** (`PairKeys::new(root, own ID)`), which only your devices can derive. They connect (own devices are allowed, routing D9) and swap state on a `sync` stream: at connect, at each refresh, and on every change to the devices that are connected. | Your answer: sync friends, not just blocks. Otherwise a friend you add on the desktop can't reach the laptop, because only the desktop publishes records for them. The self pair gives own-device rendezvous with no new mechanism. Last-writer-wins with tombstones is enough for one person editing their own lists, and it ends the same whatever order merges happen in. |
| D12 | **Presence over live connections, to friends only.** While unlocked, the helper dials each friend and `outgoing` contact at unlock and at each refresh. Once one is a friend and connected, each side sends its presence on a `presence` stream, on change and every 60 s. A friend who sends nothing for 150 s, or whose connections all close, is offline. | Presence has to be live, and seed records refresh every 30 minutes, which is too slow. Friends are tens, not thousands, and an idle QUIC connection costs almost nothing (transport measured 0.07% CPU). |
| D13 | **The status model.** You pick one status for the account: online, idle, DND or invisible, synced to your devices (D11). Each device also goes auto-idle after 10 minutes without input in Mesh-Voice. A friend sees your best device: DND beats online, which beats idle. **Invisible** sends `offline` to friends, but you stay connected and reachable (Q4, as recommended). | Discord's model, which people already know. Auto-idle has to be per device, because it's per keyboard. |
| D14 | **Names.** A friend's display name comes from their request (or their sync of it) and is only a hint (identity: "self-chosen, not unique"). Your nickname for them wins when set. **Verified** is a flag you set after comparing the safety number, and it's synced (D11). | Identity left showing "verified" to `social`. |
| D15 | **Nothing identifying is logged**, on the seed or in the helper's stderr: no IDs, lookup IDs, names or notes, only counts. | Same rule as routing D10. |

## Behaviour

### Seed side (D1, D3, D6)

- `code-get` answers as before but no longer counts a use.
- New store `knocks.rs`, shaped like `addrs.rs`: lookup ID → the code's
  revoke tag and up to 32 `(slot, put_at, ciphertext)`, saved to
  `knocks.bin` every 5 minutes and on stop. A damaged file is refused.
- `knock-put` frame: `lookup_id ‖ slot ‖ ciphertext`. Answers `ok`, `gone`
  (no live code under that lookup ID; this looks the same as never held),
  `slow-down`, `full` or `bad`. `ok` uses up one of the code's uses.
- `knock-get` frame: a `CodeRevoke` (48 bytes). Answers `0`, a byte for live
  (1) or gone (0), then each request (u16 length + bytes); `1` if the revoke
  doesn't match a code or box; or `3` (slow-down).
- Requests are swept 7 days after they were put. A box goes once it's empty
  and its code is gone.
- The hourly counts line gains request puts, gets and requests held.

### Client side (the helper)

- **Commands**, each answered by `meshSocial`:
  - `friendRequest(code, note)`: resolves the code, seals a request, puts it
    on every seed, then makes the owner `outgoing`. Errors: the code's parse
    kinds, `not-found` (the code is gone), `self`, `blocked` (unblock them
    first), `already-friend`, `slow-down`, `offline`.
  - `friendAdd(id, nick)`: add by ID (D2), as `outgoing`.
  - `friendAccept(id)`, `friendDecline(id)`, `friendCancel(id)`,
    `friendRemove(id)`, `friendRename(id, nick)`,
    `friendVerify(id, true|false)`.
  - `block(id)` and `unblock(id)`. Blocking works whether or not they're in
    your lists.
  - `statusSet(online|idle|dnd|invisible)`; `activity(active|idle)`, which
    Mesh-Voice sends after 10 minutes without input and on the next input.
  - `socialList`.
- These replace routing's `contact*` commands and `meshContacts`.
  `netPingId` stays, for friends and `outgoing` contacts.
- **`meshSocial`** is sent after each command, when a request arrives, on a
  presence change and after a sync:

  ```json
  {"friends":  [{"id","name","nick","verified","presence":"online|idle|dnd|offline","seen"}],
   "incoming": [{"id","name","note","at"}],
   "outgoing": [{"id","nick","at"}],
   "blocked":  [{"id","nick","at"}],
   "status": "online|idle|dnd|invisible"}
  ```

- **Streams**: two new transport services, `presence` (status frames) and
  `sync` (account state), both postcard in length-capped frames.

### Mesh-Voice

*As built (2026-11-10): Mesh-Voice has no Friends page for any network, so
the lists, requests and status menu are on Your ID for now
([PLAN-social.md](PLAN-social.md) Task 10). A Discord-style page in the
shell is open for your decision.*

- **Friends page** for Mesh, as for the other networks: Online, All, Pending
  (incoming with Accept / Decline / Block, outgoing with Cancel) and Blocked
  (Unblock). The presence dot comes from `presence`.
- **Add Friend** gains a Mesh mode. A friend code resolves (as Check an ID
  does) and shows the safety number, an optional note and Send request. An
  ID shows Add by ID, with a line saying they need to add you too.
- **Your status** menu: Online, Idle, Do not disturb, Invisible.
- **A friend's profile**: nickname (editable), short ID, safety number with
  Mark as verified, Ping, Remove, Block.
- Your ID's Contacts list (routing) goes away. The Friends page replaces it.

## Tech stack

Nothing new. The request seal reuses `RootKey::diffie_hellman` (routing D1)
with a throwaway `RootKey`, plus `blake3`, `chacha20poly1305` and `postcard`,
all already dependencies.

## Commands

```
cargo test --workspace                       # Mesh-Node
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p mesh-app -p mesh-discord       # Mesh-Voice
```

## Project structure

```
Mesh-Node/
  crates/mesh-identity/src/knock.rs       seal and open a request
  crates/mesh-identity/src/account.rs     account state: LWW map, merge, tombstones
  crates/mesh-identity/src/keystore.rs    account state stored, contacts kept in step
  crates/mesh-node/src/codes.rs           code-get stops counting; spend a use
  crates/mesh-node/src/knocks.rs          the seed's request store and limits
  crates/mesh-node/src/seed.rs            knock-put / knock-get services
  crates/mesh-node/src/social.rs          collecting requests, presence, sync (a child module of helper.rs)
  crates/mesh-node/src/helper.rs          friend*, block, status commands; meshSocial
  crates/mesh-node/tests/social.rs        a seed, two people, one with two devices
Mesh-Voice/
  crates/mesh-discord/src/mesh.rs         meshSocial, the new commands
  crates/mesh-app/src/…                   Friends page, Add Friend Mesh mode, status
```

## Code style

The same as routing: pure, unit-tested identity modules, a seed store shaped
like `codes.rs`, limits as named constants, and derivation contexts as
constants (`mesh/v1/knock-key`, `mesh/v1/knock-slot`, `mesh/v1/knock`).

## Testing strategy

- **Unit tests**:
  - `knock.rs`: seal and open round trip. The wrong owner, a tampered byte,
    a request replayed under another lookup ID, and a device list from
    another root are all refused.
  - `account.rs`: merge is commutative and idempotent. The newer change wins.
    A tombstone beats an older add and loses to a newer one. Only the
    oldest tombstones past 1,000 go; an empty account takes an old friend.
  - `knocks.rs` and `codes.rs`: a put needs a live code and uses a use; a
    single-use code takes one request; a resend replaces; a full box drops
    its oldest; reading needs the right revoke; requests outlive their code;
    every limit; restart; a damaged file.
  - Keystore: a keystore from routing opens with its contacts as `outgoing`.
    The account state survives unlock and backup.
- **Process test** (`tests/social.rs`): a seed, Alice, and Bob on two devices
  (the second from Bob's backup).
  - Bob makes a code and locks. Alice requests with it. A second request
    with the same single-use code gets `not-found`. Bob unlocks, sees the
    request and accepts. Both show each other as friends and online, and
    ping by ID.
  - Bob's second device shows Alice as a friend without doing anything.
  - Bob sets DND: Alice sees `dnd`. Bob goes invisible: Alice sees `offline`
    but can still ping him.
  - Alice locks. Bob sees her offline.
  - Carol requests Bob, and Bob blocks her from the request. Her next
    request (with a new code) shows on neither of Bob's devices.
  - Decline: Dave stays `outgoing` and gets no event.
  - Add by ID on both sides makes two people friends.
  - The seed's files and log contain no ID, name or note.
- **Two-machine check** (by hand, the module gate, with the homelab seed):
  1. The Linux box makes a friend code and closes Mesh. Windows sends a
     request with it.
  2. The Linux box opens Mesh, sees the request and accepts. Both show
     Online within 5 s.
  3. Status changes show on the other machine within 2 s.
  4. Block and unblock from the other side, then remove.
  5. `docker logs` on the seed shows counts only.

## Boundaries

- **Always:**
  - Every seed request goes through the throwaway identity.
  - Every request is sealed to the owner and signed by the sender's device.
  - Presence goes to friends only.
  - Counts-only logs.
- **Ask first:**
  - Any new dependency.
  - Changing D1–D15.
  - Custom status text, or anything that would show presence to
    non-friends.
- **Never:**
  - Take a request without a live code.
  - Let an `incoming` request through the handshake before it's accepted.
  - Show a blocked ID's request, or tell them they're blocked.
  - Tell a sender they were declined.
  - Log or keep IDs, names or notes on the seed.
  - Name the homelab in the public Mesh-Voice repo.

## Success criteria

1. Requests, the account state, the seed store and the keystore changes pass
   their tests on Linux and Windows. Clippy and fmt are clean.
2. The process test passes, covering: a request sent while the owner is
   locked, a spent code refused, accept, sync to a second device, presence,
   block, decline, and add by ID.
3. On the gate, Accept → both Online in under 5 s, and a status change shows
   on the other side in under 2 s.
4. Idle while unlocked with 50 friends, 10 of them online, stays under 0.5%
   CPU on Linux. Live presence connections cost more than routing's 0.2%.
5. The seed's request file and logs contain no ID, name or note.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Someone with an unlimited, never-expiring code floods the box | Low | A box holds 32 and drops the oldest. The per-network limits apply. Revoke the code. |
| Requests reach only the device that made the code | Low | Sync passes them to your other devices as soon as they connect (D11). |
| Clock skew makes a request look too old or too new | Low | The owner accepts a created time up to a day ahead and up to 8 days old. |
| Two devices edit the same entry at once | Low | Last-writer-wins by `changed_at`, ties broken by device key. One person rarely races themselves. |
| Many live presence connections on a big friends list | Low | Only friends who are online, and idle QUIC is cheap. Criterion 4 measures it. |

## Resolved questions (2026-11-10)

- **Q1. Who can send a request?** Only someone holding a live friend code
  (D1). Two people with only IDs add each other (D2).
- **Q2. Decline?** Silent, like Discord (D9).
- **Q3. Sync friends between devices?** Yes: friends, pending requests,
  blocks and status (D11).
- **Q4. Invisible?** Friends see you offline, but can still reach you (D13).
- **Q5. Custom status text?** Later.

## Open questions

None.
