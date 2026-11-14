# Plan: `packaging`, slice 1: one download for a friend (Mesh)

Status: **APPROVED 2026-11-14**, with three changes: one sitting at your PC,
with Tasks 2 and 3 written first and Task 1's check at the start of the
sitting; a risk and fallback for Task 6's update; and Before beta item 7
folded into Task 5. Built from [SPEC-packaging.md](SPEC-packaging.md)
(approved 2026-11-14): Windows only, the file sent directly, unsigned, all
four binaries kept, and the first beta at 0.2.0.

## Who does what

Nothing in this plan can run on the Linux box. It has no PowerShell, no MSVC
toolchain and no Inno Setup, and I can't reach your Windows PC from here.
So each task is one of two kinds:

- **Alone:** I write and commit the change on the Linux box. I can read it
  over, but I can't run it.
- **At your PC:** you run the steps on your Windows PC (and in Windows
  Sandbox), then send me the output the step asks for. I record it in this
  plan.

Tasks 2 and 3 are written first, with the static runtime set the expected
way. Then there is **one sitting at your PC**, which runs Task 1's check
and then Tasks 4 to 6, top to bottom (*The sitting*, below). Task 7 comes
after it.

## What the C runtime check has to cover

The spec names `mesh-wa`'s Opus build. Each of the four binaries builds C
code, and every piece must link the runtime the same way:

| Binary | C code it builds |
|---|---|
| `Mesh.exe` (mesh-app) | Opus (`audiopus_sys`, through CMake), SQLite, aws-lc (CMake), ring |
| `mesh-wa.exe` | Opus (`opusic-sys`, through CMake), OpenH264, SQLite, ring |
| `mesh-dc.exe` | Opus (`libopus_sys`), ring |
| `mesh-node.exe` | ring |

## Task list

- [ ] **Task 1: Does a static C runtime build and run? (At your PC, at the
  start of the sitting. A hard stop if it fails.)**
  - **What:** `installer\build.ps1` (Task 2) builds all four binaries with
    `RUSTFLAGS=-C target-feature=+crt-static` and `--target
    x86_64-pc-windows-msvc`. The explicit target keeps the flag off build
    scripts and proc macros. Then `dumpbin /dependents` runs on the four
    binaries, and each one runs in a clean Sandbox as part of Task 5.
  - **Accept:**
    - the build makes a setup;
    - no `dumpbin` list names `VCRUNTIME140.dll`, `VCRUNTIME140_1.dll`,
      `MSVCP140.dll` or any `api-ms-win-crt-*.dll`;
    - Task 5 shows no missing-DLL dialog.
  - **If the build or `dumpbin` fails:** you stop there and send me the
    output. I bring you two fallbacks, and the choice is yours, because
    either changes D4:
    - **(a)** Fix the one crate's CMake runtime setting
      (`CMAKE_MSVC_RUNTIME_LIBRARY`, through the environment in
      `build.ps1`). No app code changes.
    - **(b)** Keep the dynamic runtime and put Microsoft's runtime DLLs
      next to `Mesh.exe` in `{app}`. Microsoft allows this "app-local" copy,
      and it needs no admin prompt.
  - Size M for you (four release builds from scratch).

