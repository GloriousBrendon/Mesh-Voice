# Spec: `transport` (Mesh)

Status: **APPROVED 2026-10-26.** Plan: [PLAN-transport.md](PLAN-transport.md). Module 2 of the build order in
[CAPABILITY-MAP.md](CAPABILITY-MAP.md). Depends on
[`identity`](SPEC-identity.md) (done, Checkpoint D pending on Windows).
Replaces the original repo's TUNNEL_MANAGEMENT and WireGuard ADR.

## Objective

Let two Mesh devices open an encrypted connection to each other, dialled by
the device's public key, from anywhere: behind home routers, on changing IPs,
without admin rights, port forwarding or a VPN. When a direct path can't be
made, traffic goes through a relay, which only ever sees ciphertext.

This module is the pipe. It doesn't decide *who* to connect to or *where*
someone is: finding a person's devices by their ID is `routing`, running our
own relay is `seed`, and who's allowed to message you is `social`.

What this module delivers:

1. **An endpoint per device.** While the identity is unlocked, `mesh-node`
   runs an iroh endpoint whose key is the device key (identity D1), connected
   to a home relay.
2. **Dial by address.** Given a device's key and a relay to reach it through,
   connect, then upgrade to a direct path by hole punching when possible.
3. **Mutual Mesh authentication.** Both sides prove which identity they
   belong to: each sends its root-signed device certificate, and a connection
   whose certificate doesn't match the key it connected with is dropped.
   Upper layers only ever see an authenticated `(MeshId, device)` peer.
4. **Streams for upper layers.** A small framing layer: open a stream to a
   peer for a named service (`ping` now; `dm`, `mailbox`, `spaces` and
   `calls` later), send and receive length-prefixed messages.
5. **Staying connected.** Reuse one connection per peer device, reconnect
   with backoff when it drops, and keep working when the computer changes
   networks.
6. **A test path you can use by hand.** A connection ticket and a ping, from
   both the helper protocol and a small Network section in Mesh-Voice, so the
   two-machine check works before `routing` exists.

**User stories**

- I open Mesh and it says it's online, without me setting anything up.
- My friend and I are on different home networks. Our devices connect
  directly when our routers allow it, and through a relay when they don't.
  Either way it just works.
- I take my laptop from home Wi-Fi to a phone hotspot. Mesh reconnects by
  itself.
- At work, where UDP is blocked, Mesh still connects (through the relay, over
  HTTPS).
- Nobody who doesn't already know where I am can look up my IP address from
  my ID.

### Decisions this spec makes (review these)

A spike on 2026-10-26 backs D1–D4: two endpoints on the Linux box, with iroh
1.4.1, the real n0 relays, and keys built from our own 32 bytes.

| # | Decision | Why |
|---|---|---|
| D1 | **iroh 1.4.1**, the current release, as the QUIC + hole-punching + relay layer. | It does exactly the map's job (dial a key, hole punch, relay fallback), and it's what identity was designed around: the device key is the iroh key. Writing our own stays the fallback (map decision). |
| D2 | **No public address publishing.** Use iroh's `Minimal` preset, not `N0`. | `N0` publishes every node's current IPs and relay to n0's public DNS server, keyed by its public key: anyone who has your device key could look up your IP and when you're online. Spike: with `Minimal`, dialling by key alone fails with "No addressing information available". Finding devices by ID is `routing`'s job, done through friends and seeds. |
| D3 | **A Mesh address is a device key plus a relay URL**, and optionally IPs. Transport dials only what it's given. | Spike: key + relay URL alone connected in 0.3 s and switched to a direct path after the first ping (297 ms → under 1 ms on the LAN). IPs aren't needed to start, so addresses shared between people don't have to include them. |
| D4 | **Relays come from a configured list.** Until `seed` ships, the list is n0's four public relays. | They work today (spike). Relays see ciphertext, plus which device keys talk and their IPs. The seed module puts our own homelab relay first in the list (see Q1). |
| D5 | **Mutual Mesh handshake on every connection.** The first stream each side opens carries its `DeviceCert` (identity `device.rs`). Accept only if it verifies and its device key equals the key iroh authenticated (`remote_id`). | iroh proves who holds a device key; only the certificate proves which person that device belongs to. Checking against the root's current device list (revocation) needs the peer's list, which `routing`/`social` will supply; transport exposes a hook for it. |
| D6 | **ALPN `mesh/1`.** One QUIC connection per peer device; each request is a bidirectional stream that starts with a service tag and then carries length-prefixed frames (u32 length, at most 1 MiB per frame). | Simple, versioned, and lets each later module own its own messages. Large things (attachments, call media) get their own streams or datagrams in their modules. |
| D7 | **The endpoint runs only while the identity is unlocked.** Locking closes it. | It needs the device's secret key, which is only in memory while unlocked. |
| D8 | **`mesh-node` becomes async** (a tokio runtime). stdin is read on its own thread and fed into the runtime, the `mesh-dc` pattern. | iroh is async; the helper must answer commands while connections run. |
| D9 | **Inbound limits from day one:** a cap on concurrent connections and on streams per connection, and a timeout for the handshake. | A node anyone can dial needs limits before it's reachable. Who's *allowed* to talk to you (friends, blocks) is `social`. |
| D10 | **The test ticket**, `mesh://node/<device key>?relay=<url>`, holds no IP addresses. It's for testing only; people will reach each other by ID once `routing` lands. | Lets the two-machine check run now, without putting a home IP into something people paste around. |

