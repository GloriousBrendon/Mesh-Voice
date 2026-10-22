# Spec: `identity` (Mesh)

Status: **DRAFT, awaiting approval.** Module 1 of the build order in
[CAPABILITY-MAP.md](CAPABILITY-MAP.md). Carries over
`GloriousBrendon/Mesh` `docs/IDENTITY_AND_KEY_MANAGEMENT.md` §1–4 where it
still holds (noted inline).

## Objective

Give every Mesh user one permanent identity that they own, with no account,
server or registration, and that is safe on disk and can be shared by hand.

What this module delivers:

1. **Keypair and permanent ID.** An Ed25519 root key per person. The public
   key is the ID, written `mesh1…`, and travels as text, a `mesh://` link or a
   QR code.
2. **Devices.** Each install (device) has its own key, certified by the root
   key. A person has one ID and one or more devices.
3. **Friend codes.** Short codes like `K7QM-3XPA` that stand for your ID,
   with an expiry, a use limit and revocation, like Discord invites. This
   module defines the code and its signed record. Publishing and looking them
   up belongs to `seed` and `routing`.
4. **Password-encrypted keystore.** Everything secret is encrypted at rest
   with a key derived from the password set at onboarding. The password never
   leaves the device. Includes an encrypted backup file.
5. **Safety number.** A number two people compare to confirm they hold each
   other's real IDs.
6. **Linking another device.** While one existing device is online, a new
   device joins the same ID. This module defines the protocol and its
   cryptography. The real network path arrives with `transport` (see
   Boundaries).

**User stories**

- First launch: I pick a username and a password, and I have an ID. No email,
  no phone number, no sign-up.
- I copy my ID, show its QR code, or make a code that lasts 24 hours and
  works once, and give that to a friend.
- After we connect, we read out our safety number to check nobody is in the
  middle.
- I add my laptop to my identity by typing a short code from my PC while the
  PC is on.
- If I lose a device, I remove it from my other device.
- A stolen disk image is useless without my password.

### Decisions this spec makes (review these)

| # | Decision | Why |
|---|---|---|
| D1 | **Root key plus device keys.** The ID is the root public key. Each device has its own Ed25519 device key, certified by a root-signed device certificate. | iroh dials one key per running endpoint, so two of your devices online at the same time need different keys. Per-device keys also make removing a lost device possible. This settles the old doc's open question (§7). |
| D2 | **The root secret is copied to every linked device** (encrypted in each keystore). | "Linking needs one existing device online" means any device must be able to link. If only the first device held the root key, losing it would freeze your devices forever. Cost: anyone who steals a device and its password holds your identity. In v1 the remedy is ID rotation, which is out of scope. See open question Q1. |
| D3 | ID text is **bech32m** with prefix `mesh`: `mesh1` + 52 characters + a 6-character checksum (about 63 in all). | The checksum catches typos in copy-pasted IDs. It's the format the map already names. |
| D4 | Friend codes are **8 Crockford base32 characters (40 bits)**, shown as `XXXX-XXXX`. Input ignores case and hyphens, and reads `I`/`L` as `1` and `O` as `0`. | Easy to read aloud, with no ambiguous characters. 40 bits plus a slow key derivation and rate limits make guessing too expensive (see Q3). |
| D5 | A code's record is **encrypted and found under keys derived from the code**, so whoever stores it (a seed, or the DHT later) can't tell which ID it belongs to without breaking the code. | Seeds hold ciphertext only (map: seed hardening). |
| D6 | Keystore: **Argon2id → key-encryption key → wraps a random data key; XChaCha20-Poly1305**. | Changing the password re-wraps one key instead of re-encrypting everything. Later modules (`e2ee`, `dm`) can encrypt their local stores with the same data key. |
| D7 | Safety number: **60 digits (12 groups of 5)**, built from both IDs in sorted order, so both people see the same number. Also offered as a QR code. | The Signal pattern, which people know. Based on root IDs, so it doesn't change when either person adds a device. Refines the old doc's §3.4 BLAKE3 formula with domain separation. |
| D8 | Device linking uses a **short typed code with SPAKE2** (a password-authenticated key exchange), not a long QR payload. | Desktops rarely have a camera aimed at another screen. SPAKE2 makes a short code safe: an attacker gets one guess per linking attempt. |
| D9 | **One identity per install** in v1. | Mesh-Voice shows one account per network today. Multiple identities can come later. |

