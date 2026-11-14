# Spec: `spaces` (Mesh)

Status: **DRAFT 2026-11-14, for your review.** No plan and no code until you
approve it. Module 9 of the build order in [CAPABILITY-MAP.md](CAPABILITY-MAP.md).
It depends on [`dm`](SPEC-dm.md), [`mailbox`](SPEC-mailbox.md),
[`e2ee`](SPEC-e2ee.md), [`routing`](SPEC-routing.md) and
[`social`](SPEC-social.md), all built. It changes five approved decisions in
them (*Changes to approved decisions*), each of which needs your OK.

## Acceptance tests

Process tests in Mesh-Node (`tests/spaces.rs`) run helpers and a seed, as
`tests/dm.rs` and `tests/mailbox.rs` do. The Mesh-Voice checkpoint test
(`checkpoint.rs`) gains a Spaces step. The last two tests are by hand.

In the process tests, **A** creates the Space. **B** and **C** are A's
friends but not each other's. **D** is a friend of nobody in it.

1. **Strangers in one Space.**
   - A creates a Space with `#general`, and adds B and C.
   - B and C talk in `#general` both ways, live, in under 1 s.
   - They still aren't friends:
     - neither shows in the other's friends list;
     - neither gets the other's presence;
     - a DM between them is refused (`not-friends`);
     - once C leaves, C's devices no longer get through B's handshake.
2. **Away and back.**
   - C locks. B posts three lines and locks too.
   - C unlocks with nobody else online, and gets all three by mail, in order.
   - The seed's mail puts for those three lines are **at most one per C
     device**, not one per line (S9).
3. **An invite code.**
   - B makes a Space invite code. D, who is nobody's friend, joins with it.
   - B's device admits D on its next collect, within 5 minutes.
   - D gets `#general`'s history from before D joined, newest first, from
     whichever member it reaches first.
4. **History can't be forged.** A member's test client alters the text of
   another person's event in the history it serves. D refuses that event
   (bad signature), takes the rest, and gets the real one from another
   member.
5. **Roles.**
   - A member can't create a channel, delete someone else's message, or post
     in an admins-post-only channel. Every member's device refuses it, not
     just the sender's.
   - An admin deletes one of C's messages, and it's gone on every device.
6. **The admins' channel.** A channel only admins can see:
   - no member's device ever receives its events, live, by mail or in
     history;
   - a member made admin later gets its history;
   - an admin made a member again keeps what they had, but gets nothing new.
7. **Kick and ban.**
   - A kicks C. C can rejoin with a new invite.
   - A bans C. C's later events are refused by everyone, C's invite codes and
     joins are refused, and C's devices stop getting through every member's
     handshake, except where C is also someone's friend.
8. **Two admins remove each other at once.** They do it while apart. When
   they meet, every device ends with the same members, and the owner's
   decision wins (S7).
9. **Fifty members.** A Space of 50 members, one device each (an `--ignored`
   measurement, recorded in the plan):
   - a line reaches all 49 others;
   - unlock with 5 such Spaces is under 1 s after "online";
   - a sender's CPU time and seed puts per line are recorded.
10. **Unlock doesn't read Space history.** Unlock takes the same time with a
    Space holding 100,000 events as with an empty one. A Space's log is read
    the first time it's used (S11).
11. **Nothing identifying is logged.** The seed's files and logs and the
    helper's stderr hold no Space ID, channel name, member ID or message text.
12. **By hand, on the gate** (three machines, two networks, an outside seed):
    the above with real people, in Mesh-Voice.
13. **By hand, a solo look** at the screens: the Space rail, channels and
    categories, the member list with roles, Space settings, the invite flow,
    unread counts and mentions, and the narrow layout.

## Objective

Today Mesh has friends, 1:1 chats and group DMs of up to 10 friends. A
Discord server is something else: people who aren't all friends, talking in
named channels, with someone in charge who can remove people. That is what
`spaces` adds.

**The smallest version that replaces a small Discord server for text:**

- A Space of **up to 50 members** (*Question 1*), which needn't be friends.
- **Text channels in categories**, which the owner and admins create, rename,
  reorder and delete.
- **Three roles:** the owner, admins and members. Each channel has two
  switches: who may post (everyone, or admins only, for announcements) and
  who may see it (everyone, or admins only).