## Behaviour

### Endpoint

- Built with `Endpoint::builder(presets::Minimal)`, the device secret key,
  `RelayMode::Custom(<configured list>)`, and ALPN `mesh/1`.
- On unlock: bind, wait until online (a home relay is connected; the spike
  took 3.5 s), and report it. If no relay is reachable within 20 s, report
  offline and keep retrying in the background.
- Reports, as `meshNet`: online or offline, the home relay, whether a direct
  path has been seen on this network, and the number of open connections.
  Never the IP addresses themselves, to keep them out of logs and screenshots.
- On lock: close every connection with a "locked" reason, then the endpoint.

### Connections

- `dial(addr)`: reuse an open, authenticated connection to that device, or
  connect and run the handshake (D5) within 10 s.
- The connection reports its current path: direct or relay, and the
  round-trip time (spike: `Connection::paths()`).
- When a connection drops, the next use reconnects. Transport has no retry
  loop of its own; modules that retry by themselves (`routing`, `mailbox`)
  pace their retries (changed 2026-10-26, Task 5).
- Network changes (Wi-Fi to hotspot, VPN on or off): connections recover by
  themselves. How iroh 1.4.1 detects the change (automatically, or through
  `Endpoint::network_change()`) is checked in the plan's first task, with a
  real switch on the laptop.

### Handshake (D5)

1. The dialler opens the first stream with service tag `hello` and sends its
   `DeviceCert`. The listener answers with its own.
2. Each side checks: the certificate parses and its root signature verifies;
   its device key equals the connection's `remote_id`.
3. Any failure closes the connection with a specific reason (`bad-cert`,
   `key-mismatch`, `timeout`), reported as an error to whoever dialled.
4. A device connecting to another device of the same identity is allowed;
   device linking (identity D8) will use that path.

### Services and framing (D6)

- Stream start: a service tag (1–32 printable ASCII bytes, length-prefixed).
- Then frames: `u32` big-endian length + bytes. A frame over 1 MiB, or an
  unknown service, closes that stream with an error, not the connection.
- Transport's own services: `hello` and `ping` (echoes an 8-byte nonce;
  used for the round-trip time). Upper modules register theirs.

### Helper protocol

The same JSON lines as identity. New commands:

- `netStatus` → `meshNet` (online, home relay, direct seen, connections).
  Also sent by itself on every change.
