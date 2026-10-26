# Spec: `seed` (Mesh)

Status: **APPROVED 2026-10-26** (with the recommended answers). Plan: [PLAN-seed.md](PLAN-seed.md). Module 3 of the build order in
[CAPABILITY-MAP.md](CAPABILITY-MAP.md). Depends on
[`transport`](SPEC-transport.md) (built; Checkpoint C pending on Windows) and
uses [`identity`](SPEC-identity.md)'s friend-code records as they are.

## Objective

Run Mesh's first piece of always-on infrastructure on the homelab: a
**seed**. It's an ordinary Mesh node that never sleeps. It does two jobs in
v1:

1. **Our own relay.** Mesh clients use it alongside n0's, so a connection
   that can't go direct no longer depends on n0's goodwill.
2. **It holds friend codes.** Identity already makes the encrypted code
   records and revokes; they're waiting for somewhere to live (identity D13).
   The seed stores them, answers lookups, counts uses, honours expiry and
   revokes, and rate-limits lookups. It never learns whose code it holds
   (identity D5).

Then a code like `K7QM-3XPA`, typed on another machine, turns into the right
`mesh1…` ID, ready for the safety number. That's the first time two people
can find each other without pasting a 60-character ID.

**Not in v1 of this module** (see Q3 and Q4):

- Rendezvous by ID ("where is `mesh1…` right now?"). Its privacy rules
  belong to `routing`, which adds it to the seed as one more service.
