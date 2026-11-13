# Spec: `packaging`, slice 1: one download for a friend (Mesh)

Status: **APPROVED 2026-11-14**, with your answers (Linux no, the file sent
directly, unsigned, `mesh-wa` and `mesh-dc` stay; the first beta is 0.2.0)
and three edits: the build machine named (P1, *What it needs from you*), P2's
uncommitted changes meaning tracked files only, and Before beta item 7's
padded counts. Plan: [PLAN-packaging.md](PLAN-packaging.md).
This is the first slice of module 11 in [CAPABILITY-MAP.md](CAPABILITY-MAP.md)
(`packaging`). The rest of that module is listed under *Left for the full
packaging part*.

## Acceptance tests

Each test is done by hand on a Windows machine that has never had Mesh, Rust
or a Visual C++ runtime on it. Windows Sandbox works for this: it starts clean
each time. Tests P1 to P6 run at home, because the seed is home-only. P7 needs
a seed reachable from outside (see *Not this slice's to fix*).

- **P1. One command builds the download.** On your Windows PC (not the
  Linux box: the setup is built where Inno Setup and the MSVC toolchain
  are), with Mesh-Voice and Mesh-Node checked out side by side and both
  clean, `installer\build.ps1` prints the path of
  `dist\Mesh-Setup-<version>.exe` and its SHA-256.
- **P2. A missing or broken engine stops the build.** Each of these prints why
  and makes no setup file:
  - no Mesh-Node checkout;
  - Mesh-Node fails to build;
  - either checkout has uncommitted changes to tracked files. An untracked
    file (a scratch note, a log) doesn't stop the build.

  Today the build leaves the engine out with a warning and still makes a
  setup.
- **P3. The setup says what's inside.** Settings → Apps shows "Mesh
  <version>". The setup's file properties name the two commits it was built
  from (Mesh-Voice and Mesh-Node).
- **P4. It installs and runs on a clean machine:**
  - Copy the one file into the Sandbox.
  - Its hash matches the one P1 printed.
  - Run it. Windows' "unrecognised app" warning is expected while the setup
    is unsigned (D3): click "More info" and then "Run anyway".
  - There is no admin prompt.
  - Mesh starts on its own network: you're asked to create an identity, and
    no Matrix login appears.
  - No "VCRUNTIME140.dll was not found" (or similar) error appears for Mesh
    or any helper.
- **P5. It works with nothing set up by hand:**
  - Create an identity.
  - Your ID shows "online" within 10 s.
  - Make a friend code.
  - Add your own Mesh on your PC from that code.
  - Send a line each way.
  - If Windows asks about the firewall, try both answers on two runs. Mesh
    works either way: with Cancel, it goes through the relay.
- **P6. An update keeps everything:**
  - Build a second setup from a later commit.
  - Run it in the same Sandbox while Mesh is open.
  - Mesh and `mesh-node` are closed, replaced and started again.
  - The identity, the friend and the chat are all still there.
  - Uninstalling removes the program and keeps `%APPDATA%\mesh`.
- **P7. A friend, for real (waits on an outside seed):**
  - A friend on another network gets the one file from wherever it lives
    (D2).
  - They install it as in P4 and add you from a friend code.
  - You chat, and a message sent while they're away reaches them by mail.

Nothing here can be a machine test: every test is about a clean machine and a
real download. P2 is the exception, and it gets a line in the plan's own
check, not a test crate.

## Objective

Today a friend can't install Mesh. `installer\build.ps1` already builds a
per-user setup that puts `mesh-node.exe` next to `Mesh.exe`, where Mesh looks
for it (main.rs `mesh_node()`). It has three gaps:

- it quietly ships without the engine when the Mesh-Node checkout is missing;
- its binaries may need a Visual C++ runtime that a friend's PC doesn't have;
- nothing records which engine is inside, or gives the friend a hash to check.

This slice closes those gaps and decides where the file lives. That's all a
friend needs: the built-in relay and seed lists (Mesh-Node `helper::SEEDS`) mean
there's nothing to configure.

## Decisions (approved 2026-11-14)

- **D1. Windows only.** Linux waits for the full part, because a binary built
  on this box needs a C library as new as CachyOS's and won't start on older
  distros. Building portably needs a build container (local Docker is broken
  here, PLAN-seed Task 5) or a new tool such as `cargo-zigbuild`. A Linux friend
  for now builds from source with `installer/install-linux.sh`, which needs
  read access to the private Mesh-Node repo. *Question 1.*
- **D2. Where the download lives: (a), sent directly (your answer to question 2).** The three options were:
  - **(a) Send the file directly**, by a drive link or a chat attachment, with
    the hash beside it. Recommended for the first few friends: nothing new is
    public.
  - **(b) A GitHub Release on the public Mesh-Voice repo.** The engine binary
    contains the home relay's name and the seed's ticket. The name already
    resolves in public DNS, but a public release would put it next to the
    public repo, which the "never name the homelab in Mesh-Voice" rule exists to
    avoid.
  - **(c) A release on the private Mesh-Node repo.** Friends can't download
    from it without access, so it only works as storage for you.
- **D3. Unsigned for the beta (question 3).** P4 accepts the "unrecognised app"
  warning. A code-signing certificate costs money every year. As far as I know,
  it doesn't remove the warning at once either: Windows still wants a download
  history first. Azure Trusted Signing is the cheap option, but it's open only
  in some countries, so check yours. Signing slots into `build.ps1` later
  without changing anything else here.
- **D4. Static C runtime on Windows.** The release build links the Visual C++
  runtime into each binary, set in `build.ps1` and not in the repos' cargo
  config, so debug builds don't change. The other way, shipping Microsoft's
  runtime installer, needs an admin prompt, which the per-user setup avoids.
  - Risk: `mesh-wa` builds Opus through CMake, and the C parts must link the
    runtime the same way. The plan's first task checks this with a clean
    Sandbox run before anything else.
- **D5. The commits go into the setup, not the app.** `build.ps1` passes both
  short commit IDs to Inno Setup's version fields. That needs no app code, and
  the app keeps showing its Cargo version.
- **D6. The setup's contents don't change.** It still ships Mesh, `mesh-wa`,
  `mesh-dc` and `mesh-node`. A friend who never signs into WhatsApp or Discord
  never starts those helpers. Leaving them out is a later choice. *Question 4.*
- **D7. The version is yours to bump.** `mesh-app` is 0.1.0. A second beta setup
  must have a higher version, or the update test (P6) looks like a reinstall.
  The plan asks you for the number at each build; it doesn't pick one.

## Not this slice's to fix

- **A friend outside your home can't reach the seed yet.** Friend codes and
  mail go through the home seed, and it's home-only. P1 to P6 pass at home; P7
  waits for a seed reachable from outside: the VPS (finding 6) or a port
  forward. That's seed work, not packaging.
- Windows' firewall prompt and its "no admin rights" behaviour are already
  covered by transport's Checkpoint C. P5 only checks that the installed setup
  behaves the same.

## What it needs from you

1. Answers to questions 1 to 4 (given 2026-11-14).
2. **A clean Windows to test on:**
   - **Windows Sandbox** needs Windows Pro or higher, and turning it on once in
     "Turn Windows features on or off".
   - **Or** a second PC or VM that has never had Rust or Visual Studio on it.
3. **Your Windows PC as the build machine.** `build.ps1` runs there, not on
   the Linux box, with Rust (MSVC), Inno Setup 6 (`build.ps1` already
   needs it), and both checkouts side by side.
4. A version number for each beta setup (D7): 0.2.0 for the first, unless
   you say otherwise.
5. For P7 only: the VPS seed.

## Left for the full packaging part

- Release CI that builds and publishes the setup, instead of your PC.
- Code signing (D3), once the beta has shown it's worth paying for.
- A Linux download (D1): AppImage, Flatpak or a tarball, built portably.
- A published seed image, so a second seed needs only `docker compose up`.
- Updates from inside Mesh. For now a friend downloads the new setup.
- Changelogs, version rules, winget or MSIX, and ARM64.
- Reproducible builds, so a friend can check the binary against the source.

## Questions (answered 2026-11-14)

1. **Linux in this slice?** No (D1).
2. **Where does the download live?** (a): the file is sent directly, with no
   public release (D2).
3. **Ship unsigned for the beta?** Yes (D3).
4. **Keep `mesh-wa` and `mesh-dc` in a friend's setup?** Yes (D6).
