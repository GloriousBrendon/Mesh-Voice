# Plan: `transport` (Mesh)

Status: **APPROVED 2026-10-26.** Implements
[SPEC-transport.md](SPEC-transport.md) (approved 2026-10-26).

As with identity, the plan and its checklist live here; the code goes in
`GloriousBrendon/Mesh-Node` (`crates/mesh-transport`, the `mesh-node`
helper) and Mesh-Voice (glue and the Network section).

## Overview

Build `mesh-transport` in Mesh-Node: the pure parts first (frames, tickets,
the key conversion), then the endpoint with the handshake, then ping,
connection reuse and limits, then the relay path. Then make the helper async
and give it the `net*` commands, then the Network section in Mesh-Voice. The
module ends with the two-machine check on the Windows PC and the Linux box.

## Architecture decisions

- **Same layering as identity:** library, then helper, then UI, each tested
  before the next uses it.
- **One file per concept** in `mesh-transport`, as the spec's structure
  lays out: `endpoint`, `handshake`, `frame`, `ticket`, with the public API
  (`Node`, `MeshAddr`, `Peer`, `Service`) in `lib.rs`.
- **The device key passes straight through.** `DeviceKey::secret_bytes()`
  (32 bytes, zeroized) becomes iroh's `SecretKey::from_bytes`; iroh 1.4.1
  accepts `ed25519-dalek` 3, the version identity uses. No identity changes.
- **Tests don't need the internet.** In-process tests use direct local
  addresses with relays disabled, and the relay path uses a local relay from
  iroh's `test-utils` (dev-dependency only). Real n0 relays are `--ignored`
  tests.
- **The helper takes `--relay <url>`** (repeatable), overriding the default
  n0 list. The protocol test points two helpers at a local test relay; later,
  `seed` uses the same flag (or its config) for our own relay.
- **Errors are typed** (`TransportError`, `HandshakeError`, `TicketError`)
  and map to the spec's `meshError` kinds.
- **Crate versions checked 2026-10-26:** `iroh` =1.4.1 (`default-features =
  false`, `tls-ring` + `portmapper`), `tokio` 1; `iroh` with `test-utils` as a
  dev-dependency (you approved the spec, which lists it as ask-first; I'm
  asking here: approve the plan and it's in).

## Task list

### Phase 1: Foundation

- [x] **Task 1: Scaffold `mesh-transport`, and answer the unknowns** (Mesh-Node `1e74a9d`)
  - What:
    - the crate with its dependencies;
    - `fn secret_key(&DeviceKey) -> iroh::SecretKey`, with a test that iroh's
      public key equals the identity's device key;
    - read iroh 1.4.1's network-change handling (automatic, or
      `Endpoint::network_change()`) and write the answer into this plan;
    - record how much `mesh-node`'s release binary grows.
  - Accept: the workspace builds; clippy and fmt clean; the key test passes;
    the network-change answer and the binary size are in this plan.
  - Verify: the spec's Commands.
  - Files: `Cargo.toml`, `crates/mesh-transport/{Cargo.toml,src/lib.rs}`.
  - Size S.
  - **Result (2026-10-26):** done. 48 workspace tests pass; clippy and fmt
    clean.
    - **Network changes:** iroh runs its own network monitor (`netwatch`)
      and detects changes by itself on desktop systems.
      `Endpoint::network_change()` exists for platforms like Android that
      hide those events from native code; calling it is harmless but not
      needed. So no extra code; the gate tests a real switch.
    - **Binary size:** iroh with our features adds about 14 MiB stripped
      (measured on the spike: 15.6 MiB against 0.6 MiB for tokio alone).
      `mesh-node` is 7.1 MiB stripped now (9.4 MiB as built), so expect
      about 22 MiB once Task 8 links iroh in. Neither repo strips release
      builds yet; `strip = true` would be a later, separate change.
    - **Duplicate crates:** iroh's QUIC stack brings older copies of
      `aead`, `aes`, `cipher` and `getrandom` (through `aes-gcm` 0.10 and
      `ring`), and its network monitor two `netlink` versions. They're inside
      iroh, so not ours to fix; noted.
- [x] **Task 2: Frames and service tags** (`frame.rs`) (Mesh-Node `f9014d7`)
  - What: write and read a service tag, then length-prefixed frames, over
    any async stream; the 32-byte tag and 1 MiB frame limits.
  - Accept: unit tests for round trips, an empty frame, exactly 1 MiB, over
    1 MiB, a truncated length, a bad tag.
  - Verify: `cargo test -p mesh-transport`.
  - Files: `crates/mesh-transport/src/frame.rs`.
  - Size S.