- **Joining** is by being added by a member who is your friend, or with a
  Space invite code that works like a friend code.
- **History for new members**: they see what was said before they joined
  (*Question 2*).
- **Moderation:** admins delete anyone's message, and kick or ban members.
- **Everything a DM already has**, per channel: edits, deletes, reactions,
  replies, files and pictures, typing, unread counts and read state.
  **Mentions** are added: `@name`, and `@everyone` for admins.
- **It works while people are away**: by mail through the seed, and from
  whichever member is online.

### Not in this version

- Voice channels (`calls`).
- Custom roles, per-person permission overrides, and per-role channel
  access beyond "admins only".
- Threads, pins, slow mode, timeouts, audit log, server emoji, webhooks and
  bots, server discovery, and vanity invites.
- Presence for members who aren't your friends. Friends show as they do now.
- Spaces past 50 members (see *The path past 50*).
- Per-Space nicknames and avatars. Display names come from joining.
- Notification settings per channel, beyond muting a whole Space.

## What it reuses

**From `dm`** (Mesh-Node `mesh-dm` and the helper), close to unchanged:

| dm | In a Space |
|---|---|
| D1 events | The same event format, with a channel in it, and **signed by the author's device** (S5) so other members can pass it on. |
| D2 ordering | The same `seq` and `n` rules, per channel. |
| D3 edits and deletes | The same, plus an admin may delete anyone's message (S8). |
| D4 reactions | Unchanged. |
| D5 delivery counters | Unchanged: per reader device, sender's own events. This is what makes "sending" and "sent" honest. |
| D7 signed change chain | Generalised from the creator to roles: the owner, then admins (S7). |
| D8 own-device sync | Unchanged. Your own devices get your Spaces as they get your chats. |
| D9 read state | Per channel. |
| D10 typing | Per channel, to connected member devices. |
| D11 store | Segments per channel. The device-wide log changes for Spaces (S11). |
| D12, D13 files | Unchanged. Members fetch from any member device that has the file. |
| D14 limits | The same, plus the Space limits (*Limits*). |
| Group DMs over 1:1 Olm sessions (e2ee D6 as changed) | **Spaces too, in this version** (S4). |

**From `mailbox`**, unchanged in format:

- A Space member is a pair like a friend is (routing D1 needs only the two
  IDs), so mail boxes, slots, claims, mail records, receipts and the
  six-week collect all work for them as they are.
- The one change is when Space events are mailed (S9): batched, not one put
  per event.
- Claims are padded to 64 boxes a week. A member of one full Space has 49
  pairs from it, so most people still fit one bucket.

**From `routing` and `social`:**

- Address records, reach and the handshake allow-list cover Space members
  (S2).
- Space invite codes are friend codes with a different purpose inside (S6):
  the same hold, lookup, knock box, seal and limits at the seed.

**From `e2ee`:** Olm sessions per device and `encrypt_device_batch`, as
group DMs use them. The Megolm code (`mesh-e2ee` `group.rs`) is already
built. It stays unused in this version, and it's what the path past 50 uses.

## Decisions (proposed)

