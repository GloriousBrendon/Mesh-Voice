# Spec: `mailbox` (Mesh)

Status: **APPROVED 2026-11-12 with four changes, made here as revision 3
(2026-11-12), for your review** with [PLAN-mailbox.md](PLAN-mailbox.md). No
code until both are approved. Revision 3 changes the batch order (M8),
reads every contact's mail record ahead of need (M11), collects last week at
a refresh, and replaces healing by silence with a "couldn't open" list in
receipts (M9, M16). Two things in it are new and not yet approved: the pair
tag on the wrapper (M7) and a test flag to cut connections (tests 11, 18);
see *Open questions*.
Module 8 of the build order in [CAPABILITY-MAP.md](CAPABILITY-MAP.md).
Depends on [`routing`](SPEC-routing.md), [`seed`](SPEC-seed.md),
[`e2ee`](SPEC-e2ee.md) and [`dm`](SPEC-dm.md) (all built). The changes it
makes to them are written into those specs, dated 2026-11-12 (*Changes made
elsewhere*).

## Objective

Today a message reaches someone only while one of your devices and one of
theirs are online at the same time (dm D5). Send a message and quit, and a
friend who comes online an hour later sees nothing until you're both back.
Discord never has that problem, because its servers hold the message.

`mailbox` adds **holders**: nodes that agree to keep sealed mail for someone
who's away and hand it over when they come back. A holder can't read the
mail, can't tell who it's from, and can't be made to hold mail for anyone
who didn't ask it to.

**The promise:** mail is held for someone who's away **up to 30 days**, the
same 30 days dm D5 keeps trying for. Every limit below is derived from it
(*Limits*). It holds while the reader's holder has room; when it doesn't, the
message waits on the sender's devices, as it does today, and is never lost.

**What this module delivers**

1. **Offline delivery, 1:1**: you send and go offline; your friend comes
   online later, while you're still off, and gets it.
2. **The same for a group**: each member who comes online later gets it.
3. **The same for your own other device.**
4. **Holders that anyone can run.** Your record tells your friends which
   holders keep your mail; a seed can be closed to everyone but its
   operator's friends (M14).
5. **Keys for someone who's been away**: the keys-only record from
   SPEC-dm's note, now the **mail record**, read for every contact ahead of
   need and remembered after it lapses (M11).
6. **Quotas and expiry** as named limits; a full or gone holder delays mail
   and never loses it.

**Not in v1 of this module**

- **Friends' devices as holders.** M3's wording allows them so they can be
  added later; v1's holders are seeds (decided Q4).
- **Files.** A file's message travels as mail; the file itself is still
  fetched from a device that has it (dm D12) (decided Q5).
- **The proof-of-work stamp** (decided Q7): not for the beta. Built-in seeds
  run with a small `MAIL_STORE_MAX`; the stamp is decided before a public
  release.
- Spaces' mail, and push notifications to phones.

## Acceptance tests

Process tests with real helpers and a seed, as `tests/dm.rs`. "Away" means
locked. "The clock moves" means the helpers and the seed run with
`--clock-offset` (decided Q14), not by waiting. "Cut" means a test-only
switch that stops one helper's connections of one kind (the plan names it).

**The first, before anything else is built:**