- Holding mail. That's `mailbox`, which adds it the same way.
- Device-link rendezvous (identity's link codes), unless you choose it in Q3.

**User stories**

- I make a friend code in Mesh, read it to a friend over the phone, and they
  type it in on their machine: Mesh shows them my ID and our safety number.
- A single-use code works once. A second person typing it gets "that code has
  already been used".
- I revoke a code I posted somewhere by mistake; from then on it finds
  nothing.
- Someone trying codes at random gets slowed to a crawl long before they hit
  a real one.
- Mesh's status line shows our own relay as home when it's the closest one.
- I install Mesh and use it. I never forward a port, open a firewall or
  set up anything on my router; neither does anyone I talk to.
- Brendon runs the seed with one `docker compose up -d` on the homelab. It
  needs no new router setting either: the relay rides on the HTTPS port
  Caddy already has, and nothing else about the seed is reachable from the
  internet.

### Decisions this spec makes (review these)

| # | Decision | Why |
|---|---|---|
| D0 | **No port forwarding, anywhere.** People who install Mesh never touch their router (transport already works this way: hole punching, then a relay over HTTPS; `portmapper` may ask a home router for a port by itself, but nothing depends on it). The seed adds no router setting either: the relay uses the existing 443 → Caddy, and the seed node has no public port at all. | Mesh has to be install-and-use, like Discord. If a seed needed forwarding, so would anyone running one later. |
| D1 | **`mesh-node seed --data <dir>`**: the same binary, a headless mode. No stdin protocol; logs to stderr; stops cleanly on SIGTERM. | The map's one-binary decision. Docker and systemd both handle a foreground process that logs to stderr. |
| D2 | **The seed has its own key, stored unencrypted** in its data directory (file mode 0600, the container's volume). It's a device key with no root identity behind it: clients know a seed by its device key, from a built-in list. | A seed starts unattended, so there's nobody to type a password. It holds no one's secrets: its key only proves it is the seed. Losing it means shipping a new seed key in the next release, not anyone's identity. |
| D3 | **The relay is upstream `iroh-relay` 1.4.1**, iroh's own relay server, in its own container next to the seed. Built from crates.io in our Dockerfile, the same pinned version as the client. TLS comes from the homelab's existing Caddy on a `relay.` subdomain; the relay itself listens on plain http inside the Docker network. | No relay code of ours to maintain, and nothing server-side gets linked into the client `mesh-node`. Caddy already terminates TLS for the homelab's other sites, so no new certificate setup. |
| D4 | **The relay is open to every iroh endpoint**, rate-limited per client (iroh-relay's `limits`). QUIC address discovery is off: it needs its own UDP port, and clients get the same help from n0's relays, which have it. | A relay can't tell Mesh traffic from other iroh traffic, and a token compiled into a public app isn't a secret. It forwards only ciphertext, so the cost is bandwidth; the rate limits cap that. |
| D5 | **Clients use our relay and n0's together** (`Relays::Custom` with both). iroh picks the home relay by latency, so ours wins from nearby. `--relay` still replaces the list. | Transport Q1: n0 until our own, then ours. Keeping n0 in the list means our homelab going down doesn't take Mesh offline. |
| D6 | **The seed is reached like any device**: a built-in list of seed tickets (`mesh://node/<key>?relay=<our relay>`) in `mesh-node`, plus `--seed <ticket>` (repeatable) to replace it. It has no public port: clients reach it through our relay, and go direct when hole punching allows, as between any two devices. | No discovery needed: seeds are few and known. A ticket never carries IPs (transport D3). Seed requests are a few small frames, so going through the relay costs nothing that matters. |
| D7 | **Friend-code services on the seed**, over transport streams (D6 there): `code-hold` (store a `HeldCode`), `code-get` (a `lookup_id` returns the encrypted record and counts a use) and `code-revoke` (a `CodeRevoke`). Any authenticated Mesh device may use them. | Identity's records are already designed for this: the seed sees a 16-byte lookup ID and ciphertext, never the ID it points to. |
| D7b | **The helper talks to seeds from a throwaway identity**, a fresh root and device key made at each unlock and never saved. *Added 2026-10-26 while planning.* | Transport's handshake shows each side the other's ID. Without this, a seed would see that Alice held a code and Bob fetched it, which is a friendship, even though it can't read the record. With it, the seed sees two strangers. |
| D8 | **Limits on the seed**, all enforced there: per device, 10 lookups a minute and 20 new held codes an hour; across everyone, 600 lookups a minute and 100,000 held codes; per source network (/24 or /48), 30 lookups a minute. A record is deleted when it expires, runs out of uses or is revoked. | A live-codes-per-person cap would need the seed to remember who held what, which D10 rules out; a rate doesn't. Keys are free to make, so per-device limits alone don't stop guessing; the per-network and global limits do. With 40-bit codes at the full 600 a minute, finding one *particular* code takes about 1,700 years on average. Finding *any* code among 100,000 live ones takes about two weeks, and gives the guesser a random stranger's ID (not access: the safety number and `social` still stand). The catch: someone using the whole global budget slows real lookups too. The per-network limit makes that take many networks; if it happens, the limits move. |
| D9 | **Storage is one file** in the data directory, rewritten atomically on change (`postcard`, already a dependency). | 100,000 records of about 200 bytes is 20 MB: no database needed. |
| D10 | **Nothing identifying is logged or kept.** No IPs, keys, lookup IDs or codes in logs; counts only (holds, lookups, refusals per hour). The store keeps no record of which device held or fetched a code; rate-limit counters (by throwaway device key and by network) live in memory for a minute or an hour. Nothing listens for admin: admin is ssh over Tailscale, and `docker logs`. | The map's hardening: the seed holds nothing readable and tells nothing about who used it. |
| D11 | **Deployment**: a `Dockerfile` (multi-stage, static binary, non-root, read-only root filesystem, `/data` volume) and a `compose.yaml` for the homelab with the seed and relay containers. A systemd unit too, for running without Docker (the later VPS). | The map: "deployed with Docker or systemd". |

## Behaviour

### Seed mode

- `mesh-node seed --data <dir> [--relay <url>]`. On first
  start it makes `seed.key` (0600) and prints its ticket, which goes into the
  built-in list.
- It runs a transport `Node` with its own key and a certificate it makes for
  itself (a root key generated alongside, kept in the same file; the
  certificate is what transport's handshake needs).
- It serves `ping`, `code-hold`, `code-get` and `code-revoke`. Unknown
  services reset the stream, as in transport.
- Every hour it sweeps expired records and logs one line of counts.

### Code services (D7, D8)

Each request is one stream: the service tag, one request frame, one answer
frame.

- `code-hold`: request `HeldCode` bytes. Answers:
  - `ok`: stored, or already held for the same owner (the record's revoke
    tag matches). A repeat hold changes nothing, so its uses aren't reset;
  - `gone`: this owner's code was used up or revoked; the client drops it
    from its list;
  - `taken`: a different owner's code has that `lookup_id` (a 40-bit
    collision, a fluke; the client makes another code);
  - `slow-down` (this device's 20 an hour), `full` (the seed's 100,000)
    or `bad` (doesn't parse, or already expired).
- When a record is used up or revoked, the seed keeps a marker (lookup ID,
  revoke tag, expiry: 56 bytes) until the record would have expired, or for
  good if it never expires, counted in the 100,000. Without it, a client
  re-holding its codes would bring a used single-use code back.
- `code-get`: request a 16-byte `lookup_id`. Answer the record (`found` +
  bytes), `none`, or `slow-down` (a limit hit). A found single-use record is
  deleted as it's sent.
- `code-revoke`: request `CodeRevoke` bytes. The seed deletes the record if
  the revoke matches (identity's `is_revoked_by`) and answers `ok` either way,
  so revokes reveal nothing.
- "Not found", "expired", "used up" and "revoked" all answer `none`: the
  seed doesn't help anyone learn which codes once existed.

### Client side (the helper)

- `codeCreate` now also hands the held record to a seed. `meshCodes` marks
  each code `held: true|false`. The helper holds every live code again when
  the node starts and retries any not yet held every 10 minutes, rebuilding
  the record from the keystore (identity gains `Keystore::held_record`). A
  `gone` answer drops the code from the list, so used-up codes disappear
  from Your ID too.
- `codeRevoke` sends the revoke to the seeds too.
- New `codeResolve(code)` → `meshResolved` (`{"id", "link"}`, after opening
  and checking the record), or `meshError` with `fn: codeResolve` and a kind:
  `code-bad-character`/`code-length` (as `parseId`), `not-found`,
  `slow-down`, `offline` (no seed reachable).
- `--seed <ticket>` (repeatable) replaces the built-in seeds; the relay list
  gains ours by default (D5).

### Mesh-Voice

On Your ID:

- **Friend codes**: "Make a code" with the expiry and uses choices
  (identity's defaults, 7 days and 1 use), the live codes with Copy and
  Revoke, and "not saved to a seed yet" on any that aren't held.
- **Check an ID** also takes a friend code: it resolves it, then shows the ID
  and the safety number as now.

## Tech stack

| Crate | For |
|---|---|
| `iroh-relay` 1.4.1, feature `server`, **only in the relay's Docker image**, built with `cargo install` | The relay (D3). Not a dependency of any crate of ours. |
| (nothing new in `mesh-node`) | Seed mode uses transport, identity, tokio and postcard, all already there. |

Container base images: `rust` (pinned) to build, `gcr.io/distroless/static`
(pinned digest) to run.

## Commands

```
cargo test --workspace                       # Mesh-Node, as now
cargo run -p mesh-node -- seed --data /tmp/seed
docker compose -f deploy/compose.yaml up -d  # on the homelab
cargo test -p mesh-app -p mesh-discord       # Mesh-Voice
```

## Project structure

```
Mesh-Node/
  crates/mesh-node/src/seed.rs         seed mode: node, services, sweep
  crates/mesh-node/src/codes.rs        the held-code store and its limits
  crates/mesh-node/src/helper.rs       codeResolve; hold/revoke via seeds
  crates/mesh-node/tests/seed.rs       a seed process and two helpers
  deploy/Dockerfile                    mesh-node (seed) image
  deploy/relay.Dockerfile              iroh-relay 1.4.1 image
  deploy/relay.toml                    relay config: http only, limits, QAD
  deploy/compose.yaml                  seed + relay on the homelab
  deploy/mesh-seed.service             systemd unit
Mesh-Voice/
  crates/mesh-discord/src/mesh.rs      meshResolved, held flags
  crates/mesh-app/src/mesh_ui.rs       Friend codes; Check an ID takes codes
```

The deploy files live in the private Mesh-Node repo; this public repo never
names the homelab's domain or address.

## Code style

As transport: small modules, typed errors, the limits as named constants
next to what they limit.

```rust
/// Lookups one device may make a minute (D8).
const LOOKUPS_PER_DEVICE: u32 = 10;
```

## Testing strategy

- **Unit tests** (`codes.rs`): hold, get, single use, uses counted down,
  expiry, revoke (right and wrong secret), every limit, the file surviving a
  restart, a corrupt file refused loudly rather than silently emptied.
- **Process test** (`tests/seed.rs`): a seed process and two helpers on the
  local http relay from transport's protocol test. Alice makes a code, Bob
  resolves it to Alice's ID; a second resolve gets `not-found`; Alice
  revokes another code and Bob's resolve gets `not-found`; past the lookup
  limit Bob gets `slow-down`. The seed's stderr holds no ID, key or code.
- **Homelab check** (by hand, the module gate):
  1. `docker compose up -d` on the homelab; from outside (phone hotspot),
     both machines show our relay as home (or n0's if it's closer, and ours
     still works when n0 is blocked with `--relay`).
  2. The Windows PC makes a code; the Linux box on the hotspot resolves it to
     the right ID and the safety numbers match.
  3. A single-use code fails the second time; a revoked code fails.
  4. Ping between the two machines through our relay (forced with
     `--relay`).
  5. No router setting was changed. From outside, only the relay (on the
     existing 443 via Caddy) is new; a port scan shows nothing else.
  6. The seed restarts (`docker compose restart`) and still holds its codes.

## Boundaries

- **Always:**
  - Every service behind transport's handshake; every limit enforced on the
    seed.
  - Counts-only logs (D10).
  - Pinned versions for images and crates.
- **Ask first:**
  - Anything that would need a port forwarded, on the homelab or anyone's
    router.
  - Any dependency not in the table above.
  - Changing D1–D11.
  - Adding services to the seed (rendezvous and mail come with their own
    modules).
- **Never:**
  - Keep or log IPs, keys, lookup IDs or codes.
  - An admin interface reachable from the internet.
  - Name the homelab's domain or address in the public Mesh-Voice repo.
  - Telemetry.

## Success criteria

1. Seed mode and the code services build and pass their tests on Linux;
   clippy and fmt clean.
2. The homelab runs seed and relay from `compose.yaml`, restarts by itself
   (`restart: unless-stopped`) and keeps codes across restarts.
3. A friend code made on one machine resolves on another, on a different
   network, in under 3 s (Argon2 included).
4. Single use, expiry and revoke hold, and the lookup limits answer
   `slow-down` (test).
5. Mesh shows our relay as home from the home network, and works with only
   our relay (`--relay`).
6. No port forwarding anywhere: none for people using Mesh, none added on
   the homelab. Nothing but the relay is reachable from outside, and the
   seed's logs hold no identifying data.

## Resolved questions (2026-10-26)

"Just do whatever you recommend. We can always change and fix after."

- **No port forwarding** for anyone, the homelab included (D0).
- **Q1.** A `relay.` subdomain through the homelab's Caddy and existing 443.
- **Q2.** None: D0.
- **Q3.** Device-link rendezvous later, as its own small task.
- **Q4.** Rendezvous by ID is `routing`'s.

## Open questions

None.
