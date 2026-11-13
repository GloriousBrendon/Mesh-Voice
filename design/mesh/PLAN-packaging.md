# Plan: `packaging`, slice 1: one download for a friend (Mesh)

Status: **DRAFT 2026-11-14, for your review.** No code until you approve it.
Built from [SPEC-packaging.md](SPEC-packaging.md) (approved 2026-11-14):
Windows only, the file sent directly, unsigned, all four binaries kept, and
the first beta at 0.2.0.

## Who does what

Nothing in this plan can run on the Linux box. It has no PowerShell, no MSVC
toolchain and no Inno Setup, and I can't reach your Windows PC from here.
So each task is one of two kinds:

- **Alone:** I write and commit the change on the Linux box. I can read it
  over, but I can't run it.
- **At your PC:** you run the steps on your Windows PC (and in Windows
  Sandbox), then send me the output the step asks for. I record it in this
  plan.

There are two sittings at your PC: Task 1 by itself, and then Tasks 4 to 6
together.

## What the C runtime check has to cover

The spec names `mesh-wa`'s Opus build. Each of the four binaries builds C
code, and every piece must link the runtime the same way:

| Binary | C code it builds |
|---|---|
| `Mesh.exe` (mesh-app) | Opus (`audiopus_sys`, through CMake), SQLite, aws-lc (CMake), ring |
| `mesh-wa.exe` | Opus (`opusic-sys`, through CMake), OpenH264, SQLite, ring |
| `mesh-dc.exe` | Opus (`libopus_sys`), ring |
| `mesh-node.exe` | ring |

So Task 1 checks all four binaries. `mesh-wa` goes first, as you asked.

## Task list