- [x] **Task 3: Test tickets** (`ticket.rs`) (Mesh-Node `f9014d7`)
  - What: `mesh://node/<device key>?relay=<url>` to and from `MeshAddr`
    (D10), with no IPs.
  - Accept: round trip; each malformed form (wrong scheme, bad key, missing
    or bad relay URL) gives its own `TicketError`; a committed test vector.
  - Verify: `cargo test -p mesh-transport`.
  - Files: `crates/mesh-transport/src/ticket.rs`, `tests/vectors.rs`.
  - Size S.

**Checkpoint A (short report):** pure parts done and tested; the network
change and binary size answers recorded.

**Checkpoint A result (2026-10-26):** done. 55 workspace tests pass (7 new
in `mesh-transport` plus its vector); clippy and fmt clean.
- Frames: round trips, an empty frame, exactly 1 MiB, over 1 MiB on write
  and on read (a claimed 4 GiB length fails before allocating), a cut-off
  length, body and tag, and bad tags.
- Tags are printable ASCII (1–32 bytes) rather than any UTF-8: stricter and
  simpler, and every planned service name fits. The spec says so now.
- Tickets: round trip (also with spaces around a paste); wrong scheme,
  missing relay, damaged key, an invalid key, a damaged relay and a non-HTTP
  relay each give their own error. The vector's key matches OpenSSL's.

### Phase 2: Connections (the risky part)

- [x] **Task 4: Endpoint and handshake** (`endpoint.rs`, `handshake.rs`) (Mesh-Node `7a6f9b4`)
  - What:
    - `Node::start(device key, own cert, relays)` binds with the `Minimal`
      preset and ALPN `mesh/1`;
    - `Node::dial(MeshAddr) -> Peer` and an accept loop;
    - the hello exchange and checks (D5), with a 10 s timeout.
  - Accept (in-process, relays disabled, direct local addresses): two nodes
    connect and each sees the other's `MeshId` and device name; a node
    presenting another identity's certificate gets `key-mismatch`; a garbage
    hello gets `bad-cert`; a silent peer gets `timeout`; dialling by key alone
    fails (success criterion 5).
  - Verify: `cargo test -p mesh-transport`.
  - Files: `src/{lib.rs,endpoint.rs,handshake.rs}`, `tests/two_nodes.rs`.
  - Size M.
  - **Result (2026-10-26):** done. 64 workspace tests pass; clippy and fmt
    clean; the two-node tests passed 5 runs in a row.
    - Both sides authenticate each other over real QUIC on loopback. Raw
      iroh endpoints play the bad peers: another identity's certificate gets
      `key-mismatch` in both directions, garbage gets `bad-cert`, a silent
      dialler and a silent listener both time out, and a key alone is
      `Unreachable`. `Node::start` refuses a certificate that isn't its own.
    - Close codes carry the reason to the other side (1 bad-cert,
      2 key-mismatch, 3 timeout, 4 dropped).
    - `MeshAddr` now has an optional relay and direct addresses (spec D3
      allows both); tickets still require a relay and never carry IPs.
    - Left for Task 5, as planned: limits on incoming connections and
      handshakes (D9), and a bound on the queue of newly accepted peers.
- [x] **Task 5: Ping, reuse, limits and lock** (Mesh-Node `d768eb7`)
  - What:
    - the `ping` service (8-byte nonce) returning the round-trip time and the
      current path (direct or relay);
    - one connection per peer device, reused, with reconnect and backoff;
    - the D9 limits (concurrent connections, streams per connection);
    - `Node::status()` (online, home relay, direct seen, connections) and
      `Node::close()`.
  - Accept: ping works and reports its path; a second dial reuses the
    connection; going over a limit is refused while existing connections keep
    working; after `close`, dialling the node fails.
  - Verify: `cargo test -p mesh-transport`.
  - Files: `src/{lib.rs,endpoint.rs}`, `tests/two_nodes.rs`.
  - Size M.
  - **Result (2026-10-26):** done. 68 workspace tests pass (11 two-node
    tests, stable over 5 runs); clippy and fmt clean.
    - Ping reports who answered, the round-trip time and the path (direct
      on loopback); repeated pings and dials reuse one connection, and the
      other side sees one peer.
    - Over the connection limit, a new device is refused while the existing
      one keeps working. The stream cap holds: with 2 allowed, a third stream
      waits. After `close`, pinging the node is `Unreachable`.
    - `Node::start` takes a `Config` (relays and limits; defaults 256
      connections, 32 handshakes, 32 streams).
    - **Changed from the spec: no retry backoff in transport.** Transport
      connects only when asked and has no retry loop of its own, so there's
      nothing to pace. A failed dial reconnects on the next call; modules
      that retry by themselves (`routing`, `mailbox`) pace their retries.
      The spec says so now.
