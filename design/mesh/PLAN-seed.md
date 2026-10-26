# Plan: `seed` (Mesh)

Status: **APPROVED 2026-10-26** ("just do whatever you recommend"). Implements
[SPEC-seed.md](SPEC-seed.md).

The code goes in `GloriousBrendon/Mesh-Node` (seed mode, the code store, the
helper's side, and `deploy/`) and Mesh-Voice (Friend codes on Your ID). The
homelab's domain and address appear only in the private Mesh-Node repo.

## Overview

The code store first (pure, unit-tested), then seed mode around it, then the
helper's side and a process test with a seed and two helpers. Then the
Mesh-Voice screens. Then the deploy files, tried locally in Docker. The
module ends on the homelab, which is the one hard stop: it changes live
infrastructure.

## Architecture decisions

- **The store knows nothing about networking.** `codes.rs` takes requests
  with a caller (device key, source network) and a clock, and answers; seed
  mode wires it to transport streams. That keeps every limit testable
  without sockets.
- **Seed mode is a second entry point of `mesh-node`**, not a new crate:
  `seed.rs` next to `helper.rs`, sharing transport.
- **One small identity change:** `Keystore::held_record(&ActiveCode)`
  rebuilds a code's `HeldCode` (same lookup ID, fresh nonce) so the helper
  can hold codes again. Nothing in the keystore format changes.
- **The built-in seed and relay lists start empty** and are filled in Task
  7, once the homelab gives the seed's ticket and relay URL. Until then,
  tests and local runs use `--seed` and `--relay`.
- **No new crate dependencies.** `iroh-relay` is built only inside the relay
  image.

## Task list

### Phase 1: The seed (Mesh-Node)

- [x] **Task 1: The held-code store** (D7, D8, D9)
  - What: `codes.rs`: hold, get, revoke with the spec's answers; markers for
    used-up and revoked codes; per-device, per-network and global limits;
    expiry sweep; save to one file atomically, load it back, refuse a
    corrupt file.
  - Accept: unit tests for every answer, every limit, single and counted
    uses, expiry, revoke with right and wrong secret, restart, corrupt file.
  - Verify: `cargo test -p mesh-node codes`.
  - Files: `crates/mesh-node/src/codes.rs`, `main.rs` (the `mod`).
  - Size M.
  - Result (2026-10-26): 10 unit tests pass, covering every answer and
    limit, a repeat hold keeping its count, restart and a damaged file.
    Identity gained `HeldCode::revoke_tag`, so the store doesn't read the
    record's layout itself. 80 workspace tests pass; clippy and fmt clean.
- [x] **Task 2: Seed mode** (D1, D2, D6, D10)
  - What: `mesh-node seed --data <dir> [--relay <url>]`: makes or loads
    `seed.key` (0600), prints its ticket, runs a node serving `ping` and the
    three code services, sweeps hourly, logs counts only, stops on SIGTERM
    and saves. The caller's network comes from the connection's direct path;
    a relayed caller has none, so only the per-device and global limits
    apply to it.
  - Accept: a test starts a seed process and holds, gets and revokes a code
    over a real connection; stderr has no keys, IDs or lookup IDs.
  - Verify: `cargo test -p mesh-node`.
  - Files: `crates/mesh-node/src/{seed.rs,main.rs}`, `crates/mesh-transport`
    (if a service hook is needed), `crates/mesh-node/tests/seed.rs`.
  - Size M.
  - Result (2026-10-26): transport gained service streams
    (`Config::services`, `Node::next_stream`, with the caller's IP only
    while direct) and `Node::open` for clients, plus a test. The process
    test holds, fetches (single use), revokes, sends junk, stops the seed
    with SIGTERM and starts it again: same ticket, and a used code stays
    `gone`. The key file is 0600; the log holds no ID, code or lookup ID.
    The store is saved every 5 s when changed, and on stop. On n0's relays
    a fresh seed has its ticket in about 3.5 s. 83 workspace tests pass.
- [x] **Task 3: The helper uses seeds**
  - What: `--seed <ticket>`; `codeCreate` holds, `codeRevoke` revokes,
    holding again on node start and every 10 minutes; `held` in
    `meshCodes`; `gone` drops a code; `codeResolve(code)` → `meshResolved`
    or the spec's error kinds. `Keystore::held_record`. Seed requests go
    out from a second node with a throwaway identity (D7b).
  - Accept: the process test with a seed and two helpers (spec "Process
    test"): resolve, single use, revoke, `slow-down`, and a code made while
    the seed is down gets held once it's back.
  - Verify: `cargo test --workspace`.
  - Files: `crates/mesh-node/src/helper.rs`, `crates/mesh-identity/src/keystore.rs`,
    `crates/mesh-node/tests/seed.rs`.
  - Size M.
  - Result (2026-10-26): the process test passes, stable over three runs:
    Bob resolves Alice's single-use code once, then `not-found` (code or
    link); after Alice unlocks again it leaves her list; a revoked code
    finds nothing; Bob gets `slow-down` within 15 guesses; a code made while
    the seed is down is held after the seed is back and Alice unlocks. No
    ID or code in the seed's log. `codeResolve` takes `K7QM-3XPA` or a
    `mesh://code/` link. Idle cost with the second node: 0.13% of a core,
    44.9 MiB (was 37.5). 84 workspace tests pass; clippy and fmt clean.

**Checkpoint A result (2026-10-26):** seed and helper work end to end on
this box.

**Checkpoint A (short report):** seed and helper work end to end on this box.

### Phase 2: Mesh-Voice

- [ ] **Task 4: Friend codes on Your ID**
  - What: `meshCodes` (with `held`) and `meshResolved` in `mesh.rs`; a
    Friend codes section (make with expiry and uses, copy, revoke, "not
    saved to a seed yet"); Check an ID takes a code too.
  - Accept: unit tests for the new events; by hand on this box with a local
    seed: make a code in one window, resolve it in the other.
  - Verify: `cargo test -p mesh-app -p mesh-discord`.
  - Files: `crates/mesh-discord/src/mesh.rs`, `crates/mesh-app/src/mesh_ui.rs`.
  - Size M.

### Phase 3: Deployment

- [ ] **Task 5: Images and compose** (D3, D4, D11)
  - What: `deploy/Dockerfile` (seed), `deploy/relay.Dockerfile`
    (`iroh-relay` 1.4.1), `deploy/relay.toml` (http only, limits, no QUIC
    address discovery, metrics off), `deploy/compose.yaml`,
    `deploy/mesh-seed.service`.
  - Accept: `docker compose up` on this box runs both; a helper with
    `--relay http://localhost:…` and `--seed <ticket>` resolves a code
    through them; containers run as non-root with read-only root
    filesystems.
  - Verify: by hand on this box, recorded here.
  - Files: `deploy/*` in Mesh-Node.
  - Size M.

**Checkpoint B (hard stop, needs your OK):** before anything touches the
homelab. I'll list exactly what changes there: a DNS record for the
`relay.` name (Cloudflare, DNS only), one Caddy site block, and the compose
project in its own folder.

- [ ] **Task 6: On the homelab**
  - What: the DNS record, the Caddy block, `docker compose up -d`; note the
    seed's ticket.
  - Accept: from a phone hotspot, a helper with only our relay comes online
    and resolves a code through the seed; a scan shows nothing new open.
  - Verify: by hand, recorded here.
  - Files: none in the repos (the homelab's own config).
  - Size S.
- [ ] **Task 7: Built-in lists** (D5, D6)
  - What: our relay plus n0's as the default relay list; the seed's ticket
    as the built-in seed. Installers unchanged.
  - Accept: a fresh helper with no flags shows our relay as home from here
    and resolves codes.
  - Verify: `cargo test --workspace`; by hand.
  - Files: `crates/mesh-node/src/{main.rs,helper.rs}`.
  - Size XS.

**Checkpoint C (module gate, needs you):** the spec's homelab check across
two machines on different networks. It can share a sitting with
transport's Checkpoint C and identity's Checkpoint D on the Windows PC.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| The homelab is down or its home connection drops | Medium | n0's relays stay in the list (D5), so Mesh stays online; codes don't resolve until it's back. A VPS seed comes after testing (map). |
| Caddy proxying the relay's websocket upgrade | Low | Caddy's `reverse_proxy` handles websockets by default; Task 5 runs the same setup locally first. |
| Guessing codes at the global limit slows real lookups | Low | Per-network limit first; the counts in the log show it; limits move if needed (spec D8). |
| Someone fills the store with junk codes | Low | 20 new an hour per device, 100,000 total, expiry; a full store refuses new holds, it doesn't drop real ones. Keys are free, so a determined filler can reach the cap; the counts in the log show it. |
| The throwaway node (D7b) costs a second endpoint while unlocked | Low | It's idle almost always; Task 3 re-measures idle cost. |
| The seed's key file is copied off the homelab | Low | It proves only "I'm the seed"; the copier still can't read any record. Rotating means a new key in the next release. |

## Open questions

None.