## Behaviour

### Keys and IDs

- **Root key**: Ed25519, generated with the OS random number generator. Its
  public key, encoded as D3, is the ID. It signs only device certificates,
  device list updates, friend-code records and revocations. It never signs
  chat traffic.
- **Device key**: Ed25519, one per install. It is the device's transport
  identity: `transport` uses it as the iroh `SecretKey`, so the iroh node ID
  is the device public key. Its certificate:
  `DeviceCert { version, root_id, device_pubkey, device_name, created_at, sig_by_root }`.
- **Device list**: `DeviceList { version, root_id, seq: u64, devices: [DeviceCert], revoked: [device_pubkey], sig_by_root }`.
  It is last-writer-wins by `seq`. A list with a lower `seq` than one already
  seen is rejected. `social` and `routing` hand it to friends. This module
  only builds and verifies it.
- **Signed bytes are a fixed, hand-written layout** with a domain-separation
  string per type, for example `b"mesh/v1/device-cert"`. Serde is never used
  for signed bytes, so encoding never makes a signature ambiguous.
- **Display name**: the username from onboarding. It is self-chosen, not
  unique and not global. Friends see it as a hint, and can keep their own
  nickname for you (`social`).

### ID text, links, QR

- ID: `mesh1qyfz…` (D3). Parsing rejects a bad checksum, the wrong prefix,
  the wrong length and mixed case, each with its own error, so the UI can say
  *what* is wrong.
- Links: `mesh://id/<mesh1…>` and `mesh://code/<XXXX-XXXX>`. The QR code is
  the link. Mesh-Voice draws QR codes already (WhatsApp and Discord sign-in),
  so this module only produces the string.

### Friend codes (D4, D5)

- **Create**: `code = 40 random bits`. Options:
  - expiry: 30 minutes, 1 hour, 6 hours, 12 hours, **24 hours (default)** or
    7 days (see Q4);
  - uses: **1 (default)**, 5, 10, 25 or unlimited.
- **Derive**: `Argon2id(code, salt = "mesh/v1/code")` → 48 bytes:
  - `lookup_id` (16 bytes): the key the record is stored and found under;
  - `record_key` (32 bytes): encrypts the record.
  The Argon2 settings are lighter than the keystore's (target about 100 ms)
  but fixed, so every client derives the same handle.
- **Record**:
  `CodeRecord { version, root_id, expires_at, max_uses, created_at, sig_by_root }`,
  encrypted with `record_key`. Someone who resolves it checks the signature
  and `root_id`, so only the owner can make a record for their ID.
- **Revoke**: `CodeRevoke { lookup_id, sig_by_root }`. The holder deletes the
  record.
- **Enforcement** of expiry, uses and lookup rate limits happens where the
  record is held (`seed`). The client also refuses an expired record. Your
  own active codes are listed in the keystore, so you can see and revoke
  them.
- A code is only a pointer. Trust comes from the safety number (D7).

### Keystore (D6)

- File: `<data>/identity.mks`. The data directory follows the helpers'
  pattern: `--data`, defaulting to `~/.local/share/mesh/node` or
  `%APPDATA%\mesh\node`.
- Header (plaintext, authenticated as associated data): magic `MKS1`, format
  version, Argon2id parameters, salt, the wrapped data key, and `root_id` (so
  the unlock screen can show whose identity this is before the password is
  entered).
