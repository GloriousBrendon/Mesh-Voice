# Spec: `mailbox` (Mesh)

Status: **DRAFT, revision 2 (2026-11-12), for review.** Not approved. No plan
and no code until it is. Revision 2 closes the seven holes from the first
review and takes your answers to its open questions (*Decided questions*).
Module 8 of the build order in [CAPABILITY-MAP.md](CAPABILITY-MAP.md).
Depends on [`routing`](SPEC-routing.md), [`seed`](SPEC-seed.md),
[`e2ee`](SPEC-e2ee.md) and [`dm`](SPEC-dm.md) (all built). It changes
routing, seed, e2ee and dm; those changes are dated proposals at the end, not
made yet.

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
   SPEC-dm's note, now the **mail record**, plus what a sender remembers
   once that record has lapsed.
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
locked. Tests 9 and 10 run the helpers with a moved clock (a test-only flag,
as `--record-ttl` is today), not by waiting.

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
   she sees that, and mails "three" again at once. Bob gets it once.
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
    Bob unlocks and collects Dave's message with no add: he answers `failed`
    by receipt. Alice's connection comes back and her add is mailed; Bob
    collects it at his next refresh. Dave unlocks, collects the `failed` and
    mails his message again at once; Bob shows it. No `MAIL_LIFE` wait.
12. **Changes first.** As test 11, but Alice's add is mailed before Bob
    unlocks, so both are collected in one collect, Dave's put first: Bob
    applies the add first and shows Dave's message straight away.
13. **Out of order.** Alice mails Bob 60 messages, then the two meet live
    and Alice sends them again on the live session; Bob collects the mail
    afterwards. Every message shows once; the mail Bob can't open any more
    is dropped quietly, starts no session repair, and leaves no error.
14. **A broken session heals by mail.** Bob's stored session with Alice is
    deleted, as a damaged session file would be (a device restored from a
    backup is a new device, with its own sessions). Alice, away, mails him; Bob
    can't open it and drops it. Alice, still never online with Bob, sees
    from his mail record that he's been online since and hasn't answered,
    mails again on a fresh session, and Bob gets the message.
15. **No re-mail on unlock.** Alice mails Bob 20 messages and locks; she
    unlocks again before Bob is back: nothing is mailed again.

## Decisions this spec makes

