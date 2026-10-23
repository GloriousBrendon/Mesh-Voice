# Plan: `identity` (Mesh)

Status: **APPROVED 2026-10-22.** Implements
[SPEC-identity.md](SPEC-identity.md) (approved 2026-10-22).

The plan and its task checklist live together here, not in `tasks/`. The
handoff keeps all Mesh planning in Mesh-Voice; the code goes in
`GloriousBrendon/Mesh-Node`.

## Overview

Build the `mesh-identity` library in Mesh-Node first: IDs, device
certificates, the keystore, friend codes, the safety number and linking, each
a small file with its own tests and test vectors. Then add the `mesh-node`
helper binary that speaks the stdio protocol. Then the Mesh-Voice screens:
onboarding, unlock, Your ID, backup, change password, and an ID checker with
the safety number. The module ends with the two-machine check from the spec
on the Windows PC and the Linux box.

## Architecture decisions

- **Library first, helper second, UI last.** Each layer is tested before the
  next uses it. Each layer is still a vertical slice: after each phase there's
  something you can run (tests, then a helper you drive by hand, then
  screens).
- **One file per concept** in `mesh-identity` (`id`, `device`, `password`,
  `keystore`, `code`, `safety`, `link`), each owning its encode/decode, signing
  layout and tests, as the spec's structure section lays out.
- **Signed bytes written by hand** with a domain string per type (spec), never
  serde. Serde with postcard is used only inside the encrypted keystore
  payload.
- **Errors are typed per area** (`IdError`, `KeystoreError`, `CodeError`,
  `LinkError`) with `thiserror`. The helper turns them into a `meshError`
  event with a stable `kind` the UI can switch on.
- **Mesh in Mesh-Voice is a network** in the existing registry
  (`mesh-core/src/networks.rs`) with its own Accounts card, like WhatsApp and
  Discord. Its helper glue mirrors `mesh-wa`: it's found next to Mesh's binary
  and driven over stdio, with the same restart and backoff.
- **Crate versions checked 2026-10-22:**
  - `ed25519-dalek` 3 (what iroh 1.4 uses), `argon2` 0.6,
    `chacha20poly1305` 0.11, `blake3` 1.8, `bech32` 0.12, `spake2` 0.4,
    `zeroize` 1, `zxcvbn` 3, `keyring` 4, `postcard` 1, `thiserror` 2;
  - `proptest` 1 as a dev-dependency (approved with the spec's dependency
    table; it is test-only).

## Task list

### Phase 1: Foundation

- [x] **Task 1: Scaffold Mesh-Node** (Mesh-Node `1c74969`)
  - What: a cargo workspace with `crates/mesh-identity` (library) and
    `crates/mesh-node` (binary that prints its version), plus `.gitignore`, a
    short README (private, what it is, how to build) and `rustfmt.toml`
    matching Mesh-Voice. Every planned dependency is added, so version
    conflicts (RustCrypto releases against `ed25519-dalek` 3) surface now.
  - Accept: `cargo build --workspace` and `cargo test --workspace` succeed;
    clippy (`-D warnings`) and fmt are clean; pushed to `main`.
  - Verify: the commands from the spec's Commands section.
  - Files: `Cargo.toml`, `crates/mesh-identity/{Cargo.toml,src/lib.rs}`,
    `crates/mesh-node/{Cargo.toml,src/main.rs}`, `README.md`.
  - Size S. Depends on nothing.
- [x] **Task 2: IDs, keys and links** (`id.rs`; the `mesh://code/…` link moved to Task 9, where codes are defined)
  - What: `RootKey` and `DeviceKey` (generate, sign, zeroize), `MeshId`
    (bech32m `mesh1…`), and `mesh://id/…` and `mesh://code/…` links.
  - Accept:
    - text and link round-trip, including a property test;
    - each parse failure gives its own error kind (checksum, prefix, length,
      not a key, mixed case);
    - test vectors fix the ID text for a known key.
  - Verify: `cargo test -p mesh-identity id`.
  - Files: `src/id.rs`, `src/lib.rs`, `tests/vectors.rs`.
  - Size S. Depends on Task 1.
- [x] **Task 3: Device certificates and device list** (`device.rs`)
  - What: `DeviceCert` and `DeviceList` with hand-written, domain-separated
    signed layouts; verification; the higher-`seq`-wins rule; revocation.
  - Accept:
    - a certificate signed by another root is rejected;
    - a list with a lower or equal `seq` is rejected;
    - a revoked device fails the "is this device current" check;
    - vectors fix the signed bytes.
  - Verify: `cargo test -p mesh-identity device`.
  - Files: `src/device.rs`, `tests/vectors.rs`.
  - Size S. Depends on Task 2.