- [x] **Task 6: The relay path** (Mesh-Node `523c5b5`)
  - What: a test with a local relay (`test-utils`) and direct paths blocked,
    so traffic must go through the relay; an `--ignored` test against n0's
    relays, as in the spike.
  - Accept: ping works through the local relay and reports `relay`; the
    ignored test passes on the Linux box.
  - Verify: `cargo test -p mesh-transport`, then with `-- --ignored`.
  - Files: `tests/relay.rs`, `crates/mesh-transport/Cargo.toml`.
  - Size S.
  - **Result (2026-10-26):** done.
    - Offline: with iroh's in-process relay and direct paths switched off,
      a ping dialled with only the key and relay goes through the relay and
      reports `relay`.
    - Real n0 relays (`--ignored`): dialled with key and relay only, it was
      on a direct path from the first ping (about 0.6 ms on this box); home
      relay `aps1`, as in the spike.
    - The test relay needs its self-signed certificate trusted, which iroh
      only allows with its `test-utils` feature. `mesh-transport` has its own
      `test-utils` feature (trust the test relay; no direct paths) that only
      its tests turn on; `cargo tree` shows it in neither `mesh-node` nor
      any normal build.
    - Found on the way: iroh can drop direct paths entirely
      (`clear_ip_transports`), which is what a "hide my IP (relay only)"
      setting would use. That answers the open part of spec Q2 for when
      `social` decides it.

**Checkpoint B (short report):** the library is done, with the test counts,
and the ping times through the real relays.

**Checkpoint B result (2026-10-26):** the library is done. 69 workspace
tests pass (22 in `mesh-transport`, stable over repeated runs), plus 1
ignored real-relay test that passes; clippy and fmt clean. Ping through
n0's relays: direct from the first ping, about 0.6 ms between two nodes on
this box.

### Phase 3: Helper and Mesh-Voice

- [x] **Task 7: An async helper** (D8)
  - What: `mesh-node` runs a tokio runtime; stdin is read on a thread and
    fed in (the mesh-dc pattern); identity commands are unchanged.
  - Accept: the existing protocol test passes unchanged; commands are still
    never logged.
  - Verify: `cargo test -p mesh-node`.
  - Files: `crates/mesh-node/src/{main.rs,helper.rs}`.
  - Size S.
  - Result (2026-10-26): done as planned; the protocol test passed
    unchanged. Identity commands run on the runtime's own thread
    (`block_on`), so a second of Argon2 doesn't hold up the workers.
