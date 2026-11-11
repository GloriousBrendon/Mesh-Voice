# Proposal: a cost for anonymous seed callers

**Status: DRAFT, for review. Nothing is built.** Audit finding 3, part 3.
Parts 1 and 2 are done (seed D8).

## The problem left

Callers use a throwaway key (seed D7b), and a new key is free. So the
per-device limits stop nobody, and only the per-network and relayed buckets
bite. Through a relay the seed sees no network, so one script with fresh keys
can use the whole relayed bucket, and every real relayed caller gets
`slow-down`. Nothing it does lasts (codes drain in 30 days, records in
3 hours), but while it runs, nobody can add a friend or be found.

## Proposal: an hourly stamp per throwaway key

- **What the work is.** Find a `nonce` (u64) such that
  `blake3::derive_key("mesh/v1/seed-stamp", seed key ‖ caller device key ‖ hour ‖ nonce)`
  starts with *D* zero bits. `hour` is `unix time / 3600`. Binding the seed
  key means a stamp works on one seed only. Binding the caller key means it
  can't be shared. blake3 is already a dependency: **nothing new**.
- **Which calls.** All seven seed services (`code-hold/get/revoke`,
  `addr-put/get`, `knock-put/get`). One new service, `stamp` (request:
  `hour ‖ nonce`; answer: `ok` or `pay ‖ D`). A key without a stamp for this
  hour or the last one (clock skew) gets a new answer, `pay ‖ D`, from every
  other service. The seed keeps the stamped keys in memory for the hour, the
  same way it keeps its rate counters (D10 holds: nothing is written down).
- **Client.** The helper mints a stamp on a blocking thread when the network
  starts, before its first seed call, then mints the next hour's ahead of
  time in its last 10 minutes. On `pay ‖ D` it mints at that *D* and tries
  once more.
- **Difficulty.** *D* = 20 normally (about 1 million hashes on average).
  When a relayed bucket filled in the last hour, the seed raises *D* by 2
  (4 times the work) for the next hour, up to 26, and lowers it by 2 for each
  quiet hour.

## How long it takes

| Machine | blake3 hashes a second, one core | D = 20 (average) | D = 26 (average) |
|---|---|---|---|
| Ryzen 7 7700X (measured) | 7.6 M (7.0 M with no SIMD) | 0.14 s | 9 s |
| Slow ARM board, Raspberry Pi 4 class (**estimate**) | about 1 M | about 1 s | about 1 minute |

One unlucky draw in 20 takes about 3 times the average. Speed follows clock
speed, not SIMD (portable code was 92% as fast), so I expect older laptops to
sit between the two rows. **Before building:** run the timing test on the
Windows PC and on the slowest machine anyone will use.

## What it buys, honestly

- It turns "free" into "costs a CPU". A naive script that makes a key per
  request now spends 0.14 s per key, and the per-device limits mean something
  again: 10 lookups a minute per million hashes.
- **It does not stop a determined attacker.** One 16-thread desktop mints
  about 120 stamps a second at *D* = 20, and about 2 a second at 26. Taking
  the relayed lookup bucket (300 a minute) needs 30 keys a minute, which that
  desktop still manages at 26. A GPU does far more. Pricing each call high
  enough to stop that would cost real users seconds per lookup on a fast
  machine and close to a minute on a slow one.
- So the stamp is a speed bump. The real backstops stay the same: the
  buckets keep an attack from lasting or spreading to direct callers, the
  hourly counts in the seed's log show it, and the limits move. More seeds
  (finding 6) spread the load further.

## Size and tests

About 80 lines: the `stamp` service and an in-memory set in the seed, the
check where streams are dispatched, minting in the helper. Tests:

- an unstamped key gets `pay`;
- a stamped key is served;
- a stamp for the wrong hour, another seed or another key is refused;
- *D* rises after a full bucket and falls back;
- a timing test, marked ignored, that prints hashes a second for the machine
  it runs on.

## Decisions for you

1. Stamp per key per hour, as above, or per call for the calls that store
   (holds and puts)? I recommend **per key per hour**. It costs real users
   one mint an hour, and per call stops no more.
2. Normal *D* of 20, rising to at most 26? I recommend **yes**, once the slow
   machine timing is in.
3. Build it before the VPS seed (finding 6). I recommend **yes**, as the
   audit says.