| # | Decision | Why |
|---|---|---|
| M1 | **Mail is dm D5 with a holder in the middle.** When events wait for a reader's device with no open connection, the sender seals them for that device and puts them on one of the reader's holders. The reader collects, opens, stores and acknowledges them as if they'd come live. The sender's counter moves only on the reader's own `ok`, live or by receipt (M9), never on the holder's word. | The event format, store and counter don't change. A holder that loses mail can delay a message, never make it look delivered. |
| M2 | **When mail goes out.** Right after an event is stored, for each reader device with no open connection; at lock and at quit (a flush, `MAIL_FLUSH`); and at each refresh for anything waiting and not yet mailed. Live delivery still happens whenever a connection opens; the event ID removes duplicates. | "Send and quit" must work a second after sending. A crash still leaves it waiting on the sender. |
| M3 | **A holder is anything that agrees to hold, named in the reader's own record.** A holder is given by ticket (a seed) or by device key (a friend's device, *not in v1*). The reader lists up to `MAX_HOLDERS`, in the order to try; by default our built-in seeds. | Not a fixed list: anyone can run a seed for their friends and point their own record at it. The reader chooses, because the reader collects. Devices can join the list later without changing the format. |
| M4 | **One holder per message** (decided Q6), the next only on failure (M10). | Twice the space bought little; M10's re-mail covers a lost holder. |
| M5 | **Boxes.** Mail sits in a box per (sender, reader) pair per week: `box = blake3::derive_key("mesh/v1/mail-box", pair ‖ reader ID ‖ week)`, 16 bytes, `week = unix days / 7`, `pair` from routing D1 (the self pair for your own devices). Inside it, one slot per reader device: `slot = H("mesh/v1/mail-slot", pair ‖ device key)`, 8 bytes. Weekly (decided Q2). | Only the two people can compute a box. It changes weekly, so a seed can't link a pair across weeks. Slots let a device collect only its own envelopes. |
| M6 | **Claims.** A holder takes mail only into boxes and slots its owner claimed: `(box, slot, collect tag, expiry)`, `collect tag = H(collect secret)`, `collect secret = H("mesh/v1/mail-collect", device secret ‖ box)`. A device claims, for each contact and the self pair, this week and the next `CLAIM_WEEKS_AHEAD`, and renews daily while unlocked; a claim lasts `CLAIM_LIFE` after its last renewal. A put into an unclaimed slot answers `no-box`. | Strangers can't aim mail at you: they can't compute your boxes, and nothing unclaimed is held. The collect secret comes from the device's own key, so neither the sender nor your other devices can empty your slot. |
| M7 | **Mail is sealed to the reader's device** (e2ee D8): an Olm envelope, padded (e2ee D9), inside the sealed-sender wrapper, which also carries the sender device's **mail sequence number** `mseq` (one counter per sender device and reader device, never visible to the holder). Puts, claims and collects go from the throwaway identity (seed D7b). | The holder sees a box, a slot, a padded length and times. `mseq` lets the reader open in the sender's order (M8). |
| M8 | **Opening collected mail.** See *Opening mail*: order by `mseq`, mail before live backlog, mail that can't be opened dropped quietly with no session repair, group changes applied before other events. | vodozemac keeps 40 skipped keys per chain and five chains; opened out of order, mail fails. The sender still holds the event, so a quiet drop costs a re-send, never a message. |
| M9 | **Receipts are mail, carrying `ok` and `failed`** (decided Q1). After storing collected events the reader puts a receipt into the sender's box at the sender's holders: per sender device, the `mseq` values it answered `ok` and those it answered `failed` (dm's `failed`: not yet a member, a newer version). On collecting it, the sender moves its counters on the `ok`s, as live; a `failed` is mailed again at once, then on `FAILED_BACKOFF`, within D5's 30 days. | Without receipts, people in different time zones show "sending" for ever. A `failed` must come back soon, not after `MAIL_LIFE`: it usually means "send it again once the add has reached me". |
| M10 | **Full or gone holders.** A put answered `full`, `no-box` or `slow-down`, or unanswered for `HOLDER_WAIT`, goes to the reader's next holder; if none take it, the event keeps waiting on the sender (D5) and is tried again at the next refresh. Mail on a holder that lost it is mailed again by the **`since` rule** (M12). | A full or vanished holder delays mail; the sender keeps every event until the reader's `ok`. |
| M11 | **The mail record, and what a sender remembers.** Per (my device, contact), under a weekly lookup `H("mesh/v1/mail-lookup", pair ‖ sender ID ‖ week)`, encrypted and signed as routing D3: the signed device list, device key, e2ee bundle and fallback key (no one-time key, no address), and the device's holders, each with its **`since`** (M12). It lasts `MAIL_RECORD_LIFE` (7 days), is put again daily and on any change; the getter asks for this week and last. **A sender remembers, per contact device, the last mail record it read** (holders, `since`, device key, fallback key) and keeps its Olm session, in its sealed store, for `REMEMBER_LIFE` after the record lapses. Mail to a device whose record has lapsed goes to the remembered holders on the existing session. | The record can't last 30 days: its device list would be stale. But a device that's away doesn't change its holders or rotate its fallback key (e2ee: replaced weekly, while unlocked), so what the sender saw last is still right while the reader is away. A device the sender has no session with, and no record for, can't be mailed: it waits (D5). The fallback key is replaced only when the device publishes records (`FALLBACK_LIFE`, 7 days, in `mesh-e2ee`), so only while it's unlocked. |
| M12 | **The `since` rule (a wiped holder).** For each holder, the mail record says since when that holder's claims are good: the time of the device's first claim there that's still standing. A device that finds its claims gone (a renewal or collect answered `no-box`) claims again and sets `since` to now. A sender that reads a `since` later than a put it made there that has no `ok` mails that event again at once, to the reader's holders as listed now. The sender keeps, per event and reader device, the holder and time of its last put (M15). | The reader sees a wipe first, because its own claims are gone. A sender has no other way to know. Without this, a wiped holder still listed means nothing is mailed again for `MAIL_LIFE`. |
| M13 | **Padding hides how many boxes are real.** Each device claims, renews and collects in buckets of `CLAIM_BUCKET` (64) boxes per week (decided Q3): someone with 70 contacts handles 128. The extra boxes are **dummies**: `H("mesh/v1/mail-dummy", device secret ‖ i ‖ week)`, stable across a week like real ones, claimed, renewed and collected exactly as real ones are. | If dummies changed at each renewal, or weren't collected, the seed would see which claims are real. It still sees which boxes ever get puts: padding hides your number of contacts, not who's writing to you this week (M17). |
| M14 | **Closed seeds** (decided Q8). `--mail-token <secret>`: the seed takes claims and puts only with that token. Its ticket carries it (`&mail=…`), which reaches friends only inside the encrypted mail record. Without the flag a seed is open. | Lets anyone run a seed for their own friends without serving the world. A token passed on is changed by the operator; the next records carry the new one. |
| M15 | **The "already mailed" mark is saved.** Per (event, reader device): the holder, put time and `mseq` of its last put, kept with dm's counters in the sealed log index, saved once per batch of puts. Unlock reads it; only events with no mark, or that M9, M10 or M12 says to mail again, are put. | Without it every unlock re-mails everything still waiting. |
| M16 | **Healing a broken session by mail.** A sender whose mail to a reader device has had no receipt for `HEAL_AFTER` while that device's mail record shows it has been online since (the record's `created` is later than the puts) concludes the session is broken. It starts a fresh outbound session from the record's fallback key and mails what's unacknowledged on it. At most once per `HEAL_AFTER` per device. | The reader can't say "I couldn't open that" (it can't tell from whom), and mail failures are usually just order (M8). A device that's been online and still hasn't answered anything is the one signal that something is really broken. |
| M17 | **What a seed learns, plainly.** Per box: when it was claimed and renewed, by a throwaway session; each put's padded size and time, and the session and network it came from; each collect and delete, with its session. It can't see who anyone is, what's said, which chat, or which boxes belong together, from its files. Padding hides how many contacts a session has, **not which of its boxes are getting mail this week**. A live operator can link sessions, as seed D10b already says for address records: the session that claims box X and the session that puts into X are contacts; over a week, that graph with volumes and times. | Stated so nobody is surprised. It's what any store-and-forward server learns, minus names and content. |
| M18 | **Junk on an open seed.** Puts only into claimed slots within the box's quota, so a friend can fill only the boxes between you two. A stranger can still claim boxes of their own and fill them: bounded by the seed's rates per throwaway key, per network and through a relay, and by `MAIL_STORE_MAX`. A full seed answers `full`, and mail waits on the sender (M10): an attack costs delay, not loss. No stamp for the beta (decided Q7); the built-in seeds run with a small `MAIL_STORE_MAX`. If the stamp proposal is adopted later, mail services take the same per-key-per-hour stamp as the others. | No holder can tell a stranger from a friend. The defence is that a stranger can spend only the seed's space, not aim it at anyone, and only so fast. |
| M19 | **Nothing identifying is logged**: counts only (claims, puts, collects, refusals, bytes held per hour). | As seed D10 and routing D10. |

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
2. **Unseal everything collected**, then group by sender device and sort by
   `mseq`. Open each sender device's envelopes in that order. Mail from
   several holders merges by `mseq`, so which holder it came from doesn't
   matter.