**Checkpoint A:** workspace green, IDs and devices covered by vectors. A
short note to you, no stop needed unless something surprised me.

### Phase 2: Keystore (security-critical; done early)

- [x] **Task 4: Password policy and username normalization** (`password.rs`)
  - What: the D11 check (at least 10 characters, `zxcvbn` score 3 or more,
    not containing the username) returning a verdict and zxcvbn's hint;
    `normalize(username)`.
  - Accept: unit tests for a dictionary word, a short password, one that
    contains the username (in any case) and a strong one; normalization trims
    and lowercases.
  - Verify: `cargo test -p mesh-identity password`.
  - Files: `src/password.rs`.
  - Size XS. Depends on Task 1.
- [x] **Task 5: Keystore create, unlock and lock** (`keystore.rs`)
  - What:
    - `MKS1` header with no ID or username;
    - Argon2id (64 MiB, t = 3, p = 1) over `normalize(username) || 0 || password`;
    - a random data key wrapped with XChaCha20-Poly1305, and the payload
      under the data key;
    - atomic write (temporary file, fsync, rename), 0600 on Unix;
    - one uniform error for a wrong username, a wrong password or a damaged
      file;
    - the D11 policy enforced on create.
  - Accept:
    - create → lock → unlock round-trips;
    - each of the three failures returns the same error;
    - a header change (version, salt, parameters) is detected;
    - the file on disk contains neither the username nor the ID (test greps
      the bytes);
    - an `--ignored` timing test prints the unlock time.
  - Verify: `cargo test -p mesh-identity keystore`;
    `cargo test -p mesh-identity --release -- --ignored unlock_time`
    (under 1 s on the Linux box).
  - Files: `src/keystore.rs`, `tests/vectors.rs`.
  - Size M. Depends on Tasks 2–4.
- [x] **Task 6: Change password, backup export and import**
  - What: `change_password` (re-wraps the data key only); `export` (fresh
    salt, protected by username and password); `import` (backup credentials,
    then a new password under D11, a new device key and certificate signed
    with the restored root).
  - Accept:
    - after a change, the old password fails and the new one works, and the
      data key is unchanged (the contents are re-encrypted, since both layers
      authenticate the header);
    - an import gives the same ID with a different device key;
    - D11 is enforced on the new password.
  - Verify: `cargo test -p mesh-identity keystore`.
  - Files: `src/keystore.rs`.
  - Size S. Depends on Task 5.
- [x] **Task 7: Keep unlocked (OS keyring)**
  - What: the D12 toggle: save the data key under service `mesh-node` and a
    random per-install entry name; open without asking while the entry
    exists; delete it on off; report "unavailable" with the reason when
    there's no keyring.
  - Accept:
    - on → a fresh process opens the keystore with no password; off → it
      asks again and the entry is gone;
    - no keyring → the unavailable state, not a crash.
  - Verify: an `--ignored` integration test on the Linux box (Secret
    Service), run by hand on Windows in Checkpoint D.
  - Files: `src/keystore.rs` (or `src/keyring.rs` if it grows).
  - Size S. Depends on Task 5.

**Checkpoint B (stop for review):** keystore done.
- I report the measured unlock time.
- I show `xxd identity.mks | head` to prove nothing readable is in the file.
- I report whether the Linux box has a Secret Service.
- You confirm before codes and linking.

**Checkpoint B result (approved 2026-10-23):**
- Unlock with 64 MiB / t = 3 / p = 1: 60 ms on the Linux box (release).
- The keystore file holds no username, ID, device name or device key
  (`nothing_identifying_is_readable_on_disk` searches the bytes).
- The Linux box has no working Secret Service: only KDE's
  `secretservicecompat` is registered and it fails to activate. "Keep
  unlocked" reports unavailable here, as designed. Installing a keyring daemon
  is a system change, so it needs your go-ahead.

### Phase 3: Safety number and friend codes

- [x] **Task 8: Safety number** (`safety.rs`, Mesh-Node `a3de90e`)
  - What: 30 digits per side from `BLAKE3-XOF("mesh/v1/safety" || id)`; the
    60-digit number ordered by the lower ID first; a QR payload string; a
    compare function.
  - Accept: `safety(a, b) == safety(b, a)` (property test); a vector for a
    fixed pair; output is 12 groups of 5 digits.
  - Verify: `cargo test -p mesh-identity safety`.
  - Files: `src/safety.rs`, `tests/vectors.rs`.
  - Size XS. Depends on Task 2.