- [x] **Task 8: The `net*` commands**
  - What:
    - the node starts when the identity opens (create, unlock, import, or
      kept unlocked at start) and stops on lock (D7);
    - `meshNet` sent on every change; `netStatus`, `netTicket`,
      `netPing(ticket)` with the spec's error kinds;
    - `--relay <url>` (repeatable).
  - Accept: a protocol test runs two helpers against a local test relay:
    unlock both → ticket → ping both ways → `meshPing` names the right ID;
    lock → the other's ping fails with `unreachable`; IPs never appear in
    `meshNet`.
  - Verify: `cargo test -p mesh-node`; by hand with two data directories and
    the real relays.
  - Files: `crates/mesh-node/src/helper.rs`, `crates/mesh-node/tests/protocol.rs`,
    `crates/mesh-node/Cargo.toml`.
  - Size M.
  - Result (2026-10-26):
    - Test relay: iroh's relay client takes plain `http://` relays, so the
      test runs an http relay on loopback (`iroh-relay` `server`, as a
      dev-dependency of `mesh-node`, added to the spec's table). `--relay`
      accepts `http` only on loopback. Nothing in `mesh-node` trusts a
      self-signed relay, in any build.
    - The test passes, stable over repeated runs: both ping each other, one
      connection reused both ways, `meshNet` holds only state, relay,
      direct and connections; after Alice locks, Bob's ping is
      `unreachable` (after the 10 s connect timeout, so the test takes
      about 17 s). The identity test now uses the local relay too, so
      neither test needs the internet.
    - By hand on n0's relays, two helpers with their own data directories:
      both online in 3 to 4 s from create; once 27 s (n0 or the network;
      the spike saw one slow first connect too). Pings direct from the
      first, 0.6 to 0.8 ms, home relay `aps1`. stderr empty.
    - `meshNet` comes from polling the node once a second (a `ponytail:`
      note); Task 10 checks the idle cost, and iroh's watchers replace it
      if it shows.
    - 70 workspace tests pass; clippy and fmt clean.
- [x] **Task 9: Network section in Mesh-Voice** (by-hand check pending)
  - What: `meshNet`, `meshTicket` and `meshPing` in `mesh.rs`; on Your ID, a
    status line, "Copy test ticket", and a paste field with **Ping** showing
    who answered, the path and the time.
  - Accept: on the Linux box, two Mesh instances (two data directories) ping
    each other through the app.
  - Verify: `cargo test -p mesh-app -p mesh-discord`; you, by hand.
  - Files: `crates/mesh-discord/src/mesh.rs`, `crates/mesh-app/src/mesh_ui.rs`.
  - Size M.
  - Result (2026-10-26): built; your by-hand check is still to do.
    - Your ID has a Network section between Check an ID and Devices: the
      status line ("Online · relay aps1 · 1 connection, direct",
      "Connecting…", "Offline"), Copy test ticket, a ticket field and Ping,
      which shows "box answered · direct · 0.7 ms" and their ID.
    - Errors show under the button with the helper's fixed messages.
    - Two windows on one machine: start the second with its own data
      folder, `XDG_DATA_HOME=/tmp/meshB mesh-app` (no code needed).
    - Tests: 25 in `mesh-app`, 15 in `mesh-discord` pass; it launches with
      the new helper and no errors.
- [ ] **Task 10: Idle cost**
  - What: measure `mesh-node`'s CPU and memory unlocked with no connections
    (success criterion 6) on the Linux box; record it.
  - Accept: the numbers are in this plan; if over the target, a note on why
    and what to change.
  - Verify: `ps`/`top` over a few minutes.
  - Files: this plan.
  - Size XS.

**Checkpoint C: module gate (needs you on the Windows PC).** The spec's
two-machine check: the Windows PC at home and the Linux box on a phone
hotspot; ping both ways; forced relay; a network switch during pings; a UDP
blocked network if one's available; Windows without admin rights (and what
the firewall prompt does). Idle numbers on Windows too. This can share a
sitting with identity's Checkpoint D.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| iroh doesn't notice network changes by itself on Linux or Windows | Medium | Task 1 reads how 1.4.1 handles it; if it needs telling, the helper calls `Endpoint::network_change()` on the OS's network events. The gate tests a real switch. |
| n0's public relays are slow from here (the spike picked Asia-Pacific, 250 ms), rate-limited or down | Medium | Only until `seed`: `--relay` already points the node at any relay, and `seed` puts ours first. |
| iroh's API changes in a 1.x release | Low | Pinned to `=1.4.1`; upgrades are a deliberate change with tests. |
| The relay-path test needs `test-utils`, which pulls in a relay server | Low | Dev-dependency only, not in the shipped binary. |
| Task 8's protocol test planned two `mesh-node` processes on a local test relay, but a normal `mesh-node` won't trust its self-signed certificate | Low | Resolved in Task 8: the test relay is plain http on loopback, which `--relay` allows only there. No trust switch anywhere. |
| Windows Firewall asks to allow `mesh-node` | Low | Expected for anything that listens. The gate checks that cancelling it still connects (outbound hole punching and the relay). |
| The binary grows a lot (rustls, QUIC) | Low | Task 1 records it. `metrics` is off; more features can be trimmed if needed. |
| The device secret is handed to iroh | Medium | It stays in the process, built from the zeroized bytes, only while unlocked; locking drops the endpoint (D7). |

## Parallel work

Tasks 2 and 3 are independent of each other; Task 7 is independent of
Tasks 2–6. I'll do them in order unless you want otherwise.

## Resolved questions (2026-10-26)

- **P1. Checkpoints.** Short reports at A and B; a hard stop at C, the module
  gate on Windows. `test-utils` as a dev-dependency approved with the plan.
