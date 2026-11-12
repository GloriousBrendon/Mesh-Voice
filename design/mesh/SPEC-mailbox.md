# Spec: `mailbox` (Mesh)

Status: **DRAFT 2026-11-12, for review.** No plan and no code until it's
approved. Module 8 of the build order in [CAPABILITY-MAP.md](CAPABILITY-MAP.md).
Depends on [`routing`](SPEC-routing.md), [`seed`](SPEC-seed.md),
[`e2ee`](SPEC-e2ee.md) and [`dm`](SPEC-dm.md) (all built). It changes
routing, seed and dm; those changes are proposals at the end (*Changes this
needs elsewhere*), not made yet.

## Objective

Today a message reaches someone only while one of your devices and one of
theirs are online at the same time (dm D5). Send a message, quit, and your
friend who comes online an hour later sees nothing until you're both back.
Discord never has that problem, because its servers hold the message.

`mailbox` adds **holders**: nodes that agree to keep sealed mail for someone
who's away, and hand it over when they come back. A holder can't read the
mail, can't tell who it's from, and can't be made to hold mail for people
who never asked it to.

**What this module delivers**

1. **Offline delivery, 1:1**: you send and go offline; your friend comes
   online later, while you're still off, and gets it.
2. **The same for a group**: each member who comes online later gets it.
3. **The same for your own other device**: what you sent from one device
   reaches your other one while the first is off.
4. **Holders that anyone can run.** Your record tells your friends which
   holders keep your mail. A holder is any node that agrees to: our seeds,
   a seed a friend runs for their friends, or a friend's device that has
   offered to.
5. **Keys for someone who's been away**: the keys-only record SPEC-dm's
   note asked for, so mail can be encrypted for a device that's been off
   for up to a week.
6. **Quotas and expiry** as named limits, and a defined fallback when a
   holder is full or gone: the message keeps waiting on your devices, as
   today, and nothing is lost.

**Not in v1 of this module**

- **Files.** A file's message travels as mail; the file itself is still
  fetched from a device that has it (dm D12), so it waits for one of them
  to be online. (Q5.)
- Spaces' mail (`spaces` will reuse this, with its own addressing).
- Push notifications to phones.

## Acceptance tests

**The first one, before anything else is built, as a process test with
real helpers and a seed (as `tests/dm.rs`):**

1. **1:1.** Alice and Bob are friends. Bob locks. Alice sends "one" and
   locks. Bob unlocks while Alice is still locked: "one" shows on Bob
   within 5 s of his helper coming online, once. When Alice unlocks again,
   her "one" turns from sending to sent, and Bob doesn't see it twice.
2. **A group.** Alice, Bob and Carol in a group. Bob and Carol lock. Alice
   sends "g1" and locks. Bob unlocks (Alice and Carol still off): he gets
   "g1". Bob locks; Carol unlocks: she gets "g1".