- Argon2id settings: **m = 64 MiB, t = 3, p = 1**, stored in the header so
  they can be raised later without breaking old files. The goal is under 1 s
  to unlock on the Linux box; see success criteria.
- Payload (XChaCha20-Poly1305 under the data key): root secret, device
  secret, device list, username, active friend codes. Later modules get the
  data key through an API, not the file.
- Writes are atomic (temporary file, fsync, rename) with permissions 0600 on
  Unix. Secrets are wiped from memory (`zeroize`) when dropped and never
  logged.
- **Password**: at least 8 characters; the UI warns about weak ones (see
  Q2). The password can be changed, which re-wraps the data key. **A
  forgotten password can't be recovered.** The ways back are a backup file or
  relinking from another device. The UI says so at onboarding.
- **Backup**: `export(path, password)` writes the same format with a fresh
  salt, holding the root key and username. A restored device makes a new
  device key and certificate, signed with the restored root key. The old
  doc's §4.1 carries over. Shamir secret sharing (§4.2) and hardware keys are
  not in v1.

### Safety number (D7)

- Each side gets 30 digits: `BLAKE3-XOF("mesh/v1/safety" || id)` → six
  5-digit groups.
- The number is the two halves ordered by the lower ID first, so both people
  see the same 60 digits.
- `safety_qr(a, b)` is for scanning to compare instead of reading aloud.
- Showing and marking a contact "verified" is `social`'s job. This module
  computes and compares the number.

### Device linking (D2, D8)

Existing device **E**, new device **N**:

1. On E, under Linked devices, "Link a device" creates a one-time **link
   code**: 8 characters, the same alphabet as friend codes, valid for 5
   minutes.
2. N's onboarding offers "I already have Mesh". You type the code there, and
   N reaches E. The rendezvous comes from `transport` and `seed`.
3. Both sides run **SPAKE2** with the code, then confirm the key. A wrong
   code fails after one attempt and burns the code.
4. Over the encrypted channel:
   - N sends its new device public key and device name;
   - E shows "Link *Laptop*?", and you confirm;
   - E sends the root secret, the username and the device list, plus a new
     `DeviceCert` for N;
   - E publishes the device list with `seq + 1`.
5. N sets its own password. Each device's keystore has its own password.
6. **Remove a device**: any device signs a new `DeviceList` with that device
   in `revoked`. Honest peers stop talking to it. Because of D2, a removed
   device that someone else controls still holds the root key; see Q1.

This module ships steps 3–5 as a transport-agnostic state machine
(`LinkInitiator` / `LinkResponder` over byte messages) with full tests over an
in-memory channel. The UI and the real network path come with `transport`.

## Tech stack

All in **Mesh-Node** (`GloriousBrendon/Mesh-Node`, private), Rust 2024
edition, stable toolchain (1.99 or later, the same as Mesh-Voice).
Dependencies, each justified, all RustCrypto or widely audited:

| Crate | For |
|---|---|
| `ed25519-dalek` 2 | Keys and signatures. iroh uses the same crate, so keys pass straight through. |
| `argon2` | Keystore and friend-code key derivation. |
| `chacha20poly1305` | XChaCha20-Poly1305 for the keystore and code records. |
| `blake3` | Safety number and other derivations. |
| `bech32` | The `mesh1…` ID with its checksum. |
| `spake2` | Device-link key exchange (D8). |
| `zeroize` | Wiping secrets from memory. |
| `rand_core` / `getrandom` | OS randomness. |
| `serde` + `postcard` | Encoding the encrypted keystore payload only (not signed bytes). |
| `thiserror` | Typed errors the UI can show. |

No network crates in this module.

## Commands

Mesh-Node (repository root):

```sh
cargo build --release                         # target/release/mesh-node
cargo test -p mesh-identity                   # unit and property tests
cargo test --workspace                        # everything
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
./target/release/mesh-node --data /tmp/mn-a   # helper mode: JSON lines on stdio
```

Mesh-Voice:

```sh
cargo build --release -p mesh-app && cargo test -p mesh-app
```

Helper protocol: the same as `mesh-wa` / `mesh-dc`. Commands arrive on stdin
as `{"fn":..,"args":[..]}`; events go to stdout as
`{"binding":..,"payload":"<json>"}`; logs go to stderr. Identity commands:
`idStatus`, `idCreate(username, password)`, `idUnlock(password)`, `idLock`,
`idChangePassword(old, new)`, `idExport(path, password)`,
`idImport(path, backupPassword, newPassword)`, `codeCreate(ttl, uses)`,
`codeRevoke(code)`, `codeList`, `safetyNumber(otherId)`, `parseId(text)`.
Events: `meshIdentity` (locked or unlocked, ID, username, devices),
`meshCodes`, `meshError`.

## Project structure

```
Mesh-Node/
  Cargo.toml                 workspace
  crates/mesh-identity/      this module: a library, no I/O beyond the keystore file
    src/lib.rs               public API
    src/id.rs                keys, mesh1 text, links
    src/device.rs            DeviceCert, DeviceList, signing layouts
    src/keystore.rs          file format, unlock, change password, export/import
    src/code.rs              friend codes and records
    src/safety.rs            safety number
    src/link.rs              SPAKE2 link state machine
    tests/vectors.rs         fixed test vectors (format stability)
  crates/mesh-node/          the binary: helper mode (stdio) now, headless seed later
    src/main.rs
    src/helper.rs            JSON-lines protocol, identity commands
Mesh-Voice/
  crates/mesh-app/src/…      onboarding, unlock, Your ID, backup and change-password screens
  (helper glue next to the existing mesh-wa / mesh-dc glue; the plan settles the file)
  design/mesh/SPEC-identity.md
```

## Code style

The style of Mesh-Voice's helpers: small modules, typed errors, a doc comment
per public item, comments for *why*, no `unwrap` outside tests.

```rust
/// A person's permanent ID: their root Ed25519 public key, shown as `mesh1…`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshId(ed25519_dalek::VerifyingKey);

impl MeshId {
    /// Parses `mesh1…` text; each failure says what's wrong, for the UI.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        let (hrp, data) = bech32::decode(text.trim()).map_err(|_| IdError::Checksum)?;
        if hrp.as_str() != "mesh" {
            return Err(IdError::NotMesh);
        }
        let bytes: [u8; 32] = data.try_into().map_err(|_| IdError::Length)?;
        // Rejects points that aren't valid keys, so a typo can't parse as some other key.
        Ok(Self(VerifyingKey::from_bytes(&bytes).map_err(|_| IdError::NotAKey)?))
    }
}
```

## Testing strategy

- **Unit tests** in each file, for the API and every error path:
  - wrong password;
  - a corrupted header, wrong magic or newer version;
  - every way a `mesh1` string can be invalid;
  - expired and revoked code records;
  - a device list with a lower `seq`;
  - a link with the wrong code.
- **Property tests**: ID text and code text round-trip; a `CodeRecord`
  encrypt-then-decrypt round-trips; `safety(a, b) == safety(b, a)`.
  `proptest` is a dev-dependency only (ask first, per Boundaries).
- **Test vectors** (`tests/vectors.rs`): fixed keys → expected ID text,
  signed-byte layouts, safety number and code derivation. Once v1 ships, any
  change to these is a breaking format change and fails loudly.
- **Keystore timing test**: unlock time with the default parameters, run with
  `--ignored` on the real machines, and the measured times recorded in the
  plan.
- **Linking**: the full E↔N exchange over an in-memory channel, covering
  success, wrong code, a cancelled confirmation and a replayed message.
- **Helper**: drive `mesh-node` over stdio in a test, the way the
  `mesh-dc` / `mesh-wa` protocol tests do.