3. **What can't be opened is dropped quietly.** A `MissingMessageKey`,
   `TooBigMessageGap` or old-chain failure on mail means the sender
   re-encrypted it since (it went live, or was mailed again): the sender
   still holds the event and will send it again if it has no `ok`. It's
   counted in the log, answered nowhere, and **never starts a session repair
   on its own** (e2ee D4's fresh-session answer is for live first messages
   only). A healing session (M16) arrives as a first message, which opens.
4. **Hand to dm in one batch per collect, group changes first**: every
   `Group` body in the batch is applied before the other events, so a
   member's message collected with their add shows at once. Anything dm
   still answers `failed` goes back by receipt (M9).
5. **Then the live backlog**, as today. Its duplicates of mail just opened
   are removed by event ID.

### Limits (named, derived from the 30-day promise)

| Name | Value | Derived from |
|---|---|---|
| `MAIL_PROMISE` | 30 days | dm D5's 30 days |
| `MAIL_LIFE` | 30 days | An envelope is kept this long after its put: the promise |
| `CLAIM_WEEKS_AHEAD` | 5 | Weeks claimed beyond this one: ⌈30 / 7⌉ + 1, so a box exists for every day of 30 away |
| `CLAIM_LIFE` | 35 days | 30 days away plus the 5 days of a partial first week; mail put on day 30 is still collectable |
| `COLLECT_WEEKS_BACK` | 5 | Weeks collected behind this one at unlock (this week only at a refresh) |
| `MAIL_RECORD_LIFE` | 7 days | The fallback key's week |
| `REMEMBER_LIFE` | 30 days | The promise: a sender keeps a lapsed record's holders and the session this long |
| `CLAIM_BUCKET` | 64 | Decided Q3 |
| `BOX_MAX_BYTES` | 4 MiB | Per box (one sender, one reader, one week) |
| `BOX_MAX_ENVELOPES` | 1,000 | Per box; D5's 2,000 events over the weeks they span |
| `MAX_HOLDERS` | 3 | Fits the mail record |
| `MAIL_FLUSH` | 3 s | Lock and quit wait this long for puts |
| `HOLDER_WAIT` | 10 s | A holder silent this long is skipped |
| `FAILED_BACKOFF` | 30 min, doubling to 1 day | A `failed` is mailed again at once, then after these |
| `HEAL_AFTER` | 2 days | No receipt this long, with the reader seen online since |
| `MAIL_STORE_MAX` | 2 GiB on the built-in seeds for the beta; operator flag `--mail-max` | Decided Q7. Claims and mail count against it together |
| Seed rates | per throwaway key: 8,192 claims or renewals and 600 puts an hour, 64 MiB of puts an hour, 120 collects a minute; per network twice that; everyone through a relay, 100 times the per-key figures | Sized for `MAX_CONTACTS` (1,000): 1,001 boxes padded to 1,024, × 6 weeks, is 6,144 claims a day |