| # | Decision | Why |
|---|---|---|
| S1 | **A Space is a signed log of changes plus channels of events.** Its ID is `blake3::derive_key("mesh/v1/space", creator ID ‖ random)`, as a group DM's is (dm D7), so nobody else can make a creation for it. The log holds: the name; the channels and categories, each with its two switches; the members, each with their role and the display name they joined with; the bans; and the invite settings. | The group DM's design, which is built and tested, with more kinds of change. |
| S2 | **Members are "Space peers", a new kind of contact.** For each member it shares a Space with, a device keeps an account entry of kind `space`. That kind publishes address and mail records, allows the handshake, and reads their records, exactly like `outgoing`. But **a connection with one never makes a friend** (social D8 excluded), and it gets no presence and no DMs. Leaving or being removed drops the entry, unless the two share another Space or are friends. Blocking still wins: a blocked member is never a peer, and their events are hidden. | Every lower layer keys on "contact", and the pair secret needs nothing but the two IDs. So the smallest way to let strangers in a Space talk is to let them be contacts of a kind that means nothing else. Discord server members can see you're online and message you; here they can do neither unless you're friends. |
| S3 | **Channels.** A channel is a conversation inside the Space, with its own ID from the Space and a random value, its own segments, read marker and typing. There are two switches. **Post:** everyone, or admins only. **See:** everyone, or admins only. Events of an admins-only channel go only to admin devices, live, by mail and in history. Categories are names that channels sit under, in order. | It's what small servers use: announcements and a mods' room. Per-role access beyond admins is custom roles, which comes later. |
| S4 | **Encryption is per device over Olm, as group DMs** (e2ee D6 as changed 2026-11-11). A Space event is encrypted for each current member device, in one `encrypt_device_batch`. *This changes e2ee D6's "Spaces keep sender chains" for this version (Question 3).* | It's what's built and tested, and it lets mail stay per pair (S9). At 50 members of about 2 devices, a line is roughly 100 Olm encryptions. Test 9 measures it. Megolm encrypts once, but then offline members need a shared Space box at the seed and a key-share round on every removal, which is a seed change and more machinery than 50 members need. |
| S5 | **Space events are signed by the author's device key** over (Space, channel, event), and checked against the author's device list. dm events carry no signature because the envelope proves the sender (dm D1). In a Space, others pass events on (history, S10), so the event has to prove its author by itself. A signature that doesn't check means the event is refused. | Without it, a member serving history could put words in anyone's mouth. Ed25519 over a small event is cheap. |
| S6 | **Joining.** (a) **Add by ID:** a member whose role allows it adds one of their friends. Members can add by default; the owner can make it admins only. (b) **Space invite code:** a friend code (identity D4, social D1) whose sealed request, a knock, asks to join the Space instead of to be friends. The inviter's device collects knocks as it does friend requests (every 5 minutes while unlocked, social D7). If the code is live and the inviter may still add people, it admits the sender with a signed add change. A knock for an expired or revoked code, or from a banned ID, is dropped. | Both reuse what exists: (a) is group DM's add, and (b) is the whole friend-code path with one more purpose. The cost is that a join waits for the inviter's device to be online. Discord is instant. |
| S7 | **Roles and the change chain.** dm D7's chain, with roles instead of "the creator". **The owner** (the creator) can do everything: make and unmake admins, transfer ownership, and delete the Space. **Admins:** channels and categories, kick, ban, delete messages, invite settings, and the Space's name. **Members:** post where allowed, react, leave, and add friends or make invite codes if allowed. A change counts only if it follows the newest change held and its author has the right in that version. **On a fork, the branch containing an owner change wins; then the one containing an admin change, by the lower (`seq`, event ID) of its first such change; then the lower event ID.** A removed admin's change can only fork from before the removal, and the owner's or the earlier admin's decision stands. | The same proof as dm D7 (review F): membership is judged by the version a change follows, which nobody can back-date. Ranking branches by role is the one new rule. |
| S8 | **Moderation.** *Delete:* an admin's `Delete` of anyone's event, signed, checked against the role in force, and honoured by honest clients as dm D3. *Kick:* remove a member; they may rejoin. *Ban:* remove a member and add their ID to the ban list. From then on, their events, knocks and adds are refused everywhere, and every member drops them as a peer. | Discord's three basic tools. As everywhere in Mesh, a modified client can keep what it already has (map). |
| S9 | **Space events are mailed in batches.** For a reader device with no open connection, a Space's waiting events go by mail at the next refresh (30 minutes) or at lock and quit (the flush), **in one envelope per reader device**, packed up to the envelope's limit. Receipts and counters are as mailbox M9 and dm D5. *This changes mailbox M2's "right after an event is stored" for Space events (Question 4).* | The seed allows 600 puts an hour per key (`PUTS_PER_KEY`). One put per event per offline device would let a 50-member Space post about 6 lines an hour. Batching makes the cost about two puts an hour per offline device, whatever is said. The delay costs nothing: a reader who comes back sooner meets the sender live, or a member (S10). |
| S10 | **Catch-up and history come from any member.** On joining, and whenever a member device returns after more than mail covers (30 days, or past dm D5's 2,000 events), it asks the first member device it reaches for the channels it may see. Each channel comes newest first, in batches of 200, by a per-(Space, channel) counter like own-device sync (dm D8), so it resumes where it stopped. The events carry their authors' signatures (S5). A member serves only channels the asker's current role may see. | The map's "history comes from other members". A pull is resumable and costs nothing while nobody asks. Serving only what the role allows keeps the admins' channel closed. |
| S11 | **A Space's log is read when the Space is first used, not at unlock.** dm's device-wide log (every stored event's number and every deleted ID) stays for 1:1 chats and group DMs. Each Space gets its own, read the first time the Space is opened, sent to or received in during that unlock. | The map's open question: "before `spaces` reuses the dm store, it must change how the history log is held." This is the smallest change that keeps unlock flat (test 10). A Space's log in memory is about 16 MB per 100,000 events (PLAN-dm Task 15), bounded by the Space, not the account. |
| S12 | **Mentions and unread.** A `Text` event may list mentioned member IDs, and `@everyone` if its author is an admin. Unread counts are per channel from the read marker (dm D9). A mention also shows on the Space in the rail. One switch mutes a whole Space. | What makes a busy server usable. A mention list in the event costs bytes, not design. |
| S13 | **Nothing identifying is logged.** No Space IDs, channel names, member IDs or contents, only counts. | As routing D10, social D15, dm D15. |

## Limits

| Name | Value | Why |
|---|---|---|
| `SPACE_MAX_MEMBERS` | 50 (*Question 1*) | S4 and S9 are sized for it |
| `SPACE_MAX_CHANNELS` | 100, in up to 20 categories | Discord's own cap is 500 |
| `SPACES_PER_ACCOUNT` | 20 | Bounds peers, so mail buckets and unlock stay small |
| `HISTORY_BATCH` | 200 events | As dm D8 |
| Text, files, reactions | dm D14, D12 | Unchanged |

## Changes to approved decisions

Each needs your OK. Each would be written into its own spec, dated, once
this one is approved.

1. **routing D9:** the handshake also lets in `space` peers (S2), not only
   contacts and your own devices.
2. **social D8:** a new account kind, `space`, that a connection never turns
   into `friend`. It's synced between your own devices (social D11) like the
   other kinds.
3. **e2ee D6:** Spaces use per-device Olm in this version, not sender chains
   (S4). Megolm returns with the path past 50.
4. **mailbox M2:** Space events are mailed in batches (S9).
5. **dm D3:** in a Space, an admin may delete anyone's message (S8). DMs are
   unchanged.

## The path past 50

Megolm for Space events (e2ee D6 as first approved), and a shared Space box
at the seed: one put per line, collected by every member, with no per-reader
take. That's a new seed service and a mailbox change. Until a Space needs
more than 50, neither is worth building. Both fit on top of this design: the
signed events (S5) and the member pull (S10) stay as they are.

## What it needs from you

1. **Answers to the questions below**, and your OK on the five changes
   above.
2. **The open question in the map, "Move to a new ID?",** is marked "settle
   before `spaces`". My recommendation is to settle it as **not in this
   version**. Removing someone from a Space is by ID, which works as it is.
   A stolen, unlocked device of a member can read a Space until that member
   removes the device, as with DMs. Your call.
3. **For the gate (test 12): three people on two networks**, and a seed
   reachable from outside (the VPS). The process tests need neither.
4. **A look at the screens** (test 13), on Linux and Windows.

## Questions

1. **The size cap.** Recommended: **50 members** for this version. Small
   Discord servers are mostly under that, and it keeps S4 and S9 simple. The
   alternative is 100 now, at about twice the encryptions and puts per line.
2. **History for new members.** Recommended: **yes, everything**, newest
   first (S10, e2ee Q4's default). The alternative is from their join on, as
   group DMs work, which is simpler but unlike Discord.
3. **Per-device Olm instead of Megolm for now.** Recommended: **yes** (S4).
   It changes e2ee D6.
4. **Batched mail for Space events.** Recommended: **yes** (S9). Without it,
   the seed's per-key limit caps a Space at a few lines an hour while
   members are away.
5. **Invite codes in this version, or add by ID only?** Recommended: **both**
   (S6). Codes reuse the friend-code path almost whole, and a server you can
   join only by first befriending a member isn't much of a server.