- **Two-machine check** (the module gate):
  1. On the Windows PC, create an identity in Mesh-Voice; note the ID.
  2. Export a backup, copy it to the Linux box, and import it there. Both
     machines show the same ID, and the Linux box gets its own device key.
  3. On each machine, enter the other's ID. Both show the same safety number.
  4. A wrong password fails on both; the correct one unlocks in under 1 s.

  Linking over the network is checked at the end of `transport`, on the same
  two machines.

## Boundaries

- **Always:**
  - Domain-separate every signature and derivation.
  - Constant-time comparisons for secrets.
  - Wipe secrets from memory, and never log them (passwords travel over the
    helper's stdin, so the helper never echoes its commands).
  - Atomic keystore writes.
  - A test vector for every on-disk or on-wire format.
- **Ask first:**
  - Any dependency not in the table above, including `proptest`.
  - Changing D1–D9.
  - Lowering the Argon2 parameters.
  - Anything that touches iroh (that's `transport`).
- **Never:**
  - Send the password, the data key or any secret off the device, except the
    root key to a device you confirmed during linking, inside the SPAKE2
    channel.
  - Telemetry.
  - A central registry of IDs or codes.
  - A recovery mechanism that needs a server.

## Success criteria

1. `mesh-identity` builds and tests pass on Windows and Linux, with clippy at
   `-D warnings` and `fmt --check` clean.
2. A new identity on first launch takes at most 2 screens in Mesh-Voice
   (username + password → done), with no network needed.
3. Unlocking with the default parameters takes under 1 s on the Linux box and
   the Windows PC (measured and recorded). A wrong password is rejected, and
   rejects the same way whether the password or the file is wrong.
4. Restarting Mesh-Voice keeps the identity; the unlock screen shows the ID's
   username before the password is entered.
5. ID text: typos in copied IDs are caught (checksum), and parse errors tell
   you which kind of mistake it is.
6. Friend codes: creation, listing, revocation and record verification work,
   and records can't be read without the code.
7. The safety number is identical on both machines for the same pair, in both
   directions.
8. Backup export → import on the other machine gives the same ID, with a new
   device key.
9. Linking: the in-memory E↔N test passes. A wrong code fails without
   revealing anything about the code beyond "wrong".
10. Test vectors are committed for every format.
11. The two-machine check above passes.

## Open questions

- **Q1. Root key on every device (D2), or only on some?** I recommend every
  device for v1, because it's the only way "any online device can link" holds
  without one indispensable device. The cost is that a stolen device plus its
  password owns the identity until ID rotation exists. The alternative is to
  keep the root key only on the first device, so others can't link or remove
  devices. Which do you want?
- **Q2. Password policy.** Is a minimum of 8 characters with a weak-password
  warning enough, or should weak passwords be refused outright? The keystore
  only protects against someone who has your disk, so this is about offline
  guessing.
- **Q3. Unlock every launch, or "remember on this device"?** Discord never
  asks. The options:
  - (a) ask every time Mesh starts;
  - (b) offer "keep unlocked on this device", which keeps the data key in the
    OS keyring (Windows Credential Manager / Linux Secret Service), so it's
    only as safe as your login;
  - (c) (b) on by default.

  I lean to (b), off by default.
- **Q4. Friend-code limits.** Discord allows invites that never expire. I
  propose 7 days at most for v1, because a code that lives forever is a
  standing pointer anyone who sees it can use. Allow "never"?
- **Q5. Where codes are held in v1.** Seeds can enforce single-use, revocation
  and rate limits. The BitTorrent DHT (pkarr) can't enforce single-use or rate
  limits: anyone can read a record as often as they like until it expires. I
  propose seeds only for v1, using the same record format so the DHT can be
  added later. OK?
- **Q6. Capability map inconsistency (not identity).** The table says
  `social` depends on `mailbox`, but the build order puts `social` before
  `mailbox`. I'd read it as "social works between online peers first, and
  gains offline delivery when `mailbox` lands", and change the table to
  `social: routing (+ mailbox later)`. OK to fix the map that way?