### What it costs

Assumptions: one holder; a typical mail record of about 1.1 KB (two
devices' list, the e2ee field, three holders); a claim of 64 bytes (box,
slot, tag, expiry) and a renewal or collect query of 24 bytes (box, slot);
frames and seals are included in round figures. "Round trips" are on one
connection to the seed, which is already open for address records.

| | 100 contacts | 1,000 contacts |
|---|---|---|
| Boxes per week (with the self pair, padded to 64) | 128 | 1,024 |
| **At unlock** | | |
| Collect: this week and 5 back | 1 round trip, 768 queries, 18 KB up, plus any mail down | 1 round trip, 6,144 queries, 147 KB up |
| Claims: 6 weeks (first unlock only; later the daily renewal covers them) | 1 round trip, 768 claims, 49 KB | 1 round trip, 6,144 claims, 393 KB |
| **Per day** | | |
| Renewals (daily) | 1 round trip, 768, 18 KB | 1 round trip, 6,144, 147 KB |
| Refresh collects (this week only, every 30 min, 48 a day) | 48 round trips, 128 each: 147 KB a day | 48 round trips, 1,024 each: 1.2 MB a day |
| Mail records put (daily, one per contact) | 1 round trip, 110 KB | 1 round trip, 1.1 MB |
| Mail records got (only for contacts mailed while away, at most once a day each) | up to 100, typically a few | up to 1,000, typically a few |
| Remembered records in the sealed store | about 50 KB | about 500 KB |
| Saved "mailed" marks (M15) | 16 bytes per waiting event and device | the same |
| **On the seed, per person** | claims: 768 × 64 B = 49 KB | 6,144 × 64 B = 393 KB |

At 1,000 contacts the daily total is about 2.5 MB up, against routing's
existing address records of about 24 MB a day at that size (1,000 × 500 B ×
48). At 100 it's about 300 KB. The beta's seeds (2 GiB of mail) hold claims
for about 5,000 people of 1,000 contacts, or 40,000 of 100, before any mail.