1. **1:1.** Alice and Bob are friends. Bob locks. Alice sends "one" and
   locks. Bob unlocks while Alice is still locked: "one" shows on Bob within
   5 s of his helper coming online, once. When Alice unlocks, her "one" is
   "sent" (Bob's receipt), it isn't mailed again, and Bob doesn't see it
   twice.
2. **A group.** Alice, Bob and Carol. Bob and Carol lock. Alice sends "g1"
   and locks. Bob unlocks (Alice and Carol still off): he gets "g1". Bob
   locks; Carol unlocks: she gets "g1".
3. **Your own second device.** Alice's laptop has been unlocked once since
   it was linked, then locked. Her desk sends Bob "two" and locks. The
   laptop unlocks while the desk is still off: it shows "two" in the chat
   with Bob.

Then:

4. **Full holder.** Bob's box is at its quota. Alice's next message waits on
   her devices (`full`), and reaches Bob when they're next online together.
5. **Wiped holder.** Alice mails Bob "three" and locks. The seed's mail is
   wiped. Bob unlocks: he finds his claims gone, claims again, and his mail
   record says the seed's claims are good only since now. Alice unlocks:
   she reads that, and mails "three" again at once. Bob gets it once.
6. **Junk.** A stranger can't put into anyone's box (`no-box`); a friend
   can't put past a box's quota; a closed seed refuses a claim or put
   without its token; a key over its hour's limits gets `slow-down`.
7. **Expiry.** Mail older than `MAIL_LIFE` and claims not renewed for
   `CLAIM_LIFE` are gone from the seed's files.
8. **Nothing readable on the holder.** The seed's files and every helper's
   stderr contain no ID, key, conversation ID or message text (greps).
9. **Away 10 days.** Bob locks. Alice sends "ten" and locks. The clock
   moves 10 days; Bob's mail record has lapsed from the seed. Bob unlocks:
   he gets "ten".
10. **Away 25 days.** As test 9, 25 days. Also: Alice sends "late" on day
    20, from her memory of Bob's holders and session (M11), with Bob's
    record long gone; Bob gets both on day 25.
11. **`failed` by mail.** A group of Alice and Bob; Bob is away. Alice adds
    Dave with her seed connection cut, so her add can't be mailed to Bob
    yet; Dave gets it live, sends "hi from Dave" (mailed to Bob) and locks.
    Bob unlocks and collects Dave's message with no add: after one more try
    at the end of the batch, he answers `failed` by receipt. Alice's
    connection comes back and her add is mailed; Bob collects it at his
    next refresh. Dave unlocks, collects the `failed` and mails his message
    again at once; Bob shows it. No `MAIL_LIFE` wait.
12. **An add and a message in one collect.** As test 11, but Alice's add is
    mailed before Bob unlocks, so both are collected in one collect, Dave's
    put first: Bob shows Dave's message straight away, and answers no
    `failed`.
13. **Out of order.** Alice mails Bob 60 messages, then the two meet live
    and Alice sends them again on the live session; Bob collects the mail
    afterwards. Every message shows once; the mail Bob can't open any more
    is dropped quietly, starts no session repair, and leaves no error. His
    receipt lists it as couldn't-open, and the ones he could open as `ok`
    (duplicates); Alice starts no fresh session, because every one already
    has an `ok`.
14. **A broken session heals by mail.** Bob's stored session with Alice is
    deleted, as a damaged session file would be (a device restored from a
    backup is a new device, with its own sessions). Alice, away, mails him
    "four" and locks; Bob can't open it, drops it, and his receipt lists it
    as couldn't-open. Bob locks. Alice unlocks, collects the receipt and
    mails "four" again at once on a fresh session. Bob unlocks and gets it.
    The two are never online together.
15. **No re-mail on unlock.** Alice mails Bob 20 messages and locks; she
    unlocks again before Bob is back: nothing is mailed again.
16. **A removal and earlier messages in one collect.** A group of Alice,
    Bob and Dave, made by Alice. Bob locks. Over five days (the clock moves)
    Dave sends "d1" to "d5", mailed to Bob, and locks. Alice removes Dave and
    locks. Bob unlocks and collects all six in one collect: "d1" to "d5"
    show, then the removal; nothing is refused or answered `failed`.
17. **First mail from a record read ahead.** Alice and Bob are friends and
    have never written to each other (no session). On day 0 both are
    unlocked long enough for Alice to read Bob's mail record, then both
    lock. On day 20 Alice unlocks (Bob's record has long lapsed) and sends
    "first": it's mailed, on a session started from the remembered fallback
    key. On day 25 Bob unlocks and gets it.
18. **A refresh over a week's edge.** Bob stays unlocked across the end of a
    week (the clock moves to a minute before it), with his live connection
    to Alice cut. Alice mails him "edge" just before the edge; Bob's next
    refresh comes after it, and he gets "edge" from last week's box.

## Decisions this spec makes

| # | Decision | Why |
|---|---|---|
| M1 | **Mail is dm D5 with a holder in the middle.** When events wait for a reader's device with no open connection, the sender seals them for that device and puts them on one of the reader's holders. The reader collects, opens, stores and acknowledges them as if they'd come live. The sender's counter moves only on the reader's own `ok`, live or by receipt (M9), never on the holder's word. | The event format, store and counter don't change. A holder that loses mail can delay a message, never make it look delivered. |
| M2 | **When mail goes out.** Right after an event is stored, for each reader device with no open connection; at lock and at quit (a flush, `MAIL_FLUSH`); and at each refresh for anything waiting and not yet mailed. Live delivery still happens whenever a connection opens; the event ID removes duplicates. | "Send and quit" must work a second after sending. A crash still leaves it waiting on the sender. |
| M3 | **A holder is anything that agrees to hold, named in the reader's own record.** A holder is given by ticket (a seed) or by device key (a friend's device, *not in v1*). The reader lists up to `MAX_HOLDERS`, in the order to try; by default our built-in seeds. | Not a fixed list: anyone can run a seed for their friends and point their own record at it. The reader chooses, because the reader collects. Devices can join the list later without changing the format. |
| M4 | **One holder per message** (decided Q6), the next only on failure (M10). | Twice the space bought little; M10's re-mail covers a lost holder. |
| M5 | **Boxes.** Mail sits in a box per (sender, reader) pair per week: `box = blake3::derive_key("mesh/v1/mail-box", pair ‖ reader ID ‖ week)`, 16 bytes, `week = unix days / 7`, `pair` from routing D1 (the self pair for your own devices). Inside it, one slot per reader device: `slot = H("mesh/v1/mail-slot", pair ‖ device key)`, 8 bytes. Weekly (decided Q2). | Only the two people can compute a box. It changes weekly, so a seed can't link a pair across weeks. Slots let a device collect only its own envelopes. |
| M6 | **Claims.** A holder takes mail only into boxes and slots its owner claimed: `(box, slot, collect tag, expiry)`, `collect tag = H(collect secret)`, `collect secret = H("mesh/v1/mail-collect", device secret ‖ box)`. A device claims, for each contact and the self pair, this week and the next `CLAIM_WEEKS_AHEAD`, and renews daily while unlocked; a claim lasts `CLAIM_LIFE` after its last renewal. A put into an unclaimed slot answers `no-box`. | Strangers can't aim mail at you: they can't compute your boxes, and nothing unclaimed is held. The collect secret comes from the device's own key, so neither the sender nor your other devices can empty your slot. |
| M7 | **Mail is sealed to the reader's device** (e2ee D8): an Olm envelope, padded (e2ee D9), inside the sealed-sender wrapper. Inside the seal, before the Olm envelope, the wrapper carries **the sender device's key, the kind (mail or receipt), the mail sequence number `mseq`** (one counter per sender device and reader device) and a **pair tag**: `blake3::keyed_hash(derive_key("mesh/v1/mail-tag", pair), sender device ‖ kind ‖ mseq ‖ envelope)`, 16 bytes. *(The pair tag is new in revision 3.)* A wrapper whose sender device isn't a current device of a contact, or whose tag doesn't match that contact's pair, is dropped and listed nowhere. Puts, claims and collects go from the throwaway identity (seed D7b). | The holder sees a box, a slot, a padded length and times. `mseq` lets the reader open each sender's mail in order (M8). The seal alone proves nothing about the sender: anyone with the reader's device key can make one. The tag is what lets the reader trust "this is Alice's `mseq` 12" before Olm opens it, which M16 needs. Only the pair's own devices can make it; a seed can't. |
| M8 | **Opening collected mail.** See *Opening mail*: each sender device's mail opened in `mseq` order; the opened events handed to dm in **dm D2's order across senders**, with one more try at the end of the batch for "not a member yet"; mail before the live backlog; mail that can't be opened dropped quietly with no session repair by the reader. *Changed in revision 3 (your review): "group changes first" is gone, for mail and for live. `group_refuses` judges an author against the group as it stands when the event is handled, and someone removed is refused for good, so a removal applied first drops the removed member's earlier messages (test 16).* | vodozemac keeps 40 skipped keys per chain and five chains; opened out of order, mail fails. Applying in D2's order is applying in the order the events were written, which is the order the group checks assume. |
| M9 | **Receipts are mail, carrying `ok`, `failed` and couldn't-open** (decided Q1). After storing collected events the reader puts a receipt into the box the sender collects (M5, with the sender as reader), at the sender's holders from its newest mail record (M11): per sender device, the `mseq` values it answered `ok`, those it answered `failed` (dm's `failed`: not yet a member, a newer version), and those it **couldn't open** (M16). A duplicate collected by mail is answered `ok` again, as live. A receipt is sealed and tagged (M7) with no Olm inside, so a broken session can't stop it, and is never itself answered. On collecting it, the sender moves its counters on the `ok`s, as live; a `failed` is mailed again at once, then on `FAILED_BACKOFF`, within D5's 30 days. | Without receipts, people in different time zones show "sending" for ever. A `failed` must come back soon, not after `MAIL_LIFE`: it usually means "send it again once the add has reached me". |
| M10 | **Full or gone holders.** A put answered `full`, `no-box` or `slow-down`, or unanswered for `HOLDER_WAIT`, goes to the reader's next holder; if none take it, the event keeps waiting on the sender (D5) and is tried again at the next refresh. Mail on a holder that lost it is mailed again by the **`since` rule** (M12). | A full or vanished holder delays mail; the sender keeps every event until the reader's `ok`. |
| M11 | **The mail record, read ahead and remembered.** Per (my device, contact), under a weekly lookup `H("mesh/v1/mail-lookup", pair ‖ sender ID ‖ week)`, encrypted and signed as routing D3: the signed device list, device key, e2ee bundle and fallback key (no one-time key, no address), and the device's holders, each with its **`since`** (M12). It lasts `MAIL_RECORD_LIFE` (7 days) and is put at unlock, daily and on any change. **Each device reads every contact's mail record at unlock and daily** (`MAIL_RECORD_READ`): this week's lookup for everyone, then last week's only for contacts with nothing this week. *(Changed in revision 3: before, a record was read only when mailing, so someone who first wrote on day 8 had nothing.)* **It remembers, per contact device, the newest record it has read** (holders, `since`, device key, fallback key), in its sealed store, for `REMEMBER_LIFE` after that record lapses, and keeps its Olm session with that device as long. A read that finds nothing leaves the memory as it is. Mail goes from the newest record, read or remembered. | The record can't last 30 days: its device list would be stale. But a device that's away doesn't change its holders or rotate its fallback key (`FALLBACK_LIFE`, 7 days in `mesh-e2ee`, renews only when the device publishes records, so only while unlocked), so what the sender read last is still right while the reader is away. Reading ahead means the sender has it whenever it first needs it (test 17). A device the sender has never read a record for, and has no session with, can't be mailed: it waits (D5). |
| M12 | **The `since` rule (a wiped holder).** For each holder, the mail record says since when that holder's claims are good: the time of the device's first claim there that's still standing. A device that finds its claims gone (a renewal or collect answered `no-box`) claims again and sets `since` to now. A sender that reads a `since` later than a put it made there that has no `ok` mails that event again at once, to the reader's holders as listed now. The sender keeps, per event and reader device, the holder, time and `mseq` of its last put (M15). | The reader sees a wipe first, because its own claims are gone. A sender has no other way to know. Without this, a wiped holder still listed means nothing is mailed again for `MAIL_LIFE`. |
| M13 | **Padding hides how many boxes are real.** Each device claims, renews and collects in buckets of `CLAIM_BUCKET` (64) boxes per week (decided Q3): someone with 70 contacts handles 128. The extra boxes are **dummies**: `H("mesh/v1/mail-dummy", device secret ‖ i ‖ week)`, stable across a week like real ones, claimed, renewed and collected exactly as real ones are. | If dummies changed at each renewal, or weren't collected, the seed would see which claims are real. What it doesn't hide is in M17. |
| M14 | **Closed seeds** (decided Q8). `--mail-token <secret>`: the seed takes claims and puts only with that token. Its ticket carries it (`&mail=…`), which reaches friends only inside the encrypted mail record. Without the flag a seed is open. | Lets anyone run a seed for their own friends without serving the world. A token passed on is changed by the operator; the next records carry the new one. |
| M15 | **The "already mailed" mark is saved.** Per (event, reader device): the holder, put time and `mseq` of its last put, kept with dm's counters in the sealed log index, saved once per batch of puts. Unlock reads it; only events with no mark, or that M9, M10, M12 or M16 says to mail again, are put. | Without it every unlock re-mails everything still waiting. The `mseq` is how a receipt's lists map back to events. |
| M16 | **Couldn't open, by receipt.** The reader unseals the wrapper and checks its tag (M7) before Olm opens, so it knows the sender device and `mseq` of mail it can't open, and lists them in its receipt (M9). The reader does nothing else: no session repair. The sender, for each listed `mseq` its marks (M15) say it put to that device: **ignores it if the event already has an `ok`**; otherwise mails the event again at once **on a fresh outbound session** from the newest fallback key it holds for that device (read or remembered, M11), which replaces the old session for everything after. At most one fresh session per reader device per `FRESH_SESSION_MIN` (1 hour, e2ee D4's limit), and never twice from the same fallback key: a further couldn't-open waits for a newer record. *Changed in revision 3 (your review): replaces `HEAL_AFTER` and the reading of the record's `created`.* | Revision 2 said the reader couldn't tell whose mail it couldn't open; with the tag it can. Most couldn't-opens are only order (test 13): those events already have an `ok` and nothing happens. A truly broken session heals in one round of mail, with the two never online together (test 14). The limits stop a stale fallback key from starting a session every hour for a month. |
| M17 | **What a seed learns, plainly.** Per box: when it was claimed and renewed, by a throwaway session; each put's padded size and time, and the session and network it came from; each collect and delete, with its session. It can't see who anyone is, what's said, which chat, or which boxes belong together, from its files. Padding hides how many contacts a session has from **a holder that isn't also its record seed**: the seed you put address and mail records on already sees one put per contact (routing D4, M11). It hides **not which of its boxes are getting mail this week**: dummies never get puts. A live operator can link sessions, as seed D10b already says for address records: the session that claims box X and the session that puts into X are contacts; over a week, that graph with volumes and times. Receipts show it both ways. | Stated so nobody is surprised. It's what any store-and-forward server learns, minus names and content. |
| M18 | **Junk on an open seed.** Puts only into claimed slots within the box's quota, so a friend can fill only the boxes between you two. A stranger can still claim boxes of their own and fill them: bounded by the seed's rates per throwaway key, per network and through a relay, and by `MAIL_STORE_MAX`. A full seed answers `full`, and mail waits on the sender (M10): an attack costs delay, not loss. No stamp for the beta (decided Q7); the built-in seeds run with a small `MAIL_STORE_MAX`. If the stamp proposal is adopted later, mail services take the same per-key-per-hour stamp as the others. | No holder can tell a stranger from a friend. The defence is that a stranger can spend only the seed's space, not aim it at anyone, and only so fast. |
| M19 | **Nothing identifying is logged**: counts only (claims, puts, collects, couldn't-opens, refusals, bytes held per hour). | As seed D10 and routing D10. |

### Opening mail (M8)

vodozemac 0.11.2 (`src/olm/session/receiver_chain.rs`, `session/mod.rs`),
checked 2026-11-12:

- **`MAX_MESSAGE_GAP = 2000`**: a message more than 2,000 steps ahead in its
  receiving chain is refused (`TooBigMessageGap`).
- **`MAX_MESSAGE_KEYS = 40`** per receiving chain: when a message skips
  ahead, only the 40 keys just before it are kept; anything older that
  arrives later fails (`MissingMessageKey`).
- **`MAX_RECEIVING_CHAINS = 5`**: only the last five receiving chains are
  kept; a message on an older one fails.

So a device opens in this order:

1. **Collect first.** At unlock, a device collects from all its holders
   before it takes the backlog of a live connection. Live envelopes that
   arrive meanwhile wait, up to `HOLDER_WAIT`.
2. **Unseal and check every wrapper** (M7); drop those that fail. Group the
   rest by sender device, sort by `mseq`, and open each sender device's
   envelopes with Olm in that order. Mail from several holders merges by
   `mseq`, so which holder it came from doesn't matter. This order is for
   Olm only.
3. **What can't be opened is dropped quietly** and listed as couldn't-open
   in the receipt (M16). A `MissingMessageKey`, `TooBigMessageGap` or
   old-chain failure on mail usually means the sender re-encrypted it since
   (it went live, or was mailed again). It's counted in the log and **never
   starts a session repair on the reader's side** (e2ee D4's fresh-session
   answer is for live first messages only). A fresh session from the sender
   arrives as a first message, which opens.
4. **Hand the opened events to dm as one batch, in D2's order:** each chat's
   events sorted by (`seq`, event ID), whoever sent them. dm applies them in
   that order, as it applies a live batch. **An event answered "not a member
   yet" gets one more try at the end of the batch**, after everything else
   in it; if it's still refused, it goes back `failed` by receipt (M9).
5. **Then the live backlog**, as today. Its duplicates of mail just opened
   are removed by event ID.

Live delivery is unchanged: no reordering, no retry at the end.

### Limits (named, derived from the 30-day promise)

| Name | Value | Derived from |
|---|---|---|
| `MAIL_PROMISE` | 30 days | dm D5's 30 days |
| `MAIL_LIFE` | 30 days | An envelope is kept this long after its put: the promise |
| `CLAIM_WEEKS_AHEAD` | 5 | Weeks claimed beyond this one: ⌈30 / 7⌉ + 1, so a box exists for every day of 30 away |
| `CLAIM_LIFE` | 35 days | 30 days away plus the 5 days of a partial first week; mail put on day 30 is still collectable |
| `COLLECT_WEEKS_BACK` | 5 at unlock, 1 at a refresh | Unlock: the promise. Refresh: last week too, so a device unlocked over a week's edge misses nothing (test 18) |
| `MAIL_RECORD_LIFE` | 7 days | The fallback key's week |
| `MAIL_RECORD_READ` | at unlock and every 24 hours | A sender holds every contact's record before it first needs it (M11) |
| `REMEMBER_LIFE` | 30 days | The promise: a sender keeps a lapsed record and the session this long |
| `CLAIM_BUCKET` | 64 | Decided Q3 |
| `BOX_MAX_BYTES` | 4 MiB | Per box (one sender, one reader, one week) |
| `BOX_MAX_ENVELOPES` | 1,000 | Per box; D5's 2,000 events over the weeks they span |
| `MAX_HOLDERS` | 3 | Fits the mail record |
| `MAIL_TAG` | 16 bytes | The pair tag (M7) |
| `MAIL_FLUSH` | 3 s | Lock and quit wait this long for puts |
| `HOLDER_WAIT` | 10 s | A holder silent this long is skipped |
| `FAILED_BACKOFF` | 30 min, doubling to 1 day | A `failed` is mailed again at once, then after these |
| `FRESH_SESSION_MIN` | 1 hour | e2ee D4's once an hour per device (M16) |
| `MAIL_STORE_MAX` | 2 GiB on the built-in seeds for the beta (decided Q12); operator flag `--mail-max` | Claims, mail records and mail count against it together |
| Seed rates | per throwaway key, an hour: 8,192 claims or renewals; 16,384 collect queries; 600 puts and 64 MiB of puts; 2,048 mail-record puts; 4,096 mail-record lookups. Per network twice that; everyone through a relay, 100 times the per-key figures | Sized for `MAX_CONTACTS` (1,000): 1,001 boxes padded to 1,024 × 6 weeks is 6,144 claims, and an unlock's 6,144 collect queries plus two refreshes of 2,048 is 10,240; one record per contact, and up to two lookups each at a read |

### What it costs

Assumptions: one holder; one device per contact; a typical mail record of
about 1.1 KB (two devices' list, the e2ee field, three holders); a claim of
64 bytes (box, slot, tag, expiry), a renewal or collect query of 24 bytes
(box, slot) and a record lookup of 16; frames and seals are included in
round figures. "Round trips" are on one connection to the seed, which is
already open for address records. *Revision 3 adds the record reads, last
week in refresh collects, and the records on the seed, which revision 2
left out.*

| | 100 contacts | 1,000 contacts |
|---|---|---|
| Boxes per week (with the self pair, padded to 64) | 128 | 1,024 |
| **At unlock** | | |
| Collect: this week and 5 back (first, before anything below) | 1 round trip, 768 queries, 18 KB up, plus any mail down | 1 round trip, 6,144 queries, 147 KB up |
| Claims: 6 weeks (first unlock only; later the daily renewal covers them) | 1 round trip, 768 claims, 49 KB | 1 round trip, 6,144 claims, 393 KB |
| Mail records read (M11) | 1 round trip (2 at a week's start), 101 lookups, 2 KB up, about 110 KB down | 1 to 2 round trips, 1,001 lookups, 16 KB up, about 1.1 MB down |
| Mail records put | 1 round trip, 110 KB up | 1 round trip, 1.1 MB up |
| **Per day** | | |
| Renewals (daily) | 1 round trip, 768, 18 KB | 1 round trip, 6,144, 147 KB |
| Refresh collects (this week and last, every 30 min, 48 a day) | 48 round trips, 256 each: 295 KB a day | 48 round trips, 2,048 each: 2.4 MB a day |
| Mail records put (daily) | 110 KB | 1.1 MB |
| Mail records read (daily) | about 110 KB | about 1.1 MB |
| Remembered records in the sealed store | about 110 KB | about 1.1 MB |
| Saved "mailed" marks (M15) | about 24 bytes per waiting event and device | the same |
| **On the seed, per person** | claims 49 KB, records about 220 KB (this week's and last week's): **about 270 KB** | claims 393 KB, records about 2.2 MB: **about 2.6 MB** |

At 1,000 contacts the daily total is about **4.7 MB**, against routing's
existing address records of about 24 MB a day at that size (1,000 × 500 B ×
48). At 100 it's about **530 KB**. A beta seed's 2 GiB holds claims and
records for about **800 people of 1,000 contacts, or 7,900 of 100**, before
any mail. Revision 2 said 5,000 and 40,000; it had left the records out.

**Test 1's 5 seconds.** Bob's helper is online when its relay connection is
up (measured: a cold ping by ID took 0.64 s on n0's relays, routing smoke
2026-11-09). His first seed connection opens for address records anyway;
the collect is the first thing sent on it, one round trip: 768 queries,
18 KB, answered with one envelope. Bob needs nothing of Alice's to open it:
it's a first message on his own keys, or the next on their session.
Opening and storing it is under 10 ms (dm's intake: 10,000 events in 1.5 s).
So "one" shows about 1 to 1.5 s after the helper is online, over the
internet with a 100 ms round trip; 5 s leaves room for a slow relay. At
1,000 contacts the collect is 147 KB up, about 1 s more on a slow uplink:
still inside 5 s. The record read and puts (about 2.2 MB) go after it and
don't hold it up.

## Behaviour

### Sending

1. dm stores the event and numbers it (D5).
2. For each device it goes to with no open connection, the helper takes
   what's after that device's counter (D5's limits) and not already marked
   mailed (M15), encrypts it on the session with that device (or starts one
   from the record's fallback key), seals and tags it with the next `mseq`
   (M7), and puts it into this week's box at the first of the reader's
   holders that takes it (M10). The reader's holders and keys come from the
   newest mail record, read or remembered (M11).
3. Live delivery is unchanged.
4. Own devices get mail only for events authored on this device and for
   read markers; everything else reaches them from its author.

### Receiving

1. At unlock: collect (this week and `COLLECT_WEEKS_BACK`), open as *Opening
   mail* says, store, delete (collect secret), put receipts (M9). Then read
   every contact's mail record (M11), act on receipts and `since` (M9, M12,
   M16), and put this device's records. Renew claims if due; if a renewal
   or collect answers `no-box`, claim again and move that holder's `since`
   (M12).
2. At each refresh: collect this week and last, the same way. Once a day,
   read the records again.

### Choosing your own holder (v1)

`--holder <ticket>` (repeatable, up to `MAX_HOLDERS`) or `MESH_HOLDERS`
(space-separated), on the helper, as `--seed` and `MESH_SEEDS` are. The
device lists those in its mail record, in that order; with neither set, the
built-in seeds. Each device sets its own. No screen in v1 (PLAN-mailbox
says why).

## Changes made elsewhere (2026-11-12)

Approved with the spec, less "changes first", and written into each spec
under a dated heading:

- **routing** (*Changed 2026-11-12 by `mailbox`*): D3 the mail record as a
  second record kind; D5 two services, `mrec-put` and `mrec-get`; D6 its
  limits and its 7 days ahead.
- **seed** (*Changed 2026-11-12 by `mailbox`*): the mail services; D8's mail
  rates and `full`; D9 the mail store's layout; D10b M17; the flags
  `--mail`, `--mail-token`, `--mail-max`, `--clock-offset`.
- **e2ee** (*Changed 2026-11-12 by `mailbox`*): D4's fresh-session answer is
  live only; D14's keep and `key-request` don't apply to mail; D8's wrapper
  carries the sender device, kind, `mseq` and the pair tag (the tag pending
  your review).
- **dm** (*Changed 2026-11-12 by `mailbox`*): D5 mail and receipts; D5's
  saved index with marks and remembered records; D8 what own devices get by
  mail; receiving, a mailed batch in D2's order with one more try at the
  end; the note "What `mailbox` changes" now points here.

## Attacking revision 3

Revisions 1 and 2's attacks still stand as answered there, now in M1 to
M19, except revision 2's 2 and 3 (healing on absence, healing from a stale
record), which M16's new form replaces. New ones:

1. **A removed member back-dates.** Dave, removed, sends with a `seq` from
   before his removal, and it reaches Bob in the same collect as the
   removal: sorted by D2, it's taken. *Accepted, not new:* live, the same
   message reaching Bob before the removal is taken too. Once Bob holds the
   removal, anything from Dave is refused, whatever its `seq`. D2's
   one-day-ahead bound doesn't stop back-dating, and never did.
2. **"Not a member yet" at the end of a batch, still not.** The add is in a
   later collect. *Covered:* `failed` by receipt, mailed again at once (test
   11).
3. **A forged couldn't-open.** Could someone make Alice start fresh
   sessions with Bob, or re-send? *No, short of the pair:* the list arrives
   only in a receipt with Bob's pair tag, which the seed and Bob's other
   friends can't make; and Alice acts only on `mseq` values she put that
   have no `ok`, which she'd re-send anyway. The pair's own devices could
   churn sessions, at most once an hour.
4. **A fresh session from a stale fallback key.** Bob rotated twice while
   Alice was away (vodozemac keeps the current and previous fallback key),
   so her remembered key is gone: the fresh session fails too. *Bounded:*
   M16 never starts a second from the same key; the next daily read brings
   Bob's newest, which works.
5. **A receipt itself can't be opened.** It's sealed with no Olm, so only a
   bad seal or tag stops it, and then it was never Bob's: dropped.
6. **The record seed counts your contacts.** Reading every record daily
   shows that seed one lookup per contact. *Not new:* address-record puts
   already show it one put per contact (routing D4). Stated in M17: padding
   hides the count only from a holder that isn't your record seed, such as
   your own seed holding mail while records go to the built-in ones.
7. **Reading ahead costs.** 1.1 MB a day at 1,000 contacts. *In the table.*
   Last week's lookup only for misses halves it.
8. **A read that finds nothing wipes the memory.** Then test 17 fails.
   *Changed:* nothing found leaves the memory (M11).
9. **The beta store fills with records, not mail.** 2.6 MB a person at
   1,000 contacts. *Stated:* about 800 such people per 2 GiB; the beta has a
   handful. `MAIL_STORE_MAX` counts records too, so a full seed answers
   `full` and mail waits on the sender.
10. **A week's edge on a refresh.** *Changed:* a refresh collects last week
    too (test 18).
11. **The end-of-batch retry hides a real refusal.** It's one more call into
    dm with the same checks; a refusal other than "not a member yet" isn't
    retried.

## Testing strategy

- Unit tests: box, slot and dummy derivation; claim padding; quotas and
  expiry; the collect secret; the pair tag; receipts with `ok`, `failed`
  and couldn't-open; `mseq` ordering across holders; D2 order across
  senders and the end-of-batch retry; the `since` rule; the saved marks;
  the fresh-session limits; reading ahead and remembering.
- Seed tests for each service, limit, token and `full`.
- Process tests: acceptance tests 1 to 18, with a moved clock for 9, 10, 16,
  17 and 18; Mesh-Voice's checkpoint test gains test 1 through the app.

## Boundaries

- **Always:** the counter moves only on the reader's `ok`; logs hold counts
  only; every limit a named constant.
- **Ask first:** a new dependency; a change to any limit; anything that
  lets a holder see more than M17 says; turning mail on at the home seed.
- **Never:** unsealed mail on a holder; collecting without the collect
  secret; mail that skips the reader's own checks (dm D2, D7).

## Decided questions (2026-11-12)

1. Receipts by mail: **yes, carrying `failed` too** (M9).
2. Boxes: **weekly** (M5).
3. Claim padding: **64, covering collects and renewals** (M13).
4. Friends' devices as holders: **later**; out of v1, M3's wording kept.
5. Files on holders: **later**.
6. Holders per message: **one**, with the `since` rule (M4, M12).
7. Proof of work: **not for the beta**; a small `MAIL_STORE_MAX` on the
   built-in seeds; decided before a public release (M18).
8. Closed seeds: **yes** (M14).
9. A "held" mark: **no**.
10. Seed storage for mail: **the plan decides** (PLAN-mailbox).
11. Group changes first for live batches: **no**, and not for mail either
    (M8).
12. `MAIL_STORE_MAX` for the beta: **2 GiB per built-in seed**.
13. Refresh collects: **every 30 minutes, this week and last**.
14. The moved clock: **`--clock-offset <secs>`, test-only, on the helper and
    the seed**, as `--record-ttl` is.

## Open questions, with what I recommend

1. **The pair tag (M7).** New in revision 3: without it the wrapper can't be
   trusted for M16. 16 bytes per envelope, no new dependency. Recommend
   **yes**.
2. **The test switch that cuts connections** (tests 11 and 18). Recommend a
   test-only helper flag, `--cut <seed|live>`, beside `--clock-offset`.