- [ ] **Task 1: Does a static C runtime build and run? (At your PC. A hard
  stop either way.)**
  - **What:** in PowerShell on your PC, set
    `$env:RUSTFLAGS='-C target-feature=+crt-static'`, then build in release
    with `--target x86_64-pc-windows-msvc`, one binary at a time:
    - `mesh-wa` first, in `mesh-wa\`;
    - then `mesh-dc` in `mesh-dc\`;
    - then `cargo build --release -p mesh-app` at the Mesh-Voice root;
    - then `cargo build --release -p mesh-node` in Mesh-Node.

    The explicit `--target` keeps the flag off build scripts and proc macros,
    which can fail with a static runtime. It also moves the binaries to
    `target\x86_64-pc-windows-msvc\release\`. Each build starts from
    scratch, so expect a long first run.
  - **Then**, in a "Developer PowerShell for VS", run
    `dumpbin /dependents <exe>` on each of the four.
  - **Then** copy the four into a fresh Windows Sandbox and run each once:
    - `Mesh.exe` opens its window;
    - `mesh-node.exe --version` prints its version;
    - `mesh-wa.exe` and `mesh-dc.exe` start without a "was not found" dialog.
      They wait on stdin, so close them.
  - **Accept:**
    - all four build;
    - no `dumpbin` list names `VCRUNTIME140.dll`, `VCRUNTIME140_1.dll`,
      `MSVCP140.dll` or any `api-ms-win-crt-*.dll`;
    - the Sandbox runs show no missing-DLL dialog.
  - **Send me:**
    - the four `dumpbin` lists;
    - the first `error` lines of any build that fails;
    - the build times.
  - **If it fails:** I stop and bring you the options. Two of them:
    - fix the one crate's CMake runtime setting (`CMAKE_MSVC_RUNTIME_LIBRARY`
      through the environment, no code);
    - or keep the dynamic runtime and put Microsoft's runtime DLLs next to
      `Mesh.exe` in `{app}`. Microsoft allows that "app-local" copy, and it
      needs no admin prompt.

    Either changes D4, so it's your call.
  - Size S for me, M for you (four release builds).

- [ ] **Task 2: `build.ps1` and `mesh.iss` for a friend's setup. (Alone.)**
  - **What:** all in `installer\`, no app code:
    - **Clean checkouts (P2):** at the start, for Mesh-Voice (which covers
      `mesh-wa\` and `mesh-dc\`) and for Mesh-Node, run
      `git status --porcelain --untracked-files=no`. Any output stops the
      build and names the changed files. Untracked files don't stop it.
    - **A missing engine (P2):** no Mesh-Node checkout now stops the build
      instead of warning. In `mesh.iss`, `mesh-node.exe` loses
      `skipifsourcedoesntexist`, so the setup can't be made without it either.
      A failed build already stops it (the `throw` that's already there).
    - **The static runtime:** set as Task 1 found it worked. The flag goes in
      `build.ps1`'s own environment, not in any cargo config, and
      `mesh.iss`'s paths follow the target folder.
    - **The commits (P3):** `build.ps1` passes the two short commit IDs to
      ISCC (`/DAppCommit=… /DNodeCommit=…`). `mesh.iss` puts them in the
      setup's version fields:
      `VersionInfoProductTextVersion=0.2.0 (app abc1234, engine def5678)`,
      and the same in `VersionInfoComments`. `AppVerName` stays "Mesh
      <version>".
    - **The hash (P1):** the last line prints the setup's path, size and
      `Get-FileHash -Algorithm SHA256`.
    - The header comment says the script runs on your Windows PC.
  - **Accept:** read over only, since nothing here can run it. Task 4 is
    the real check.
  - **Files:** `installer/build.ps1`, `installer/mesh.iss`.
  - Size S.

- [ ] **Task 3: Version 0.2.0. (Alone.)**
  - **What:** `crates/mesh-app/Cargo.toml` goes to `version = "0.2.0"`, plus
    the one `Cargo.lock` line it changes. Explorer and the setup read the
    exe's version from there, through `winresource`.
  - **Accept:**
    - `cargo build -p mesh-app` passes on the Linux box;
    - there are no new clippy findings and no rustfmt diffs.
  - Size XS.

- [ ] **Task 4: P1 to P3, the build. (At your PC.)**
  - **What:** pull both repos and run `installer\build.ps1`. Then try P2's
    four cases:
    1. Make an untracked scratch file. The build still runs.
    2. Edit a tracked file. The build stops and names the file. Undo the
       edit.
    3. Run with `$env:MESH_NODE='C:\nowhere'`. The build stops with "no
       Mesh-Node checkout".
    4. Make a scratch clone of Mesh-Node, commit a deliberate compile error
       on a branch there (so the tree is clean but the build fails), and point
       `MESH_NODE` at it. The build stops, and no new setup appears in
       `dist\`.

    Then right-click the setup → Properties → Details.
  - **Accept:**
    - P1: the path, size and SHA-256 are printed;
    - P2: the four cases behave as above;
    - P3: Details shows 0.2.0 with both commits.
  - **Send me:** the last lines of each run, and the Details text.
  - Size S.

- [ ] **Task 5: P4 and P5, a clean machine. (At your PC, in Windows
  Sandbox.)**
  - **What:** copy only the setup into a fresh Sandbox. Then:
    - check `Get-FileHash` against Task 4's hash;
    - install, clicking "More info" and then "Run anyway";
    - create an identity and wait for "online";
    - make a friend code and add your own Mesh on your PC from it;
    - send a line each way.

    Run it twice, once answering the firewall prompt with Allow and once with
    Cancel. The Sandbox is clean each time it starts.
  - **Accept:** P4 and P5 as the spec says them: no admin prompt, no Matrix
    login, no missing-DLL dialog, "online" within 10 s, and the lines both
    ways on both runs.
  - **Send me:** pass or fail for each part, and the time to "online".
  - Size S.

- [ ] **Task 6: P6, an update. (At your PC, in Windows Sandbox.)**
  - **What:** first build an older setup:
    - make a throwaway local branch;
    - set `mesh-app` to 0.1.9 and commit it, so the tree is clean;
    - build;
    - delete the branch afterwards. 0.1.9 is never sent to anyone.

    Then, in one Sandbox:
    - install 0.1.9 and set up as in Task 5;
    - with Mesh open, run the 0.2.0 setup;
    - check Mesh comes back with the identity, the friend and the chat;
    - uninstall, and check that `%APPDATA%\mesh` is still there.
  - **Accept:**
    - P6 holds;
    - Settings → Apps showed 0.1.9, then 0.2.0.
  - Size S.

- [ ] **Task 7: Results and the list. (Alone.)**
  - **What:**
    - record Tasks 1, 4, 5 and 6 here, and tick them;
    - in CAPABILITY-MAP, set `packaging` slice 1 as done;
    - add P7 (a friend on another network, once an outside seed exists) to
      *Before beta* as item 8;
    - in the README's Windows section, say that the setup is the one file to
      send a friend, and that it's checked against its SHA-256.
  - Size XS.

## Order and size

Task 1 comes first, and it's a hard stop: Tasks 2 and 3 wait for its result,
because Task 2 sets the runtime the way Task 1 found it working. Then Tasks 2
and 3 (alone), Task 4, Task 5 and Task 6 (one sitting at your PC), and Task 7.

The changes come to about 40 lines in `installer\` and one version line, with
nothing in Mesh-Node.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| One C library won't link the static runtime | Medium | Task 1 finds it before anything is written; two fallbacks, your choice. |
| The flag reaches proc macros or build scripts and breaks them | Low | `--target` keeps it off the host; Task 1 builds that way. |
| A static-runtime build takes a long time | Low | Only release builds use it; debug builds and tests are unchanged. |
| Defender flags an unsigned new binary | Medium | Task 5 runs with Defender on in the Sandbox. If it flags one, I stop and we look at submitting it to Microsoft, or at signing (D3). |
| Your Windows edition has no Sandbox (Home) | Low | A VM, or a second PC that has never had Rust or Visual Studio. |
| I can't run anything in Task 2 | Medium | Task 4 tests every branch of it, including all four P2 cases. |

## Needs a new dependency

None. `dumpbin` comes with the MSVC build tools the PC already has, and
Windows Sandbox is part of Windows.

## Open questions

None.