- [x] **Task 2: `build.ps1` and `mesh.iss` for a friend's setup. (Alone.)**
  - **What:** all in `installer\`, no app code:
    - **Clean checkouts (P2):** first, for Mesh-Voice (which covers
      `mesh-wa\` and `mesh-dc\`) and for Mesh-Node, run
      `git status --porcelain --untracked-files=no`. Any output stops the
      build and names the changed files. Untracked files don't stop it.
    - **A missing engine (P2):** no Mesh-Node checkout now stops the build,
      before anything is built. In `mesh.iss`, `mesh-node.exe` loses
      `skipifsourcedoesntexist`, so the setup can't be made without it
      either. A failed build already stops it.
    - **The static runtime (D4):** `RUSTFLAGS` set in `build.ps1` for its
      own builds, then put back. Each build gets `--target
      x86_64-pc-windows-msvc`, and `mesh.iss` reads the binaries from
      `target\x86_64-pc-windows-msvc\release\` (passed as `/DTarget=`).
    - **The commits (P3):** `build.ps1` passes the two short commit IDs to
      ISCC (`/DAppCommit=… /DNodeCommit=…`). `mesh.iss` puts them in
      `VersionInfoProductTextVersion` and `VersionInfoComments`, as
      "0.2.0.0 (app abc1234, engine def5678)". `VersionInfoVersion` is set
      to the app's version. `AppVerName` stays "Mesh <version>".
    - **The hash (P1):** the last two lines print the setup's path, size
      and commits, then its SHA-256.
    - The header says the script runs on your Windows PC.
  - **Accept:** read over only, since nothing here can run it. Tasks 1 and
    4 are the real check.
  - **Files:** `installer/build.ps1`, `installer/mesh.iss`.
  - Size S.

- [x] **Task 3: Version 0.2.0. (Alone.)**
  - **What:** `crates/mesh-app/Cargo.toml` goes to `version = "0.2.0"`, plus
    the one `Cargo.lock` line it changes. Explorer and the setup read the
    exe's version from there, through `winresource`, and so does the app's
    About text.
  - **Accept:**
    - `cargo build -p mesh-app` and its tests pass on the Linux box;
    - there are no new clippy findings and no rustfmt diffs.
  - Size XS.

- [ ] **Task 4: P1 to P3, the build. (At your PC.)**
  - **What:** steps 2 to 5 of *The sitting*: the build, `dumpbin`, the
    setup's Details, and P2's four cases.
  - **Accept:**
    - P1: the path, size, commits and SHA-256 are printed;
    - P2: the four cases behave as the sitting says;
    - P3: Details shows 0.2.0 with both commits.
  - Size S.

- [ ] **Task 5: P4 and P5, a clean machine, and mail through the home
  seed. (At your PC and in Windows Sandbox.)**
  - **What:** steps 6 to 10 of *The sitting*:
    - your PC moves to the 0.2.0 setup, so it sends mail with the new
      engine;
    - one Sandbox run: install, identity, friend, a line each way, then
      Mesh closed in the Sandbox, a line sent from your PC, and Mesh
      reopened to see it arrive;
    - a second Sandbox run with the firewall prompt cancelled;
    - after the sitting, I read the seed's next hourly mail line against
      Before beta item 7's table.
  - This covers Before beta item 7 (*folded in 2026-11-14*).
  - **Accept:**
    - P4 and P5 as the spec says them: no admin prompt, no Matrix login,
      no missing-DLL dialog, "online" within 10 s, and lines both ways on
      both runs;
    - the line sent while the Sandbox was closed arrives when it reopens,
      and your PC's "sending" mark clears;
    - the seed's mail counts move as item 7's table says.
  - Size S.

- [ ] **Task 6: P6, an update. (At your PC and in Windows Sandbox.)**
  - **What:** steps 11 and 12 of *The sitting*. A throwaway 0.1.9 setup is
    built from a local branch that's deleted afterwards, so 0.1.9 is never
    sent to anyone. 0.2.0 is then installed over it while Mesh is open.
  - **Accept:**
    - P6 holds;
    - Settings → Apps shows 0.1.9, then 0.2.0;
    - `%APPDATA%\mesh` is still there after uninstalling.
  - **Risk:** Mesh restarts `mesh-node` by itself. During the update, the
    engine may come back after the setup closes it and lock its file, so the
    setup can't replace it.
    - **Fallback:** if P6 fails that way, the setup runs the `taskkill`
      line the uninstaller already has before the files are copied.
    - I don't add it unless P6 fails.
  - Size S.

- [ ] **Task 7: Results and the list. (Alone.)**
  - **What:**
    - record Tasks 1, 4, 5 and 6 here, and tick them;
    - record the seed's hourly line from Task 5;
    - in CAPABILITY-MAP, set `packaging` slice 1 as done, and tick Before
      beta item 7;
    - add P7 (a friend on another network, once an outside seed exists) to
      *Before beta* as item 8;
    - in the README's Windows section, say that the setup is the one file
      to send a friend, checked against its SHA-256.
  - Size XS.

## The sitting

Run these at your Windows PC, top to bottom. Each step says what to send
me. If a step marked **stop** fails, stop there and send me what it says;
the rest waits.

1. **Pull.** In both checkouts, `git pull`, then `git status`. Both must
   say clean (untracked files are fine). Mesh-Node must be at `d437a51` or
   later.
2. **Build (Task 1, stop).** Open "Developer PowerShell for VS", which
   `dumpbin` needs. In the Mesh-Voice folder, run `.\installer\build.ps1`.
   The first run builds everything from scratch, so expect a long wait.
   - **Send me:** the last two lines (path, size, commits; SHA-256) and
     roughly how long it took.
   - If it fails, send the first `error` lines and stop.
3. **`dumpbin` (Task 1, stop).** In the same window and folder, run:
   ```
   dumpbin /dependents target\x86_64-pc-windows-msvc\release\mesh-app.exe mesh-wa\target\x86_64-pc-windows-msvc\release\mesh-wa.exe mesh-dc\target\x86_64-pc-windows-msvc\release\mesh-dc.exe ..\Mesh-Node\target\x86_64-pc-windows-msvc\release\mesh-node.exe | Select-String 'Dump of|\.dll'
   ```
   - **Send me:** the whole output.
   - Stop if any line names `VCRUNTIME`, `MSVCP` or `api-ms-win-crt`.
4. **Details (P3).** Right-click `dist\Mesh-Setup-0.2.0.0.exe` →
   Properties → Details.
   - **Send me:** Product version, File version and Comments. Product
     version should read "0.2.0.0 (app …, engine …)".
5. **P2's four cases.** After each, **send me** its last lines.
   1. `New-Item scratch.txt` in Mesh-Voice, then `.\installer\build.ps1`.
      It builds; the cached build is quick. Then `Remove-Item scratch.txt`.
   2. Add a blank line to `README.md`, then run `.\installer\build.ps1`.
      It stops with "uncommitted changes" and names `README.md`. Then
      `git checkout README.md`.
   3. `$env:MESH_NODE='C:\nowhere'; .\installer\build.ps1`. It stops with
      "no Mesh-Node checkout".
   4. A broken engine on a clean tree:
      - `git clone ..\Mesh-Node $env:TEMP\node-broken`;
      - in that clone, `git switch -c broken`, then add the line
        `compile_error!("P2 check");` to the end of
        `crates\mesh-node\src\main.rs` and `git commit -am broken`;
      - back in Mesh-Voice, `$env:MESH_NODE="$env:TEMP\node-broken";
        .\installer\build.ps1`. It stops with "cargo build failed", and
        `dist\` gains no new file.
      - Then `Remove-Item Env:MESH_NODE`.
6. **Your PC to 0.2.0.** Run `dist\Mesh-Setup-0.2.0.0.exe` on your PC, over
   your current Mesh. Mesh closes, updates and starts again. Unlock it.
   - It needs the new engine to send mail in step 8.
   - Your accounts and chats stay.
   - **Send me:** whether Mesh came back with everything.
7. **Sandbox run 1 (P4, P5).** Open Windows Sandbox and copy the setup into
   it. Keep this Sandbox open until step 8 is done: closing it wipes it.
   1. In PowerShell there, run `Get-FileHash .\Mesh-Setup-0.2.0.0.exe`.
      It matches step 2's hash.
   2. Run the setup and click "More info", then "Run anyway". There's no
      admin prompt. Mesh opens asking you to create an identity, with no
      Matrix login and no missing-DLL dialog.
   3. Create the identity and time it to "online" on Your ID.
   4. If the firewall asks, click **Allow**.
   5. Make a friend code (Your ID → Friend codes) and add it from your PC.
   6. Send a line each way.
   - **Send me:** pass or fail for each of 1 to 6, and the time to
     "online".
8. **Mail (item 7).** Note the time.
   1. In the Sandbox, close Mesh.
   2. On your PC, send the Sandbox a line. Its mark stays at "sending".
   3. In the Sandbox, start Mesh again from the Start menu and unlock it.
   4. The line shows within a few seconds, and your PC's mark clears.
   - **Send me:** pass or fail, and the time you noted. I read the seed's
     hourly line for that hour myself and check it against item 7's table.
   - Then close the Sandbox.
9. **Sandbox run 2 (P5, the other answer).** Open a fresh Sandbox. Repeat
   step 7, but click **Cancel** at the firewall prompt. Skip the mail step.
   - **Send me:** pass or fail, and the time to "online".
10. **Tidy your friends.** The two Sandbox identities are now friends on
    your PC. Remove them under Friends if you like. Nothing depends on them.
11. **The throwaway 0.1.9 setup (Task 6).** In Mesh-Voice:
    1. `git switch -c throwaway-0.1.9`.
    2. Set `version = "0.1.9"` in `crates\mesh-app\Cargo.toml`.
    3. Run `cargo metadata -q --format-version 1 > $null`, which updates
       `Cargo.lock`.
    4. `git commit -am "throwaway 0.1.9"`.
    5. `.\installer\build.ps1`, which makes `dist\Mesh-Setup-0.1.9.0.exe`.
    6. `git switch main; git branch -D throwaway-0.1.9`.
    - **Send me:** the last two lines.
12. **The update (P6).** Open a fresh Sandbox and copy both setups in.
    1. Install 0.1.9. Create an identity, add it from your PC, and send a
       line each way. Settings → Apps shows Mesh 0.1.9.0.
    2. With Mesh open, run the 0.2.0 setup. Mesh closes, the files are
       replaced, and Mesh starts again.
    3. The identity, the friend and the chat are all there. Settings →
       Apps shows Mesh 0.2.0.0.
    4. Uninstall from Settings → Apps. Then `Test-Path $env:APPDATA\mesh`
       prints True.
    - **Send me:** pass or fail for each of 1 to 4.
    - If the setup reports a file in use, or can't replace `mesh-node.exe`,
      send its exact message. That's Task 6's risk, and its fallback is
      ready.

## Order and size

Tasks 2 and 3 first (alone, written now). Then the sitting: Task 1 at its
start, and Tasks 4 to 6 straight after unless Task 1 stops it. Then Task 7.

The changes come to about 60 lines in `installer\` and one version line,
with nothing in Mesh-Node.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| One C library won't link the static runtime | Medium | The sitting stops at step 2 or 3, before anything else; two fallbacks, your choice (Task 1). |
| The flag reaches proc macros or build scripts and breaks them | Low | `--target` keeps it off the host. |
| A static-runtime build takes a long time | Low | Only `build.ps1` uses it; debug builds and tests are unchanged. |
| During the update, Mesh restarts `mesh-node` and it locks its file | Medium | Task 6's fallback: the setup runs the uninstaller's `taskkill` line before copying. Added only if P6 fails that way. |
| Defender flags an unsigned new binary | Medium | Task 5 runs with Defender on in the Sandbox. If it flags one, I stop and we look at submitting it to Microsoft, or at signing (D3). |
| Your Windows edition has no Sandbox (Home) | Low | A VM, or a second PC that has never had Rust or Visual Studio. |
| I can't run anything in Task 2 | Medium | The sitting tests every branch of it, including all four P2 cases. |

## Needs a new dependency

None. `dumpbin` comes with the MSVC build tools the PC already has, and
Windows Sandbox is part of Windows.

## Open questions

None.