3. **Your own second device.** Alice's laptop has been unlocked once since
   it was linked, then locked. Alice's desk sends Bob "two" and locks. The laptop unlocks while the desk is still off: it
   shows "two" in the chat with Bob. (Bob gets "two" too, by test 1's path.)

Then:

4. **Full holder.** Bob's box on the seed is at its quota. Alice's next
   message isn't lost: the put answers `full`, the message waits on Alice's
   devices (dm D5), and reaches Bob when they're next online together.
5. **Gone holder.** The seed holding Bob's mail is wiped. When Alice's
   devices next see Bob's record, they mail what Bob hasn't acknowledged
   again (D10); it arrives once.
6. **Junk.** A stranger with no pair secret can't put into anyone's box
   (`no-box`); a friend can't put past a box's quota; a key that's used up
   its hour's budget is refused (`slow-down`); a closed seed refuses a put
   without its token.
7. **Expiry.** Mail older than `MAIL_LIFE` is gone from the seed's files; a
   claim not renewed for `CLAIM_LIFE` is gone with its box.
8. **Nothing readable on the holder.** The seed's files and every helper's
   stderr contain no ID, key, conversation ID or message text (the test
   greps them).

## Decisions this spec makes

| # | Decision | Why |
|---|---|---|
| M1 | **Mail is dm D5 with a third party in the middle.** When events wait for a reader's device and no connection to it is open, the sender seals them for that device and puts them on one of the reader's holders. The reader's device collects, opens, stores and acknowledges them as if they'd come live. The sender's counter for that device still moves only on the reader's own `ok` (D5), never on the holder's word. | The event format, the store and the counter don't change (SPEC-dm *What mailbox changes*). A holder that loses mail can delay a message, never make the sender believe it arrived. |
| M2 | **When mail goes out.** Right after an event is stored, for each reader device with no open connection; again at lock and at quit (a short flush, `MAIL_FLUSH`); and at each refresh for anything still waiting and not yet mailed. Live delivery still happens whenever a connection opens; the event ID removes the duplicate. | "Send and go offline" must work when you quit a second later. Mailing at once costs one put per message; holding it back to see whether the reader turns up saves little and loses the quit case. |
| M3 | **A holder is anything that agrees to hold**, named in the reader's own record: a **seed** (by ticket), or a **friend's device** that has offered to hold for its friends (by device key). The reader lists up to `MAX_HOLDERS`, in the order they should be tried. Each device of the reader lists its own holders; by default, our built-in seeds. | Not a fixed list: anyone can run a seed for their friends and point their own record at it, and nobody else has to agree. The reader chooses, because it's the reader who has to collect. |
| M4 | **A seed is the holder of last resort; friends' devices may hold too.** A sender tries the reader's holders in order, but a friend's device only if the sender can reach it (it's the sender's contact too, routing D9). A friend's device holds only if its owner turned on **Hold mail for my friends** (off by default), within `FRIEND_HOLD_BYTES`. | A seed is always reachable; a friend's PC often isn't, and holding costs its owner disk. Friends holding is a bonus, not the plan. |
| M5 | **Boxes.** Mail for a reader sits in a **box** per (sender, reader) pair per week: `box = blake3::derive_key("mesh/v1/mail-box", pair ‖ reader ID ‖ week)` (16 bytes, `pair` from routing D1, `week = unix days / 7`). Inside a box, one **slot** per reader device: `slot = H("mesh/v1/mail-slot", pair ‖ device key)` (8 bytes), as routing D2. Your own devices use the self pair. | Only the sender and reader can compute a box ID, so nobody else can aim mail at it or find it. It changes weekly, so boxes can't be linked across weeks. Slots let each device collect only its own envelopes without telling the holder its key. |
| M6 | **Claims: a holder takes mail only for boxes their owner claimed.** The reader's device claims, at a holder, the boxes for each contact for this week and the next `CLAIM_WEEKS_AHEAD` weeks: `(box, slot, collect tag, expiry)`, where `collect tag = H(collect secret)` and `collect secret = H("mesh/v1/mail-collect", device secret ‖ box)`. Claims are renewed once a day while unlocked, and last `CLAIM_LIFE` from the last renewal. A put into an unclaimed box or slot answers `no-box`. | This is what keeps strangers out: you can only fill a box someone asked for, and only that person's contacts can compute its ID. The collect secret comes from the device's own key, so the sender can't empty or read the reader's box, and neither can the reader's other devices. |
| M7 | **Mail is sealed to the reader's device** (e2ee D8): an Olm envelope inside the sealed-sender wrapper, padded (e2ee D9). Puts and collects go from the throwaway identity (seed D7b) when the holder is a seed. The holder stores `(box, slot, sealed bytes, put time)`. | The holder sees a box, a slot, a padded length and times, nothing else (M12). |
| M8 | **Collecting.** At unlock, and at each refresh while unlocked, each device asks each of its holders for its slots in its boxes for this week and the two before. The holder returns what's there; once the device has stored it, it sends the collect secret and the holder deletes those envelopes. Then it answers `ok` for them to the sender the next time they connect, and at once by **receipt mail** (M9). | Fetch, then delete only after storing: a crash between the two means a duplicate, which the event ID removes, never a loss. |
| M9 | **Receipts are mail too.** After storing collected events, the reader puts a small receipt (the events it took, per sender device) into the sender's box at the sender's holders. The sender collects it at its next unlock, and its counters move as if the `ok`s had come live: "sending" turns to "sent", and nothing is mailed again. | Without receipts, the sender would show "sending" and keep re-mailing until the two are online together, which for people in different time zones may be never. A receipt is one padded 256-byte envelope. (Q1.) |
| M10 | **When a holder is full or gone.** A put that answers `full`, `no-box`, `slow-down`, or doesn't answer within `HOLDER_WAIT`, goes to the reader's next holder. If none take it, the events keep waiting on the sender's devices (dm D5, as today) and are tried again at the next refresh. Mail already on a holder that has lost it is mailed again when the sender sees a reader record that no longer lists that holder, or once `MAIL_LIFE` has passed with no `ok`, within D5's 30 days and 2,000 events. | A full or vanished holder delays mail; it never loses it, because the sender keeps every event until the reader's `ok` (M1). |
| M11 | **The keys-only record** (SPEC-dm's note, now the **mail record**): per (my device, contact), under a weekly lookup `H("mesh/v1/mail-lookup", pair ‖ sender ID ‖ week)`, encrypted as routing D3. It holds the signed device list, the device key, its e2ee bundle and **fallback key** (no one-time key, no address), and the device's **holders** (M3). It lasts `MAIL_RECORD_LIFE` (7 days, the fallback key's life), is put again when the fallback key or the holders change, and otherwise once a day. The getter asks for this week and last. | Mail is encrypted while the reader is away, so the sender can't wait for a fresh address record (2 hours). For a device it already has a session with, the session is enough; for a device it's never spoken to, the fallback key starts one (e2ee D4). The address record is full (about 40 bytes left after e2ee D4), so the holders can't go there; this record has room for `MAX_HOLDERS` tickets. |
| M12 | **What a holder learns, plainly.** A seed sees, per box: that it was claimed, when, by a throwaway session, for how many weeks; each put's padded size and time, and the throwaway session and network it came from; each collect's time and session; when mail is deleted. It **can't** see who anyone is, what's said, which chat, or which of a person's boxes belong together, from its files alone. **A live operator can link sessions**, as seed D10b already says for address records: the session that claims box X and the session that puts into X are contacts. Watched over a week, that's the same graph D10b describes, now with volumes and times. A friend's device that holds sees more: it knows the reader (its friend collects over an authenticated connection) and the sender (who put over one), so it learns *that* they wrote, when and how much, but not what. | Stated so nobody is surprised. It's what any store-and-forward server learns, minus names and content. Padding (e2ee D9) and the throwaway identity keep it to that. |
| M13 | **Claim counts are padded.** A device claims boxes in buckets of `CLAIM_BUCKET` (64): someone with 70 contacts claims 128 boxes a week, the extra ones for random box IDs. | Otherwise the number of claims per session is the number of contacts, which is a fingerprint. (Attack 3 below.) |
| M14 | **Closed seeds.** A seed can be started with `--mail-token <secret>`: it then takes claims and puts only with that token. Its ticket carries the token (`&mail=…`), and the token travels to friends only inside the encrypted mail record. Without the flag a seed is open, as ours are. | Lets anyone run a seed for their own friends without serving the world. A friend could pass the token on; the operator changes it, and the next records carry the new one. |
| M15 | **Stopping junk** on an open seed. (1) Puts only into claimed boxes (M6), within the box's quota (`BOX_MAX_BYTES`, `BOX_MAX_ENVELOPES`), so a friend can fill only your box with them, never anyone else's. (2) A stranger can still claim boxes of their own and fill them: that's bounded by the seed's limits per throwaway key, per network and for everyone through a relay (as seed D8), and by `MAIL_STORE_MAX` in all. (3) **The proof-of-work stamp** (PROPOSAL-seed-cost), if adopted, covers the mail services as it covers the others: one stamp per key per hour, which also buys that key `STAMP_BYTES` of puts in that hour. (4) A seed that's full answers `full`, and mail falls back to waiting (M10): the cost of an attack is delay, not loss. | No holder can tell a stranger from a friend (throwaway keys), so the defence is that a stranger can only spend the seed's space, not aim it at anyone, and only so fast. The stamp turns "free" into "costs a CPU" (the proposal says plainly it's a speed bump); the caps and the fallback are what really hold. |
| M16 | **Nothing identifying is logged**: no IDs, box IDs, slots, keys or tokens, only counts (claims, puts, collects, refusals and bytes held per hour). | Same rule as seed D10 and routing D10. |

### Limits (named constants)

| Name | Value | What |
|---|---|---|
| `MAIL_LIFE` | 14 days | A put envelope is deleted this long after it was put, collected or not |
| `CLAIM_LIFE` | 14 days | A claim lasts this long after its last renewal |
| `CLAIM_WEEKS_AHEAD` | 2 | Weeks claimed beyond this one (covers two weeks away) |
| `CLAIM_BUCKET` | 64 | Claims per device are padded up to a multiple of this |
| `BOX_MAX_BYTES` | 4 MiB | Sealed bytes per box (one sender, one reader, one week) |
| `BOX_MAX_ENVELOPES` | 1,000 | Envelopes per box |
| `MAX_HOLDERS` | 3 | Holders a device lists in its mail record |
| `MAIL_RECORD_LIFE` | 7 days | The mail record's life (the fallback key's) |
| `MAIL_FLUSH` | 3 s | How long lock and quit wait for puts to finish |
| `HOLDER_WAIT` | 10 s | A holder that doesn't answer a put within this is skipped |
| `FRIEND_HOLD_BYTES` | 256 MiB | What a friend's device holds for all its friends together, when turned on |
| `MAIL_STORE_MAX` | 10 GiB | A seed's mail in all, by default (an operator flag changes it) |
| Seed rates | per throwaway key: 4,004 claims and 600 puts an hour, 64 MiB of puts an hour, 120 collects a minute; per network twice that; everyone through a relay together, 100 times the per-key figures | As seed D8 and routing D6, sized for `MAX_CONTACTS` |
| `STAMP_BYTES` | 64 MiB | Bytes of puts one stamp buys a key for its hour, if the stamp is adopted |

## Behaviour

### Sending

1. dm stores the event and numbers it (D5), as now.
2. For each device it goes to (the reader's devices, your own other
   devices) with no open connection, the helper takes what's after that
   device's counter (D5's limits), encrypts it on the session with that
   device (or starts one from the mail record's fallback key), seals it
   (e2ee D8), and puts it into this week's box and the device's slot at the
   first of the reader's holders that takes it (M10).
3. Live delivery is unchanged: when a connection opens, everything after
   the counter goes over it, mailed or not.
4. Own devices get mail only for events authored on this device and for
   read markers; everything else reaches them from its author, by mail or
   live (D8 then fills any gap when two of your devices meet).

### Receiving

1. At unlock and each refresh, each device collects its slots from its own
   holders (M8), opens the envelopes, and hands them to dm as if they'd come
   live from that sender's device: the same checks (D2, D7) and the same
   store.
2. It deletes what it stored (collect secret), and puts receipts (M9).
3. A message that still can't be opened (a session lost to a restore, say)
   is kept 30 days and handled as e2ee D14 says, the next time the two are
   online together.

### Holding for friends (off by default)

The reader chooses its holders (M3), so a friend's device holds for you
only if you list it, and only if its owner agreed. With **Hold mail for my
friends** on, a device answers a friend's `hold-offer` stream with yes and
its quota; the friend's devices then list it. It takes claims, puts and
collects from friends only (it already takes connections only from
contacts, routing D9), over the same services as a seed, within
`FRIEND_HOLD_BYTES` for all its friends together. When full it answers
`full` (M10); it never drops mail it has taken before `MAIL_LIFE`.

## Changes this needs elsewhere (proposals, dated 2026-11-12, not made)

**routing**

- *D3, proposed:* a second record kind, the **mail record** (M11), beside
  the address record: under a weekly lookup (`mesh/v1/mail-lookup`), with
  the same slot, encryption and signing; contents as M11, at most 4,096
  bytes. Put daily and on change, for every contact and the self pair.
- *D5, proposed:* `addr-put` and `addr-get` carry a record kind byte, or two
  new services `mail-rec-put` and `mail-rec-get`; the plan picks.
- *D6, proposed:* the seed may keep a mail record up to 7 days ahead of its
  expiry (address records: 3 hours); counted against the same totals.

**seed**

- *New D, proposed:* services `mail-claim`, `mail-put`, `mail-get` and
  `mail-take` (collect secret, then delete), from the throwaway identity,
  batched as the others.
- *D8, proposed:* the mail rates and caps in *Limits*, and `full` when
  `MAIL_STORE_MAX` is reached.
- *D9, proposed:* mail doesn't fit "one file rewritten on change" (10 GiB):
  one append-only file per box, swept daily; the plan picks the layout.
- *D10b, proposed:* add M12's list of what a seed learns from mail.
- *New flag, proposed:* `--mail-token` (M14) and `--mail-max` for
  `MAIL_STORE_MAX`.

**dm**

- *D5, proposed:* "When a connection to that device opens" becomes "when a
  connection opens, or, with none open, by mail (mailbox M1, M2)". Receipts
  (M9) move the counter as live `ok`s do.
- *D8, proposed:* own devices get by mail only events authored on this
  device and read markers (Sending, step 4).
- *The note "What mailbox changes":* replaced by a pointer here.

**e2ee:** none. D8 already says mail is sealed and leaves the outer
address to `mailbox` (M5).

## Attacking this draft

I tried to break it before showing it, as with dm D2. What came up, and
what changed:

1. **"Send, then quit at once" loses the message.** The first draft mailed
   only at refresh. A quit a second after sending would leave it on the
   desk until the next unlock, failing test 1. *Changed:* mail goes right
   after storing, with a flush at lock and quit (M2). A crash or a pulled
   plug still leaves it waiting; that's stated, not hidden.
2. **The holder's word moves the counter.** If a holder's "stored" moved the
   sender's counter, a holder that drops mail would make a message vanish
   with "sent" showing. *Changed:* only the reader's `ok` moves it, live or
   by receipt (M1, M9).
3. **Claims count your contacts.** One claim per contact per week means the
   seed counts each session's contacts: a fingerprint across weeks.
   *Changed:* claims are padded to buckets of 64 (M13). The week-on-week
   bucket is still visible; I think that's acceptable.
4. **The sender can empty the reader's box.** With a collect secret from the
   pair, the sender (who knows the pair) could fetch and delete. *Changed:*
   the collect secret comes from the reader's own device secret (M6).
5. **A stranger fills a seed with their own boxes.** Claims stop strangers
   aiming mail at *you*, but nothing stops a stranger being a user. *Not
   fully fixable on an open seed:* M15 bounds it by rate, total and (if
   adopted) work, and makes the result delay rather than loss. Closed seeds
   (M14) are the answer for people who want their own.
6. **A friend floods you.** A friend can fill the boxes between the two of
   you, nothing else: per-box quota (M15), and you can block them (social
   D10), which stops your claims for that box next day.
7. **Where do the holders go?** I first put them in the address record. It
   has about 40 bytes left after e2ee D4. *Changed:* they go in the mail
   record (M11), which has room for 3 tickets.
8. **A revoked device collects mail.** The mail record's device list can be
   up to 7 days old, so mail may still be encrypted to a device removed
   this week, and a removed device still holds the root secret (identity's
   open question). *Not fixed here:* it's the same hole as identity's; a
   newer device list in any record or handshake stops new mail to it at
   once (e2ee D11).
9. **Box IDs link a whole week.** A box is the same for 7 days, so a seed
   sees a pair's weekly volume. Daily boxes would hide that, at 7 times the
   claims. *Kept weekly:* Q2.
10. **Group mail is heavy.** A message in a 10-person group with 3 devices
    each is 27 sealed copies through a seed. *Accepted:* each is at most one
    padded bucket, and it's the same work dm D7 already does live.
11. **Mail arrives twice** (by mail and live). The event ID removes it (dm),
    and Olm doesn't mind the same plaintext twice on one session.
12. **Old mail on a wiped seed is never re-sent.** *Changed:* the sender
    mails again when the reader's record stops listing that holder, or after
    `MAIL_LIFE` with no `ok` (M10).
13. **A device that has never claimed gets no mail.** Boxes exist only once
    their device has claimed them, so a new device, or one away longer
    than `CLAIM_LIFE` plus `CLAIM_WEEKS_AHEAD`, gets `no-box` puts until its
    first unlock. *Accepted:* at that unlock it claims, and what it missed
    comes from its own devices (dm D8) and from friends live or by the next
    mail (D5 keeps everything after its counter). Acceptance test 3 needs
    the laptop to have been unlocked once since it was linked; the test
    says so.
14. **Two weeks away is the edge.** Claims reach two weeks ahead and mail
    lasts 14 days, so mail for someone away longer than about two weeks
    isn't held: it waits on the sender (M10), as everything does today.
    *Accepted for v1;* `CLAIM_WEEKS_AHEAD` is the knob (Q2's cost grows
    with it).
15. **A friend's holder drops mail when full.** The first draft dropped the
    oldest. That loses mail the sender thinks is held, and only the re-mail
    after `MAIL_LIFE` would recover it. *Changed:* a full friend's device
    answers `full`, like a seed, and keeps what it has.

## Testing strategy

- Unit tests in a new `mesh-mail` module (or inside `mesh-dm`; the plan
  picks): box and slot derivation, claim padding, quotas, expiry, the
  collect secret, receipts moving counters.
- Seed tests for each service and limit, and `full`.
- Process tests: acceptance tests 1 to 8, with real helpers and a seed, as
  `tests/dm.rs`; Mesh-Voice's checkpoint test gains test 1 driven through
  the app.

## Boundaries

- **Always:** the counter moves only on the reader's `ok`; nothing
  identifying in logs; every limit a named constant.
- **Ask first:** a new dependency; a change to any limit above; anything
  that lets a holder see more than M12 says.
- **Never:** unsealed mail on any holder; a holder that decides who may
  collect from anything but the collect tag; mail that skips the reader's
  own checks (dm D2, D7).

## Open questions, with what I recommend

1. **Receipts by mail (M9)?** Recommend **yes**: without them, people in
   different time zones show "sending" for ever and keep re-mailing.
2. **Boxes weekly or daily (M5, attack 9)?** Recommend **weekly**: daily is
   7 times the claims for a weekly-volume leak that the operator-linking in
   M12 mostly gives away anyway.
3. **Claim padding bucket (M13)?** Recommend **64**: cheap (at most 63
   empty claims), and enough to blur most contact counts.
4. **Friends' devices as holders in v1 (M4)?** Recommend **yes, opt-in, off
   by default**, with groups not holding for each other automatically yet.
   A group member's device could hold for the others without being asked,
   since they're all friends (dm D6), but that's a privacy step (it learns
   who's away) I'd rather you choose later.
5. **Files on holders?** Recommend **later**: v1 carries the file's message,
   and the file waits for a device that has it. Holding files means seeds
   holding up to 100 MB each, which changes `MAIL_STORE_MAX` and the junk
   story.
6. **How many holders per message?** Recommend **one**, the next only on
   failure (M10). Two would survive a holder dying, at twice the space; the
   re-mail rule already covers that, later.
7. **The proof-of-work stamp.** Mailbox doesn't need a separate one.
   Recommend: **adopt the proposal's per-key-per-hour stamp for all seed
   services, mail included, with `STAMP_BYTES` per stamp, before mail opens
   on public seeds**; closed seeds (M14) can run without it.
8. **Closed seeds (M14)?** Recommend **yes**: it's a flag and a ticket field,
   and it's what makes "anyone can run a seed for their friends" safe.
9. **A "held" mark in Mesh-Voice?** Recommend **no**: with receipts,
   "sending" turns to "sent" once the reader has it, which is what people
   care about. A tooltip "waiting for Bob to come online" could come later.
10. **Seed storage for mail (seed D9)?** Recommend one append-only file per
    box, swept daily; the plan decides.