**Test 1's 5 seconds.** Bob's helper is online when its relay connection is
up (measured: a cold ping by ID took 0.64 s on n0's relays, routing smoke
2026-11-09). His first seed connection opens for address records anyway;
the collect is one more round trip on it: 768 queries, 18 KB, answered with
one envelope. Opening and storing it is under 10 ms (dm's intake: 10,000
events in 1.5 s). So "one" shows about 1 to 1.5 s after the helper is
online, over the internet with a 100 ms round trip; 5 s leaves room for a
slow relay. At 1,000 contacts the collect is 147 KB up, about 1 s more on a
slow uplink: still inside 5 s.

## Behaviour

### Sending

1. dm stores the event and numbers it (D5).
2. For each device it goes to with no open connection, the helper takes
   what's after that device's counter (D5's limits) and not already marked
   mailed (M15), encrypts it on the session with that device (or starts one
   from the mail record's fallback key), seals it with the next `mseq`
   (M7), and puts it into this week's box at the first of the reader's
   holders that takes it (M10). The reader's holders and keys come from the
   latest mail record, or the remembered one (M11).
3. Live delivery is unchanged.
4. Own devices get mail only for events authored on this device and for
   read markers; everything else reaches them from its author.

### Receiving

1. At unlock: collect (this week and `COLLECT_WEEKS_BACK`), open as *Opening
   mail* says, store, delete (collect secret), put receipts (M9). Renew
   claims if due; if a renewal or collect answers `no-box`, claim again and
   move that holder's `since` (M12).
2. At each refresh: collect this week only, the same way.

## Changes this needs elsewhere (proposals, dated 2026-11-12, not made)

**routing**

- *D3:* a second record kind, the **mail record** (M11), beside the address
  record: weekly lookup (`mesh/v1/mail-lookup`), same slot, encryption and
  signing, at most 4,096 bytes; put daily and on change for every contact
  and the self pair.
- *D5:* the record kind travels with `addr-put` and `addr-get`, or two new
  services; the plan picks.
- *D6:* the seed keeps a mail record up to 7 days ahead (address records:
  3 hours), within the same totals.

**seed**

- *New D:* services `mail-claim` (claim or renew), `mail-put`, `mail-get`
  and `mail-take` (collect secret, then delete), batched, from the
  throwaway identity.
- *D8:* the mail rates and caps in *Limits*; `full` at `MAIL_STORE_MAX`.
- *D9:* mail doesn't fit one file rewritten on change; the plan picks the
  layout (decided Q10).
- *D10b:* add M17's list.
- *New flags:* `--mail-token` (M14), `--mail-max`.

**e2ee**

- *D4:* "a first message that still can't be opened is answered with a
  fresh session" applies to live delivery only; mail that can't be opened
  is dropped (M8), and a broken session heals by M16.
- *D14:* the 30-day keep and `key-request` don't apply to mail (M8, step 3):
  the sender holds the event.
- *D8:* the sealed wrapper carries `mseq` (M7).

**dm**

- *D5:* "when a connection to that device opens" becomes "when a connection
  opens, or by mail with none open (mailbox M1, M2)". Receipts (M9) move the
  counter as live answers do; a `failed` by receipt is sent again as M9 says.
- *D5:* the counters' saved index gains M15's marks and M11's remembered
  records.
- *D8:* own devices get by mail only events authored on this device and
  read markers.
- *Receiving:* a batch applies its group changes first (M8, step 4). Today
  dm applies a live batch in order; the same rule suits live too, and costs
  nothing.
- *The note "What mailbox changes"* becomes a pointer here.

## Attacking revision 2

Revision 1's attacks (send-then-quit, the holder's word, counted claims, the
sender emptying a box, strangers' own boxes, a flooding friend, where the
holders go, revoked devices, weekly linking, group weight, duplicates, a
wiped seed) still stand as answered there, now in M1 to M18. New ones:

1. **Dummies give themselves away.** If dummy boxes changed at each
   renewal, or were claimed but never collected, a seed could tell them
   from real ones. *Changed:* dummies are derived per week and treated
   exactly like real boxes (M13). What's left: dummies never get puts, so a
   box that does is real. Stated in M17.
2. **Healing on a mere absence.** A reader who's simply away doesn't
   answer either. If silence alone triggered a fresh session, every long
   absence would start one. *Changed:* healing needs the reader's mail
   record to show it's been online since (M16).
3. **Healing from a stale record.** M16 needs a fresh mail record; with
   only a remembered one (the reader's record has lapsed), the sender can't
   tell. *Accepted:* a reader who's away can't be healed until they've been
   online again for a day (a new record), which is when healing can help
   anyway.
4. **The remembered fallback key goes stale.** A reader online while the
   sender is away rotates its fallback key weekly; the sender's memory
   then holds a key the reader no longer has. *Not a problem in practice:*
   the sender only needs a fallback key for a device it has no session
   with; for one it has a session with, the session carries on. A brand-new
   device the sender has never reached waits for a live meeting or a fresh
   record (stated in M11).
5. **A `failed` loop.** A message that will always fail (its sender really
   isn't a member) would be mailed again for ever. *Bounded:*
   `FAILED_BACKOFF` doubles to a day and stops at D5's 30 days; D7's
   `NotInChat` is dropped as now, not answered `failed`.
6. **Receipts telling the seed who replies to whom.** A receipt is a put
   from the reader's session into the sender's box, so the operator sees
   the pair's traffic both ways. *Accepted:* M17 already says a live
   operator can link the pair.
7. **`since` spoofing.** Could someone make a sender mail everything again
   by faking a `since`? *No:* it's in the reader's signed, encrypted mail
   record, which only the pair can read and only the reader can sign.
8. **A wiped seed while the reader is away too.** Nobody sees the wipe
   until the reader returns, re-claims and publishes `since`; the sender
   then mails again when it next unlocks. *Accepted:* delay, not loss (puts
   into the wiped seed meanwhile answer `no-box` and wait on the sender).
9. **2,000 mailed envelopes.** Opened in `mseq` order the gap is one each
   time, so `MAX_MESSAGE_GAP` doesn't bite; only a hole of 2,000 lost
   envelopes would, and D5 stops at 2,000 events anyway.
10. **The promise vs a full seed.** A 2 GiB beta seed can fill. *Stated:*
    the promise holds while the holder has room; past that, mail waits on
    the sender (the Objective says so).
11. **Clock skew at a week's edge.** Sender and reader may disagree on the
    week. *Covered:* the reader claims weeks ahead and collects five back,
    so either week's box is claimed and collected.
12. **The first collect delays live messages.** Live envelopes wait up to
    `HOLDER_WAIT` while mail is collected. *Accepted:* at most 10 s, once
    per unlock, and only when a holder is slow; normally one round trip.

## Testing strategy

- Unit tests: box, slot and dummy derivation; claim padding; quotas and
  expiry; the collect secret; receipts with `ok` and `failed`; `mseq`
  ordering across holders; the `since` rule; the saved marks; healing's
  trigger.
- Seed tests for each service, limit, token and `full`.
- Process tests: acceptance tests 1 to 15, with a moved clock for 9 and 10;
  Mesh-Voice's checkpoint test gains test 1 through the app.

## Boundaries

- **Always:** the counter moves only on the reader's `ok`; logs hold counts
  only; every limit a named constant.
- **Ask first:** a new dependency; a change to any limit; anything that
  lets a holder see more than M17 says.
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
10. Seed storage for mail: **the plan decides**.

## Open questions, with what I recommend

1. **Batch order in dm for live delivery too.** M8 step 4 applies group
   changes first in a mailed batch. Recommend **the same for live
   batches**: it's free, and it removes the live case of test 11.
2. **`MAIL_STORE_MAX` for the beta.** Recommend **2 GiB per built-in seed**:
   claims for a few thousand people with room for their mail; the homelab's
   disk takes it easily.
3. **Refresh collects.** Recommend **every 30 minutes, this week only**, as
   above: mail for someone online is rare (it's what live delivery is for),
   so this is a backstop, and at 1,000 contacts it's 1.2 MB a day.
4. **The moved clock for tests 9 and 10.** Recommend a **test-only helper
   flag** (`--clock-offset <secs>`), like `--record-ttl`, rather than
   waiting or faking time in a library.
