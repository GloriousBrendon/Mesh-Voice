# Claude Code prompt: implement the Mesh "rice" redesign

Run from the Mesh-Voice repo root with this `design/` folder in place:

    claude "$(cat design/HANDOFF-PROMPT.md)"

---

You are implementing a UI redesign of **Mesh**, the Rust Matrix client in this
repo (`crates/mesh-core`, `crates/mesh-app`, iced 0.14, matrix-sdk 0.18).

Read these first, in order:
1. `design/DESIGN.md`: the spec (tokens, screens, mapping to code, bridge rules).
2. `design/screens/Main.dc.html`, `Rice.dc.html`, `AddFriend.dc.html`,
   `Accounts.dc.html`: HTML mockups with every pixel value inline. Treat them
   as the visual reference. Ignore the `<script>` blocks except `renderVals()`
   and `detect()`, which describe the data and the detection logic.
3. The root `HANDOFF-PROMPT.md` and `README.md`: the existing constraints still apply
   (pure Rust, one codebase for Linux and Windows, all Matrix logic in `mesh-core`, every colour
   from `theme::Palette`, no `unwrap()` on network or store paths).

Work in this order, and keep `cargo check --workspace` clean after each step.
Commit each step separately:

1. **Theme engine.** Extend `theme.rs`: a `colors.conf` parser (`$key = rgba(rrggbbaa)`,
   path `~/.config/themes/active/colors.conf`, overridable with `MESH_THEME_FILE`),
   the derived colours with the 4.5:1 contrast adjustment, an optional `hyprland.conf`
   layout parser, and a hot-reload subscription. Unit-test the parsers and the
   contrast function.
2. **Tiled shell.** Rebuild `view()` as top bar + space rail + channel pane +
   chat pane + member pane. Use the gaps, borders and rounding from the theme, and give the
   focused pane the accent border. Bundle IBM Plex Sans and JetBrains Mono.
3. **Spaces & channels.** Group rooms by `m.space` in mesh-core, with Home for DMs.
   Add `Alt+1..9` space switching and the `Ctrl+K` jump palette.
4. **Network tags.** Bridge detection in mesh-core (DESIGN.md §5), plus tags on
   spaces, authors, members and connection chips.
5. **Timeline polish.** Author grouping, code blocks, image figures,
   reactions, system events, the typing indicator, density modes (compact / cozy / irc).
6. **Settings → Rice page**, then **Add friend dialog**, then **Accounts page**
   (multi-account is the largest change; propose the `mesh-core` API before writing it).

Rules:
- Match the mockups' spacing and hierarchy, but use iced widgets that fit. Don't
  try to reproduce CSS one-for-one.
- If a design item needs something mesh-core can't do yet (Discord friend
  requests through the bridge, voice channels), build the UI disabled with an
  explanatory tooltip and note it in `README.md` → "Not there yet".
- Keep `README.md` accurate as features land.
