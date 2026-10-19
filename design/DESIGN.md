# Mesh — design spec (v0.2 "rice")

Source of truth for the redesign. The four screens in `screens/*.dc.html` are
HTML mockups with every value written inline. Open them in a browser to read them
(they need the canvas runtime to render fully, so read them as markup if they
don't display). This file turns them into what the iced code needs.

## 1. Principles

1. **Wear the user's desktop.** Every colour, gap, border width and corner
   radius comes from the rice, either from `colors.conf` or `hyprland.conf`. Nothing is
   hardcoded in widget code (this is already constraint 3 in HANDOFF-PROMPT.md).
2. **Tiling, not cards.** The window is a set of panes separated by `gaps_out`
   on a `$bg2` background, each pane with a `border_size` border and `rounding`
   corners. The focused pane's border is `$accent`, like Hyprland's
   `col.active_border`. Other panes use `border`.
3. **Always show the network.** Every person, message author, space and link
   carries a small mono tag: `mx` (Matrix) or `dc` (Discord, through the bridge). The tag is
   text, not only colour.
4. **Keyboard first.** `Super`-style numbered spaces (`Alt+1..9`), `Ctrl+K`
   jump palette, `Esc` closes settings and dialogs, `/` commands in the composer.
5. **mesh-core stays Matrix-only.** Discord support comes from a
   `mautrix-discord` bridge on the user's homeserver. The client detects bridged
   rooms and users. It never talks to Discord directly.

## 2. Tokens

### Colours: read from `~/.config/themes/active/colors.conf`

Format (the same file CNTRL parses, see `cntrl-core/src/theme.rs`):
`$key = rgba(rrggbbaa)` per line. `#` lines are comments. `$` is optional.

| rice key   | default   | palette field  | used for |
|------------|-----------|----------------|----------|
| `$bg2`     | `#080c10` | `background`   | gaps / window background, inputs, code blocks |
| `$bg1`     | `#0f1822` | `surface`      | pane fill |
| `$surface` | `#1f252e` | `surface_alt`  | selected rows, chips, active space tile |
| `$accent`  | `#d88f92` | `accent`       | focused pane border, active indicators, primary buttons, own avatar |
| `$text`    | `#a8b0be` | `text`         | body text |

Derived (computed, never read from the file):

| field          | default   | rule |
|----------------|-----------|------|
| `text_bright`  | `#e3e7ee` | `text` lightened about 35%. Names, headings, active labels |
| `text_muted`   | `#8892a0` | blend of `text` and `surface`, then **lightened until contrast is ≥ 4.5:1 against `surface`** (it's 4.8:1 at the defaults) |
| `accent_text`  | `#1a0d0e` | text on accent: near-black tinted toward the accent, or near-white if the accent is dark (choose by contrast) |
| `border`       | `#1f2a36` | `surface` lightened about 8%. Unfocused pane borders and separators |
| `border_ctrl`  | `#253040` | `surface` lightened about 12%. Input and chip borders |
| `panel_dim`    | `#0b1219` | between `background` and `surface`. User panel and footer strips |
| `proto_mx`     | `#8fd6cc` | Matrix tag colour (fixed, fairly light) |
| `proto_dc`     | `#9aa6f5` | Discord tag colour (fixed, different lightness from mx) |
| `success`      | `#8fc79a` | online, synced, speaking ring |
| `warning`      | `#e0b36a` | idle, unverified session |
| `danger`       | `#e07a7a` | mention badges, muted mic, errors |

Fallback when colors.conf is missing: the defaults above. The existing
presets (Catppuccin Mocha, Nord, Gruvbox Dark) stay available, mapped onto the
same fields.

### Layout: read from `~/.config/hypr/hyprland.conf` (optional)

Parse only the `general { … }` block: `gaps_in`, `gaps_out`, `border_size`,
and `decoration { rounding }`. If the file is missing or the toggle is off, use the defaults.

| key           | default | used for |
|---------------|---------|----------|
| `gaps_out`    | 8 px    | window padding and the gap between panes |
| `gaps_in`     | 4 px    | gap inside panes (lists, message groups) |
| `border_size` | 2 px    | pane border width |
| `rounding`    | 10 px   | pane radius. Controls use `rounding * 0.6` (6 px), chips 4 px |

### Type

- UI: **IBM Plex Sans** 14 px (400/500/600). Fallback: system sans.
- Mono: **JetBrains Mono** 11–12 px for tags, timestamps, section labels,
  keybind hints, code, IDs. Fallback: `fc-match monospace`.
- Section labels: mono 11 px, uppercase, letter-spacing 0.04em, `text_muted`.
- Ship both fonts inside the binary (`iced::font::load`) so Windows looks the same.

## 3. Screens

### 3.1 Main window (`screens/Main.dc.html`, 1440×900)

```
┌ top bar ─────────────────────────────────────────────────────────────┐
│ >_ mesh  [2] tiling.club / #rice-showcase   [ search… Ctrl K ]  ●mx strange.lab ●mx matrix.org ●dc bridge [add friend] │
└──────────────────────────────────────────────────────────────────────┘
┌rail┐┌ channels ──┐┌ chat (focused, accent border) ──────────┐┌ members ┐
│ ⌂  ││ tiling.club ││ # rice-showcase  topic…   pin bell ppl ││ ONLINE—4│
│ ── ││ dc bridged… ││──────────────────────────────────────── ││ …       │
│ SL ││ TEXT        ││ ── today ──                             ││ OFFLINE │
│1·mx││ # announce  ││ (N) nixwitch [dc] 14:02                 ││ …       │
│ TC ││ # rice-show ││     text / image / reactions            ││         │
│2·dc││ …           ││ (T) tux_fan [mx · matrix.org] 14:05     ││         │
│ …  ││ VOICE       ││     ``` code block ```                  ││         │
│ +  ││ jam-room    ││ → kernel_panic joined from mx arch-za   ││         │
│    ││  participants│ [clip][ message #… — sends to discord ][>]│         │
│    ││ [S strange mic deafen settings] ││ tux_fan is typing…  / commands … │ │
└────┘└────────────┘└──────────────────────────────────────────┘└────────┘
```

- **Top bar** (its own pane, 2 px `border`): wordmark `>_` in accent. A breadcrumb
  with the space number in a `surface_alt` chip. A search button that opens the
  `Ctrl+K` palette. Connection chips (dot plus `mx`/`dc` plus name), one per
  homeserver and one for the bridge. A primary "add friend" button opens dialog 3.3.
- **Space rail** (64 px): Home tile, then a divider, then one 44×44 tile per space
  (initials in mono 13/700). Below each tile is `n·mx` / `n·dc`, which is its `Alt+n`
  key plus its network. The active tile has a `surface_alt` fill and a 2 px accent ring.
  Mentions show a `danger` badge at the bottom-right. The dashed "+" tile joins or creates a space.
- **Channel pane** (232–260 px): space name, a line saying whether it's bridged, then
  TEXT and VOICE sections. Active channel: `surface_alt` fill plus a 2 px inset
  accent bar on the left. Unread channels use `text_bright` and weight 600, with a count chip.
  The voice channel shows its elapsed time, with participants listed under it (green
  ring while speaking).
- **User panel** (pinned to the bottom of the channel pane, `panel_dim`): avatar
  with a status dot, display name, full MXID in mono, then mic, deafen and settings buttons. Muted
  means `danger` icon on a dark red fill. Deafen also mutes.
- **Chat pane**: header (channel name, topic, pins, notifications, members toggle).
  The timeline groups messages by author (avatar only on the first message of a group). Each
  author row shows name, tag (`mx · <server>` for Matrix, `dc` for Discord ghosts)
  and time in mono. Images go in a figure with a mono caption (filename · size).
  Code blocks use `background` with a 1 px border, in mono 12. Reactions are pills; the
  user's own reaction gets an accent border. System events are mono 11 with an arrow.
  The composer has an attach button, an input whose placeholder says where the message
  goes ("sends to discord via bridge" in bridged rooms), and an accent send button.
  Under it, on the left: the typing indicator. On the right: key hints.
- **Member pane** (220–250 px, toggled from the chat header and hidden on narrow
  windows): ONLINE and OFFLINE groups. Each row has an avatar with a status dot, name, status
  line and network tag. Offline rows are at 60% opacity.

### 3.2 Rice settings (`screens/Rice.dc.html`)

Settings layout: nav on the left (back link plus Esc, USER: Accounts & bridges,
Friends, Privacy & E2EE · APP: **Rice**, Keybinds, Voice & audio,
Notifications), content pane in the middle (focused), preview and export on the right.

- **Theme source** (radio cards): Follow my rice (colors.conf, "● watching ·
  reloaded HH:MM"), Built-in preset, Custom theme.toml.
- **Token table**: key · swatch · hex · what it's used for · `rice`/`derived` tag.
  Footnote: derived colours are adjusted until they pass 4.5:1.
- **Layout**: four read-only boxes (gaps_in, gaps_out, border_size, rounding,
  each with a bar). Density switch: `compact` (no avatars, one line per message),
  `cozy` (default), `irc` (`[14:02] <nick> text`).
- **Extras** (switches): Digital rain background (the existing `rain.rs`), Translucent
  panes (92% pane opacity so Hyprland's blur shows through), Show platform tags,
  Plain-text mode (no embeds or previews).
- **Right column**: a small live preview of the window, the generated `theme.toml`,
  "Export theme" (primary) and "Share to #rice-showcase".

### 3.3 Add friend dialog (`screens/AddFriend.dc.html`, 720×680)

- Mode switch: `auto-detect` (default) · `matrix` · `discord`.
- One input with a network tag on the left (`mx`/`dc`/`··`) and the send button on the right.
  Detection rules (see `detect()` in the mockup):
  - `^@[^:]+:[host](:port)?$` means a Matrix user ID.
  - In matrix mode, a bare name becomes `@name:<primary homeserver>`, and the hint says so.
  - `^[a-z0-9_.]{2,32}$` means a Discord username (new style, no discriminator).
  - `name#1234` gives the error "Discord dropped #1234 tags".
  - `@name` with no server gives the error "Matrix IDs need a server: @name:matrix.org".
- The hint line under the input is coloured by result: mx colour, dc colour or danger.
- Preview card (profile lookup): avatar, name, route (`target → from <your account>`),
  shared rooms or servers.
- Explanation box. **Matrix has no friend list.** Mesh opens an encrypted DM
  (`is_direct`) and stores contacts in account data
  (`m.direct` plus a custom `org.mesh.contacts` event). Discord: the request goes through the
  bridge (see §5).
- Pending list: incoming requests (Accept/Ignore) and outgoing ones (Cancel), each with a network tag.

### 3.4 Accounts & bridges (`screens/Accounts.dc.html`)

- **Matrix homeservers**: one row per account (initials tile, MXID, PRIMARY
  badge, server info, sync state, verification and key-backup state). An add row
  with the homeserver input (`.well-known` discovery), Sign in, and SSO/OIDC.
  This is **multi-account**, so `mesh-core` needs `Vec<MeshClient>`, one per account.
- **Discord through a bridge**: radio cards for "My homeserver's bridge
  (mautrix-discord on <hs>)" (recommended) and "A bridge I trust" (enter the bot MXID).
  Linked-account row (QR login, server count, Unlink). Per-server bridge switches.
- **Warning box** (warning colours): Discord doesn't allow third-party clients, and a
  bridged personal account can be flagged. Show it once and remember that the user saw it.
- **Right pane**: a diagram of where a message goes (mesh → homeserver → federation
  / appservice → discord).

## 4. Mapping to the current code

| design element | where it lives today | change |
|---|---|---|
| Palette fields | `mesh-app/src/theme.rs` `Palette` (10 fields) | add the derived fields and a `Layout { gaps_in, gaps_out, border_size, rounding }`. Add a `colors.conf` loader and a `hyprland.conf` loader. Add a file watcher (notify, or poll mtime every 1 s in a `Subscription`) |
| Three panes | `view()` in `main.rs` | wrap each pane in a styled `container` (border width = `border_size`, radius = `rounding`); the window gets `padding(gaps_out)` and `spacing(gaps_out)`. Add the space rail and the top bar |
| Focused pane | none | track `focused: Pane` in state, set on click or keyboard. The border colour switches to accent |
| Rooms list | DM / Rooms sections | regroup as spaces (`m.space` hierarchy) → channels. DMs and friends move under Home |
| Digital rain | `rain.rs` | keep it, behind a switch in Rice settings |
| Calls | `call.rs` 1:1 | the voice channel UI is future work; the user panel's mute and deafen already exist |
| Settings | gear → theme switcher | becomes the settings layout with the Rice page; Accounts is a new page |

## 5. Bridge detection (mesh-core, Matrix-only)

- A room is bridged if its state has an `m.bridge` / `uk.half-shot.bridge`
  event (mautrix bridges send these). Read `protocol.id == "discord"`.
- A user is a Discord ghost if their MXID matches the bridge's user namespace
  (mautrix-discord's default is `@discord_<id>:<hs>`). Expose this as
  `RoomSummary.network: Network::{Matrix, Discord}` and
  `TimelineMessage.sender_network`.
- Linking an account, logging in with a QR code and choosing servers to bridge all go through the bridge bot's DM
  commands (`login-qr`, `guilds bridge <id>`, and so on). Check the command names against
  the version of mautrix-discord you have installed.
- **Discord friend requests: not verified.** Check whether your mautrix-discord
  version supports sending or accepting relationships. If it doesn't, show the Discord
  path as "DM via shared server" or disable the button with a tooltip. Never call
  Discord's API from Mesh directly.