- `netTicket` → `meshTicket` (`mesh://node/…` for this device).
- `netPing(ticket)` → `meshPing` (the peer's `mesh1…` ID and device name from
  its certificate, `direct` or `relay`, round-trip time in ms), or
  `meshError` with `fn: netPing` and a kind: `bad-ticket`, `unreachable`,
  `timeout`, `bad-cert`, `key-mismatch`, `offline` (this device isn't online).

### Mesh-Voice

A **Network** section on Your ID (Mesh's Accounts panel):

- a status line: "Online · relay euc1" (or "Connecting…" or "Offline: no
  relay reachable");
- "Copy test ticket";
- a field to paste someone's ticket, and a **Ping** button. It shows who
  answered (their ID and device name) and the path and time, for example
  "direct · 24 ms".

## Tech stack

New in **Mesh-Node**:

| Crate | For |
|---|---|
| `iroh` 1.4.1, `default-features = false`, features `tls-ring` and `portmapper` | The endpoint. `portmapper` asks the home router to open a port (UPnP / NAT-PMP / PCP) when it allows it, which helps direct paths. Leaving out the default `metrics` keeps it lighter; nothing in metrics leaves the machine either way. |
| `tokio` 1 | The async runtime (D8). |
| `url` (if not re-exported by iroh) | Parsing the ticket's relay URL. |

`iroh` brings `ed25519-dalek` 3, the version identity already uses, so the
device key converts without copying through an intermediate type.

## Commands

Mesh-Node (repository root):

```sh
cargo test -p mesh-transport                  # in-process tests, no internet
cargo test -p mesh-transport -- --ignored     # against the real n0 relays
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
printf '%s\n' '{"fn":"idUnlock","args":["u","p"]}' '{"fn":"netTicket","args":[]}' \
  | ./target/release/mesh-node --data /tmp/mn-a
```

Mesh-Voice: `cargo build -p mesh-app && cargo test -p mesh-app`.

## Project structure

```
Mesh-Node/
  crates/mesh-transport/     this module: a library
    src/lib.rs               public API: Node, MeshAddr, Peer, Service
    src/endpoint.rs          building the iroh endpoint, relays, status
    src/handshake.rs         the hello exchange and its checks (D5)
    src/frame.rs             service tags and length-prefixed frames (D6)
    src/ticket.rs            mesh://node/ tickets (D10)
    tests/two_nodes.rs       two endpoints in one process
  crates/mesh-node/          helper: async, net* commands
Mesh-Voice/
  crates/mesh-discord/src/mesh.rs   net* commands and events
  crates/mesh-app/src/mesh_ui.rs    the Network section
```

## Code style

As identity: small modules, typed errors the UI can switch on, a doc comment
per public item, comments for *why*, no `unwrap` outside tests.

```rust
/// Checks the peer's hello: its certificate must be signed by its root and
/// name the same device key iroh authenticated, or we're talking to someone
/// pretending to be that person.
fn check_hello(cert: &[u8], remote: &iroh::PublicKey) -> Result<Peer, HandshakeError> {
    let cert = DeviceCert::from_bytes(cert).map_err(|_| HandshakeError::BadCert)?;
    if cert.device().as_bytes() != remote.as_bytes() {
        return Err(HandshakeError::KeyMismatch);
    }
    Ok(Peer { id: cert.root(), device: *remote, name: cert.name().to_owned() })
}
```

## Testing strategy

- **Unit tests** in each file: frame encoding and every limit (empty, 1 MiB,
  over 1 MiB, a truncated length); ticket round-trip and every malformed
  ticket; handshake checks (bad signature, key mismatch, garbage).
- **In-process tests** (`tests/two_nodes.rs`) with relays disabled and direct
  local addresses, so they need no internet: connect, handshake, ping;
  connection reuse; a peer presenting someone else's certificate is refused;
  the stream and connection limits (D9); lock closes everything.
- **Relay path without the internet:** iroh's `test-utils` feature runs a
  local relay server in-process. A test forces relay-only and checks that a
  ping still works and reports `relay`. `test-utils` is a dev-dependency only
  (ask first, per Boundaries).
- **Against the real relays** (`--ignored`): two endpoints in one process
  through n0's relays, as in the spike.
- **Helper**: the protocol test drives `mesh-node` through unlock → netTicket
  → netPing between two helper processes.
- **Two-machine check** (the module gate), recorded in the plan:
  1. The Windows PC on the home network, the Linux box on a phone hotspot.
     Swap tickets, ping both ways: it connects, and shows the path.
  2. Force the relay (block UDP to the other machine with a firewall rule):
     ping still works and reports `relay`.
  3. Switch the Linux box from the hotspot to Wi-Fi during a series of pings:
     they recover without restarting anything.
  4. On a network that blocks UDP (if one's available, such as work): Mesh
     still comes online through the relay.
  5. Windows: no admin prompt. If Windows Firewall asks to allow Mesh,
     cancelling it still lets Mesh connect (outbound hole punching and the
     relay don't need it).

## Boundaries

- **Always:**
  - Authenticate every peer with D5 before any upper service sees it.
  - Limits on everything inbound (D9).
  - Keep IP addresses out of logs, `meshNet` and the UI.
  - Close connections cleanly on lock.
- **Ask first:**
  - Any dependency not in the table above, including iroh's `test-utils`.
  - Changing D1–D10, especially turning on any address publishing (D2).
  - Adding relays other than n0's and our own.
  - Anything that decides who may connect (that's `social`).
- **Never:**
  - Publish this device's addresses anywhere public.
  - Telemetry or metrics that leave the machine.
  - Plaintext traffic, or a fallback that skips the handshake.
  - Kernel drivers, TUN devices or anything that needs admin rights.

## Success criteria

1. `mesh-transport` builds and its tests pass on Windows and Linux, with
   clippy at `-D warnings` and `fmt --check` clean.
2. Mesh comes online within 10 s of unlocking on a normal home connection,
   and Your ID shows it.
3. The two-machine check passes: direct when the networks allow it, relay
   when they don't, recovery after a network switch, and Windows without admin
   rights.
4. A peer presenting a certificate that doesn't match its key is refused, in
   a test, with `key-mismatch` / `bad-cert`.
5. Dialling a device by key alone, with no relay or IPs given, fails: nothing
   about this device is published (test, as in the spike).
6. Idle cost while unlocked: no measurable CPU use, and `mesh-node` under
   about 60 MB of memory with no connections (measured on both machines,
   recorded in the plan).
7. Locking closes the endpoint: a ping to it afterwards fails with
   `unreachable`.

## Resolved questions (2026-10-26)

- **Q1. Relays until `seed` exists?** n0's public relays now; our own homelab
  relay comes with `seed` (D4).
- **Q2. Direct connections show each other's IP.** Direct for everyone you
  connect to in v1; `social` decides whether non-friends get relay-only.
- **Q3. The Network section in Mesh-Voice?** Yes: status, test ticket and
  ping on Your ID.

## Open questions

None left for this module.
