# Plan: device linking over the network

**Status: DRAFT 2026-11-11, plan only, for review.** Audit finding 16. It
wires identity's link state machines (`link.rs`, identity D8 and *Device
linking*) into the helper and Mesh-Voice, to be built before `dm`'s
two-machine check. Existing device **E**, new device **N**.

## Design (decisions to approve)

- **L1. Rendezvous through an ordinary address record on the seed.** E puts
  one record under `lookup = derive(link code, "mesh/v1/link-lookup")`,
  slot 0, expiring in 5 minutes (`LINK_CODE_LIFETIME`). Its ciphertext is
  E's link-node address sealed under `derive(link code, "mesh/v1/link-seal")`.
  The seed reads only the outer header (lookup, slot, expiry), as it does
  for every address record. So: **no new seed service and no seed change.**
  A guesser gets the address only by guessing a 40-bit code within
  5 minutes, past `addr-get`'s limits, and then still has only one SPAKE2
  guess, which burns the code.
- **L2. E runs a separate, short-lived link node**: a throwaway key that
  admits anyone, serves only the `link` service, takes one session, and shuts
  down at the first attempt, at the 5 minutes, or on cancel. The main node's
  allow list (contacts and own devices, routing D9) never opens.
- **L3. N uses its throwaway node** (seed D7b) to fetch the record and dial,
  so neither the seed nor E's link node learns anything before SPAKE2.
- **L4. Messages**: the four `link.rs` messages, one frame each on one
  stream, 30 s per step. A wrong code fails at once on N (`confirm`) and
  burns the code on E.
- **L5. After linking**, E signs and publishes the new device list, pushes it
  to its own devices and contacts as `own_lists` does today, and N starts the
  network and syncs the account (social D11), and later dm history (dm D8).
- **L6. Logs**: counts only. Never the code, address or device name.

## Tasks

1. **Link record** (`mesh-node/src/routing.rs`): derive the lookup and seal
   key from a link code, and seal and open a node address in the address
   record's outer format. *Tests:* round trip; the wrong code doesn't open
   it; the seed's `addr_outer` accepts it; it's within `MAX_AHEAD`.
2. **E side** (`mesh-node/src/link.rs`, new, a child module of the helper as
   `social.rs` is):
   - `linkStart()` answers `meshLink {code, expires}`;
   - on N's Device message: `meshLinkConfirm {name}`;
   - `linkConfirm(true|false)` and `linkCancel()`;
   - `meshLinkDone` or `meshError {kind: wrong-code|cancelled|timeout}`.
   *Tests:* unit tests for the state and timeouts.
3. **N side**, while no identity exists:
   - `linkJoin(code, deviceName)` answers `meshLinkReceived {username}`;
   - `linkFinish(username, password)` uses `Keystore::create_from_link`
     (D11 password rules, so the same `weak-password` errors as `idCreate`),
     then unlocks and starts the network as `idCreate` does.
4. **Process test** (`tests/link.rs`): a seed, E unlocked with one friend, N
   fresh.
   - A wrong code fails on N and burns the code on E.
   - The right code links: E's device list goes to `seq + 1` and lists N.
   - N pings E's friend by ID within a few seconds, because friends synced.
   - E removes N, and N can't reach E any more (finding 2).
   - No code, ID or device name appears in any stderr or the seed's files.
5. **Mesh-Voice**:
   - Your ID → Linked devices gets **Link a device**: shows the code with a
     5-minute countdown, then "Link *name*?" with Link and Cancel.
   - Onboarding gets **I already have Mesh**: the code and a device name,
     then the username and a new password for this device, with zxcvbn's
     hint, then done.
   - `mesh.rs` gets the commands and events. *Tests:* event parsing.
6. **By hand** (checkpoint, hard stop): link the Windows PC from the Linux
   box on the home network, check the friends list arrives, then remove it.
   Linking across networks waits for the VPS seed (finding 6), like every
   other two-machine check.

## Size, risks, what it doesn't do

- About 300 lines in Mesh-Node, 250 in Mesh-Voice. No new dependency:
  `spake2` 0.5.0-pre.0 is already there, and this makes it used.
- **Risk:** both devices must reach the same seed. Until finding 6, that
  means the home network. The by-hand check is planned for that.
- **Risk:** the new device gets the root secret (D2). That's by design, and
  the Remove device wording (finding 11) covers it.
- **Not in this plan:** linking by QR, linking with no seed (LAN discovery),
  and linking when no existing device is online (that's a backup restore).