- [x] **Task 9: Friend codes** (`code.rs`, Mesh-Node `f504146`; revoking is blind, see the spec's Revoke line)
  - What:
    - generate 40 bits; display `XXXX-XXXX`; lenient parsing (case,
      hyphens, I/L → 1, O → 0);
    - Argon2id to `lookup_id` and `record_key` (light, fixed settings,
      around 100 ms);
    - `CodeRecord` signed and encrypted; `CodeRevoke`; expiry options with
      7 days as the default and "never" only when chosen explicitly
      (`expires_at = 0`); use limits;
    - the owner's active codes kept in the keystore payload.
  - Accept:
    - parse and display round-trip (property test);
    - a record can't be read without the code;
    - a record signed by another root is rejected;
    - an expired record is refused, and a never-expiring one is accepted;
    - revoking removes the code from the active list;
    - vectors fix the derivation and the record layout.
  - Verify: `cargo test -p mesh-identity code`.
  - Files: `src/code.rs`, `src/keystore.rs` (active list), `tests/vectors.rs`.
  - Size M. Depends on Tasks 2 and 5.

### Phase 4: Linking (highest library risk)

- [x] **Task 10: Link state machine** (`link.rs`, Mesh-Node `34f7640`)
  - What: link-code generation (5-minute life, one use); `LinkInitiator` (E)
    and `LinkResponder` (N) as byte-message state machines; SPAKE2 plus key
    confirmation; then, over the derived key, device info, confirmation, and
    the transfer of the root key, username and device list, with N's
    certificate and `seq + 1`.
  - Accept, over an in-memory channel:
    - success gives N a working keystore (D11 password) with the same ID and
      a new device key, and E holds the updated list;
    - a wrong code fails after one attempt and burns the code;
    - a cancelled confirmation on E transfers nothing;
    - a replayed or out-of-order message is rejected;
    - an expired code is refused.
  - Verify: `cargo test -p mesh-identity link`.
  - Files: `src/link.rs`, `src/keystore.rs` (create from link).
  - Size M. Depends on Tasks 3, 5 and 6.

**Checkpoint C:** the whole library is done. `cargo test --workspace` is
green, clippy and fmt are clean, and the vectors are committed. A short
report to you.

**Checkpoint C result (2026-10-23):** the library is done. 46 tests pass
(2 more are `--ignored`: unlock timing and the real keyring); clippy and fmt
are clean; vectors for IDs, device certificates and lists, the safety number,
friend-code records and revokes, and the link key schedule, each computed with
independent Python libraries (`blake3`, `argon2-cffi`, `pynacl`) or OpenSSL.
- `spake2` is `=0.5.0-pre.0`, not 0.4 (your call: newest over legacy). It is
  RustCrypto's current line and shares curve25519-dalek 5 / sha2 0.11 with the
  rest, so no duplicate old majors remain. Move to 0.5.0 when it ships.
- Every other dependency is on its latest release (checked 2026-10-23).
- Friend-code derivation: Argon2id 64 MiB, t = 5, about 100 ms here.

### Phase 5: Helper and Mesh-Voice

- [ ] **Task 11: `mesh-node` helper mode**
  - What:
    - `mesh-node --data <dir>` speaks the JSON-lines protocol;
    - all identity commands and events from the spec, with `meshError` kinds;
    - stdin read on a plain thread, exiting when stdin closes (the mesh-dc
      pattern);
    - commands are never logged (passwords travel in them);
    - data directory defaults per platform.
  - Accept: a protocol test drives the binary through create → lock →
    unlock → code create/list/revoke → safety number → export/import, and
    asserts the events; stderr never contains a password used in the test.
  - Verify: `cargo test -p mesh-node`; by hand,
    `printf '{"fn":"idStatus","args":[]}\n' | mesh-node --data /tmp/mn`.
  - Files: `crates/mesh-node/src/{main.rs,helper.rs}`,
    `crates/mesh-node/tests/protocol.rs`.
  - Size M. Depends on Tasks 5–9.
- [ ] **Task 12: Mesh-Voice: Mesh network, onboarding and unlock**
  - What:
    - Mesh in the network registry;
    - `mesh-node` found next to Mesh's binary and run with the same
      restart/backoff as `mesh-wa`;
    - an Accounts card for Mesh;
    - onboarding (username, password with live strength feedback through
      `idCheckPassword`, the "can't be recovered" warning; two screens at
      most, success criterion 2);
    - the unlock screen (username and password; nothing shown before
      unlocking).
  - Accept: on the Linux box, a fresh data directory → create an identity in
    two screens → restart Mesh → asked to unlock → a wrong password shows the
    uniform error → the right one opens.
  - Verify: `cargo test -p mesh-app`; a manual run, with screenshots in the
    checkpoint report.
  - Files: `mesh-core/src/networks.rs`, `mesh-discord/src/mesh.rs` (new, next
    to `whatsapp.rs`), `mesh-app/src/main.rs`, `mesh-app/src/accounts.rs`,
    `mesh-app/src/mesh_ui.rs` (new).
  - Size M. Depends on Task 11.
- [ ] **Task 13: Mesh-Voice: Your ID, keep unlocked, password and backup**
  - What:
    - the Your ID page (`mesh1…` with a copy button, a QR code of the
      `mesh://` link, the list of devices);
    - the "keep unlocked on this device" toggle, disabled with its reason
      when unavailable;
    - change password;
    - export a backup (file dialog) and import one at onboarding.
  - Accept: each action works end to end on the Linux box; the toggle
    survives a restart.
  - Verify: a manual run with screenshots; `cargo test -p mesh-app`.
  - Files: `mesh-app/src/mesh_ui.rs`, `mesh-app/src/main.rs`,
    `mesh-discord/src/mesh.rs`.
  - Size M. Depends on Task 12.
- [ ] **Task 14: Mesh-Voice: Check an ID** (safety number)
  - What: paste an ID or `mesh://` link → the specific parse error or the
    60-digit safety number (and its QR). This is what the two-machine check
    uses. Friend-code screens come with `seed`, when codes can actually be
    shared.
  - Accept: a typo in a pasted ID names the error kind; a valid ID shows the
    same number in both directions.
  - Verify: a manual run.
  - Files: `mesh-app/src/mesh_ui.rs`.
  - Size S. Depends on Task 13.
- [ ] **Task 15: Ship `mesh-node` with Mesh-Voice**
  - What: `installer/install-linux.sh` and `installer/build.ps1` with
    `mesh.iss` build Mesh-Node (from a sibling checkout, or a path variable)
    and install `mesh-node` next to Mesh, like `mesh-wa`.
  - Accept: a fresh install on Linux finds `mesh-node`; the Windows installer
    bundles it (built on your PC).
  - Verify: run `install-linux.sh`; Windows in Checkpoint D.
  - Files: `installer/install-linux.sh`, `installer/build.ps1`,
    `installer/mesh.iss`.
  - Size S. Depends on Task 11.

**Checkpoint D: module gate (needs you on the Windows PC).** The spec's
two-machine check:

1. Create an identity on Windows.
2. Export, import on Linux: the same ID, and a different device key.
3. Both machines show the same safety number.
4. A wrong password fails on both, and unlocking takes under 1 s on both
   (times recorded here).
5. Keep unlocked works on Windows.

Then every success criterion is ticked in the spec and the module is marked
done in the capability map.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| ~~`spake2` 0.4 is old (2023)~~ Resolved: on `=0.5.0-pre.0`, no duplicate majors. Watch for 0.5.0 | Low | It's isolated in `link.rs` (Task 10, late). If it conflicts or looks unmaintained, swap in another PAKE (CPace) behind the same state machine. I'd ask you first, as it's a dependency change. |
| New RustCrypto majors (`argon2` 0.6, `chacha20poly1305` 0.11, `rand_core` 0.10) don't line up with `ed25519-dalek` 3 | Medium | Task 1 adds every dependency up front, so a conflict shows on day one; pin the versions that agree. |
| No Secret Service on the Linux box (Hyprland doesn't start one by itself) | Low | The D12 "unavailable" path is designed in; Task 7 checks the box and reports. Installing a keyring daemon would be a system package, so I'd ask first. |
| 64 MiB Argon2 is slower than 1 s on one machine | Low | Parameters live in the header, so lowering them is a deliberate, recorded decision (the spec says ask first). |
| `zxcvbn` adds about 1 MB | Low | Accepted in the spec. |
| Windows can't be built or tested from the Linux box | Medium | Checkpoint D happens on your PC. Everything before it is tested on Linux, and the code has no platform-specific parts except the keyring and file permissions. |
| A test-vector change after release silently breaks old keystores and IDs | High (later) | Vectors are committed from Task 2 onward. Changing one is a visible, reviewed breaking change. |

## Parallel work

These could run in parallel, but I'll do them in order unless you want
otherwise:
- after Task 2, Tasks 3, 4 and 8 are independent;
- Task 15 is independent of Tasks 12–14.

## Resolved questions (2026-10-22)

- **P1. Name.** The network is **Mesh**: the app's own native network, with
  Discord, WhatsApp and the other bridges as add-ons. It gets the tag `mesh`,
  and comes first among networks in the Accounts page. The helper binary is
  **Mesh-Node** (`mesh-node`).
- **P2. Checkpoints.** As planned: hard stops at B and D, short reports at A
  and C, and no going beyond the plan.
