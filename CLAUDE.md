# CLAUDE.md

Internal notes for Claude (and any other agent) working on this repo.

## What this is

A Rust-native SSH client built on iced. Workspace of 24 crates; the
main ones:

| Crate | Role |
|-------|------|
| `oryxis-core` | Pure model types (`Connection`, `Identity`, `ProxyIdentity`, `Group`, `SshKey`, `KnownHost`, `PortForwardRule`, `SessionGroup`, `CloudAccount`, custom themes, etc.) |
| `oryxis-ssh` | russh-based SSH engine (connect, jump hosts, SOCKS/HTTP/Command proxies, `-L`/`-R`/`-D` forwarding, SFTP) |
| `oryxis-archive` | SFTP archive ops: remote command synthesis + safe quoting (tar/unzip/zip over exec, POSIX + Windows), local zip/tar.gz codecs, and zip central-directory browsing over ranged reads (`RangedSource`) |
| `oryxis-vault` | Encrypted SQLite vault (Argon2id + ChaCha20Poly1305) + portable export/import |
| `oryxis-sync` | P2P sync (QUIC + mDNS + STUN + Ed25519/X25519 LWW) |
| `oryxis-relay` | Self-hostable signaling + relay HTTP server (axum) for sync over the internet |
| `oryxis-terminal` | Embedded alacritty terminal + custom widget (17 themes + custom themes, URL/IP/path detection) |
| `oryxis-mcp` | MCP server binary (JSON-RPC over stdio). Distributed as a plugin (`mcp-v*` release tags + `mcp.json` manifest), not bundled in the OS installers. |
| `oryxis-app` | Iced UI, dispatcher, views, AI chat |
| `oryxis-cloud` | Cloud provider abstraction (`CloudProvider` trait: discovery + transport) |
| `oryxis-cloud-aws` | AWS provider impl (profiles / static keys / IAM Identity Center, EC2 + ECS) |
| `oryxis-cloud-k8s` | Kubernetes provider impl (kubeconfig auth, workload discovery via `kubectl`) |
| `oryxis-cloud-aws-plugin` | AWS provider as a subprocess binary (JSON-RPC 2.0 over stdio) |
| `oryxis-cloud-k8s-plugin` | Kubernetes provider as a subprocess binary (JSON-RPC 2.0 over stdio) |
| `oryxis-plugin-protocol` | Wire protocol for cloud-provider plugins (line-delimited JSON-RPC 2.0 over stdio) |
| `oryxis-plugin-signer` | CLI to Ed25519-sign plugin binaries + compute the manifest SHA-256 |

**A new crate lands with its documentation in the same change.** Same discipline as i18n keys, keynav and the settings index: three places describe the workspace and they drift silently, because nothing compiles them.

- `CLAUDE.md` (this table) — one row, the agent-facing role.
- `docs/ARCHITECTURE.md` — the `## Crates` table AND the layer diagram above it (a crate absent from the diagram reads as "not part of the system"), plus the crate count in the opening paragraph.
- `CONTRIBUTING.md` — the crate count in the "map the N-crate workspace" line, and the engine list next to it if the new crate is one a contributor would plausibly start in.

The count is workspace MEMBERS, not directories under `crates/`: `oryxis-gif` is deliberately excluded from the workspace (its render stack would land in every `cargo check --workspace`), so it is 24 members across 25 directories. Verify with the `members` list in the root `Cargo.toml` rather than `ls`.

The same rule applies to removing or renaming a crate, and to a crate that changes role enough that its one-line summary stops being true.

## Build / test gates

```bash
cargo check --workspace
cargo test --workspace --lib --bins
cargo clippy --workspace --all-targets -- -D warnings   # CI gate
```

`cargo fmt --all` reformats every file (including ones unrelated to your
edit). Don't run it blindly. Format only the files you touched, or skip it
entirely and match the file's existing style.

## Headless E2E harness

The app has a Playwright-style headless harness behind the `harness`
cargo feature (dev-only, zero release weight), built on the iced
fork's `iced_test` Emulator: it runs the REAL app (vault,
subscriptions, side effects) with no window and renders PNG
screenshots. Full manual in `docs/HARNESS.md`. Quick start for QA:

```bash
cargo build -q -p oryxis-app --features harness
target/debug/oryxis --harness-serve &                # daemon (TCP 127.0.0.1:6799)
target/debug/oryxis --harness-ctl 'click "Keychain"' # one-shot CLI client
target/debug/oryxis --harness-ctl quit               # stop (before rebuilding)
cargo run -p oryxis-app --features harness -- \
    --harness-run crates/oryxis-app/tests/e2e --home "$(mktemp -d)"  # CI batch
```

PREFERRED for agent sessions: the daemon + ctl pair above, documented
as the project skill `.claude/skills/harness/SKILL.md` (lifecycle,
patterns, gotchas). The agent owns the whole loop from Bash: quit,
rebuild, restart, drive, screenshot (ctl prints the PNG path; Read
it). Batch commands via stdin heredoc; exit code 1 on any
`== error` / `== fail`. Start reproducible flows with `reset wipe`,
commit a flow with `save crates/oryxis-app/tests/e2e/<name>.ice`.
`--harness-repl` (same grammar on stdin/stdout) and `--harness-mcp`
(MCP over stdio) remain for other clients.

Command grammar (shared by ctl / REPL / `.ice` files,
`harness/commands.rs`): any `.ice` instruction (`click "Text"`,
`type "x"`, `type enter`, `expect "Text"`) plus `screenshot [name]`,
`texts` (text dump with bounds = DOM inspector), `find "Text"`,
`wait <ms>`, `settle`, `timeout <ms>`, `status`, `reset [wipe]`,
`save <path>`, `quit`. Responses are `== `-prefixed. `$HOME` is
sandboxed (default `<tmp>/oryxis-harness`, persistent; `--home`
overrides) so runs never touch the real `~/.oryxis`.

The CI batch runner (`--harness-run`, `harness/batch.rs`) runs the
`.ice` files in file-name order, each on a freshly WIPED sandbox
(every test starts at first-run), with the interactive modes'
per-instruction timeout so a live PTY can't deadlock a test, and it
accepts the pacing lines (`settle` / `wait` / `timeout` /
`screenshot` / `#` comments) in committed tests, so terminal flows
are batchable; canvas assertions stay screenshot-based (collect the
shots dir as a CI artifact). `save_ice` records pacing lines too.

The emulator improvements live in the iced fork's `oryxis` branch
(pushed 2026-07-10, rev e5be0795; local clone `/home/wilson/iced`,
Cargo.lock repinned, no [patch] needed): Emulator::screenshot cache
fix (full widget-state fidelity in shots), public
`Emulator::operate` (backs `texts`/`find`), emulated in-memory
clipboard (widget paste + clipboard tasks, fulfilled per event),
interaction broadcast to subscriptions (global hotkeys like Ctrl+K
work), `scroll (dx, dy)` and `type ctrl+shift+f` chord grammar,
plus parser fixes (`click right "X"` target, `press enter` as
keyboard). A second dev-only feature `tester`
(`cargo run --features tester`) runs the windowed app with iced's
F12 record/play overlay that captures interactions as `.ice` files
the harness replays headless.

Caveats: text selectors can't see inside the terminal canvas and
don't match text_input VALUES (verify via screenshot); app copies
that use `arboard` directly bypass the emulated clipboard and hit
the real system clipboard.

## Architectural conventions

### Vault & encryption

- One SQLite file. Schema-versioned via `ALTER TABLE` migrations in
  `store/schema.rs::create_tables`.
- `store.rs` was split into a `store/` module (one file per entity
  family: `connections.rs`, `identities.rs`, `cloud.rs`, `logs.rs`,
  `sync.rs`, etc.; `mod.rs` keeps the struct, open/unlock, crypto field
  helpers and key rotation). Every file is `impl VaultStore` over the
  same private fields. Tests mirror this under `store/tests/`.
- Secrets (passwords, private keys) live in their own `BLOB` columns,
  encrypted per-field with the master key. Plaintext columns (JSON,
  text fields) **must not** carry credentials — the test
  `proxy_password_does_not_leak_into_proxy_column` enforces this for
  proxies.
- API for password fields follows a tri-state model:
  - `None` → preserve the existing column value
  - `Some("")` → clear it
  - `Some(pw)` → encrypt + store

### `Connection.proxy` resolution

A connection can express its proxy in two ways:

1. **Inline** — `Connection.proxy: Option<ProxyConfig>` (host/port/user
   in JSON; password in the encrypted `proxy_password` column).
2. **Identity reference** — `Connection.proxy_identity_id: Option<Uuid>`
   pointing at a `proxy_identities` row.

`Vault::resolve_proxy(&Connection)` returns the effective `ProxyConfig`
with password hydrated. **Identity wins over inline** when both are
set. A dangling identity (id no longer exists) resolves to `None` with
a warning — never an error, so a deleted proxy doesn't break every
host that referenced it.

The SSH engine consumes `Connection.proxy` only — callers
(`dispatch_ssh.rs`, `mcp/handlers.rs`) collapse the resolved value
into `conn.proxy` just before handing the connection off.

### Jump hosts + proxies

`engine::connect_via_jump_hosts` honors the **first** jump's proxy when
dialing the bastion. Subsequent hops travel inside the SSH tunnel, so
their proxy fields don't apply. Per-jump proxies are passed in
`ConnectionResolver.proxies: HashMap<Uuid, ProxyConfig>`, populated by
the caller via `Vault::resolve_proxy` for each id in `jump_chain`.

### SSH config import

`ssh_config.rs` parses `~/.ssh/config`. `ProxyCommand` maps directly to
`ProxyType::Command(cmd)`. `ProxyJump` is alias-resolved in a second
pass (`link_proxy_jumps`) once every imported host has been assigned
its UUID. Unresolved aliases are recorded in `Connection.notes` rather
than failing the import.

### Sync

`oryxis-sync` is opt-in P2P over QUIC. Manifest entries cover all
syncable entity types (`EntityType::Connection / SshKey / Identity /
Group / Snippet / KnownHost / ProxyIdentity`).

Wire payloads for connection / identity / proxy-identity use wrapper
structs (`SyncConnection`, `SyncIdentity`, `SyncProxyIdentity`) that
flatten the inner model and add `#[serde(default)]` `password` fields.
Forward + backward compatibility is automatic: older peers send bare
JSON which still deserializes; older peers receive new JSON and ignore
the unknown fields.

**Password sync is opt-in** via the `sync_passwords` setting (Settings
→ Sync toggle). When off, password fields are omitted from the wire
payload (`#[serde(skip_serializing_if = "Option::is_none")]`).

**No hosted default (since 2026-07-10).** `SyncConfig::default()` has
`signaling_url: None`; fresh installs are LAN-only until the user picks
an internet backend (SFTP snapshot recommended, or a self-hosted
relay). The old baked-in Worker URL survives only as
`config::legacy_hosted_signaling()`, consumed once by the boot
grandfather migration (`boot/load.rs`, `sync_hosted_migrated` flag)
that writes it into the settings of vaults that were actually syncing.
Settings > Sync (P2P) has a "Set up your own relay" wizard
(`RelayWizardForm` in `state/sync.rs`) that generates compose/systemd/
Caddy files and adopts the endpoint after a `/healthz` probe. Long-poll
economics: client window 110s, servers cap 120s, worker.js in-window KV
poll decays 500ms→5s; front proxies need read timeouts above 150s.

### i18n

All user-facing strings go through `crate::i18n::t("key")`. The tables
live in `i18n/` (one module per language: `i18n/en.rs`, `i18n/ko.rs`,
...) wired through `i18n/mod.rs`, which holds the `Language` enum,
`translate()` dispatch and `t()`. The English table (`i18n::en`) always
returns a value (`_ => "???"` fallback); the other 22 languages expose
`pub(super) fn lookup(key) -> Option<&'static str>` and fall back to
English on `None`. New keys must be added to **all 23** language
modules.

The 23 languages today: English, Português (BR), Spanish, French,
German, Italian, Chinese Simplified (`zh`), Japanese, Russian, Persian
(`fa`), Arabic (`ar`), Korean (`ko`), Polish (`pl`), Turkish (`tr`),
Indonesian (`id`), Vietnamese (`vi`), Ukrainian (`uk`), Hebrew (`he`),
Chinese Traditional (`zh-TW`, Taiwan vocabulary, not a script-only
conversion), Thai (`th`), Hindi (`hi`), Czech (`cs`), Greek (`el`).
Persian, Arabic and Hebrew flip `Language::is_rtl()`.

Fonts: Hebrew / Thai / Devanagari are bundled statics next to Arabic
(`main.rs`); Traditional Chinese is a fourth on-demand CJK download
(`fonts.rs`, Noto Sans TC). `MenuCJK.ttf` is the picker-name subset
(한국어 / 简体中文 / 繁體中文 / 日本語); regenerate it when a CJK
`Language::name()` changes (script: fontTools instancer + subset +
merge, see the `menu_cjk_covers_picker_names` test).

### RTL layout

`crate::i18n::is_rtl_layout()` resolves the user's `LayoutDirection`
setting (`Auto` defers to `Language::is_rtl()`; explicit
`LeftToRight` / `RightToLeft` overrides). Use this signal — never
match on language directly — when writing direction-aware code.

Two `widgets` helpers cover the common cases:

- `widgets::dir_row(items)` builds a `Row` whose children are
  reversed under RTL. Use anywhere the *physical* placement of
  widgets should mirror — sidebar/content split, leading/trailing
  icon pairs, toolbar action buttons. Don't use `iced::widget::row!`
  for these — the macro can't be reversed after construction.
- `widgets::dir_align_x()` returns `Horizontal::Right` under RTL
  and `Horizontal::Left` otherwise. Apply to `Column::align_x()` /
  `Container::align_x()` when a `Length::Fill` child should hug the
  *leading* edge instead of the physical left edge. Note that the
  parent column / container also needs `Length::Fill` width — without
  slack to align inside, the alignment has no effect.

For the keychain split-button "+ ADD ▼" pattern, the rounded outer
corners need to swap sides under RTL too — compute `Radius` from
`is_rtl_layout()` rather than hard-coding LTR corner positions.

iced doesn't auto-flip text alignment in `Length::Fill` containers,
icon glyphs, or scrollbar position. The first two are handled by
`dir_align_x()` (alignment) and `panel_right_*` icons (the sidebar
collapse glyph swaps from `panel_left_close/open`). Scrollbar
position remains physical-right and isn't fixable from the iced
0.13/0.14 API.

### Windows installers

Two NSIS scripts in `resources/`, both parametrized with
`/DVERSION /DARCH /DBINPATH`:

- `installer.nsi` — system, `$PROGRAMFILES64\Oryxis`,
  `RequestExecutionLevel admin`, `HKLM` registry. Output:
  `oryxis-setup-<arch>.exe`. This is what `winget install` targets.
- `installer-user.nsi` — per-user, `$LOCALAPPDATA\Programs\Oryxis`,
  `RequestExecutionLevel user`, `HKCU` registry. Output:
  `oryxis-user-setup-<arch>.exe`. Detects existing system install on
  `.onInit` and warns; never auto-uninstalls (would need elevation
  it doesn't have).

`resources/logo.ico` is the icon for all of it (the `winresource`
resource in `build.rs`, `MUI_ICON`, the Start Menu / desktop shortcuts,
the uninstall entry) and carries one entry per size, 16 through 256,
each rendered from `logo.svg` rather than resampled: Windows picks the
entry matching what it is about to draw, and a lone 256 leaves it
downscaling with GDI in the title bar and Explorer. Regenerate with
`resources/make-icon.sh` whenever the logo changes. Sizes below 128
stay DIBs there because PNG entries are only understood from Vista on
and some tooling still expects the classic payload. The window icon
(`main.rs::load_icon`) is separate and only ever becomes `ICON_SMALL`.

CI builds both for `x86_64` and `aarch64` from `windows-latest`. The
NSIS toolchain is x86 (no native ARM64 makensis upstream); ARM64
installers run under x86 emulation on install but lay down native
ARM64 binaries.

`PATH` is managed via the EnVar plugin (pinned `v0.3.1` from
`GsNSIS/EnVar`). Don't roll PATH manipulation by hand —
`WriteRegExpandStr` truncates at `${NSIS_MAX_STRLEN}` (1024 chars).
Always `EnVar::SetHKLM` (system) or `EnVar::SetHKCU` (per-user)
before each `AddValue`/`DeleteValue` call.

The auto-updater (`update.rs::launch_installer`) uses `ShellExecuteW`
with `verb=NULL` so the installer manifest controls UAC. Don't go
back to `Command::new(path).spawn()` — it returns
`ERROR_ELEVATION_REQUIRED` (740) on the system installer.
`pick_asset` checks `is_per_user_install()` (current_exe inside
`%LOCALAPPDATA%`) to choose between system and per-user
artifacts; the system fragment has an explicit `user-setup`
exclude so it doesn't grab the wrong file.

### Iced patterns specific to the wilsonglasser fork

- `pick_list(selected, options, mapper).on_select(callback)` — the
  fork's API is 4-step (mapper closure converts `&T` → `String` for
  display; `on_select` is a separate chained call). Don't try the
  upstream 3-arg form.
- For typed enum pickers (e.g. `ProxyKind`), implement `Display` so
  the mapper can be a simple `|k| k.to_string()`. When the rendering
  needs a runtime list lookup (e.g. resolving `Identity(Uuid)` to a
  user label), capture the list in the mapper closure.

### Message sub-enums (convention)

The app `Message` enum (`crates/oryxis-app/src/messages/`) is split into
**one sub-enum per dispatch domain**, wrapped in the top-level enum as
`Message::<Domain>(<Domain>Message)` (e.g. `Message::Ssh(SshMessage)`,
`Message::Sftp(SftpMessage)`). Each sub-enum lives in its own file
(`messages/<domain>.rs`), is re-exported from `messages/mod.rs` and
`app.rs`, and its handler (`dispatch_<domain>`) matches
`Message::<Domain>(<Domain>Message::Variant)`.

The **only** variants that stay flat on the top-level enum are the
documented cross-cutting globals handled outside any single domain:
`NoOp`, `OpenUrl`, `CopyToClipboard`, `ClipboardWritten`, `ToastClear`,
`ToastDismiss`, `ErrorDialogDismiss`, `ErrorDialogRunAction`,
`TogglePrivacyReveal`.
Plus the domain wrappers themselves (and `SftpFor`, a second owner-routed
wrapper for `SftpMessage`). Nothing else belongs at the top level.

Rules when adding messages:

- **A new message-heavy feature area is born as its own sub-enum**, never
  as flat `Message` variants. Same rule as i18n keys and keynav: wiring
  the sub-enum is part of the change, not a follow-up. Small additions to
  an existing domain go into that domain's sub-enum.
- **Domain membership is by name-family / owning handler, not by prefix.**
  A variant whose logic lives in another handler still belongs to its
  semantic domain (e.g. the session-logging / os-detect settings toggles
  are handled in `dispatch_ssh` but live in `SettingsMessage`; the
  `Editor*` env-var / port-forward fields are matched in `dispatch_mcp`
  but live in `EditorMessage`).
- **Keep variant names verbatim inside the sub-enum** (no prefix strip),
  unless *every* variant shares one prefix — then strip it, because
  `clippy::enum_variant_names` fires on a uniform prefix. Four domains
  stripped prefixes during the conversion: `tray` (`TrayMessage::
  MenuEvent`), `onboarding`, `player` (`SessionPlayer*` dropped,
  `PlaySessionLog` -> `Open`) and `sync` (`Sync*` dropped).
- **Variant names must stay unique across ALL the sub-enums.** Two enums
  may legally declare the same simple name and the wrappers make either
  compile at any send-site, so a same-name pair is a permanent
  wrong-wrapper landmine (the sync prefix strip minted three collisions
  with `SftpMessage`, renamed to `SftpHostPickerOpen/Close/Search`).
  Before adding a variant, grep the other `messages/*.rs` for the name.
- Sub-enum files derive `#[derive(Debug, Clone)]` and import only what
  their variants need; prefer fully-qualified `crate::state::…` /
  `uuid::Uuid` in bodies to keep imports minimal. Variants carrying
  `Box<Message>` (envelopes) import `use super::Message;`.
- **Domain routers are exhaustive per-variant matches** — no `_ => {}`
  tail, no `Err`-fallthrough chain. A multi-file domain routes each
  variant group straight to its owning sub-handler
  (`m @ (X::A | X::B(..)) => self.handle_x_sub(m)
  .unwrap_or_else(crate::dispatch::unrouted)`); the sub keeps its
  `Result` signature and `m => Err(m)` tail, whose only remaining
  meaning is "listed under the wrong group" (loud via `unrouted`, never
  a silent drop). Adding a variant without an arm is a compile error.
  References: `handle_sftp_domain` (`dispatch_sftp/mod.rs`),
  `handle_settings`, `handle_keys`. Exception: `handle_sftp_transfers`
  declines wholesale when no SFTP tab owns the continuation — its group
  drops quietly by design.

The mechanical per-domain conversion tooling and its hazards (the
`subenum.py` `Message::`-inside-`XMessage::` substring trap, splicing into
an existing sub-enum, orphan-import cleanup) are documented in the
`project-message-subenum-conversion` memory. `git log --grep
"extract .*Message sub-enum"` shows the per-domain history.

### Settings search index (convention)

Settings has a sidebar search (plus per-setting command-palette rows)
backed by the hand-maintained catalog `settings_index.rs`
(`SETTINGS_INDEX`: section + label i18n key + English keywords).
**A new setting row lands with its index entry in the same change**,
same discipline as i18n keys and keynav. Matching runs against the
active-language label AND the English label (`i18n::en_lookup`), so
English queries work in any UI language; section visibility reuses
`settings_section_items()` so feature-gated sections never leak
results. Activating a result fires `SettingsMessage::RevealSetting`,
which opens the section and rings + scrolls the row via the keynav
ring (`settings_pending_reveal` / `keynav.reveal_row_idx` handshake):
`nav_toggle_row` / `nav_pick_row` rows participate automatically;
inline rows must register through `settings_nav_slot_labeled` /
`settings_nav_record_labeled` (label = the row's visible `t(...)`
label) or the reveal falls back to just opening the section. Tests in
`settings_index.rs` assert every key resolves in English and no
(section, key) pair repeats.

### Translucent terminal background (convention)

`terminal_opacity` (Settings > Terminal, 100 = opaque) fades the
terminal's own backdrop so the desktop shows through. Two rules make it
correct, and both are easy to break by accident:

- **Exactly one layer carries the alpha.** That layer is the container
  in `views/terminal.rs::view_terminal`; the canvas hands its
  full-bounds fill over to it via `with_transparent_bg`, and the root
  container in `main_layout.rs` stops painting while a translucent
  terminal is on screen (it sits underneath, so painting it would make
  the alpha reveal `bg_primary` instead of the desktop). Two translucent
  fills of the same colour composite into a plate nobody asked for.
- **Everything else stays opaque on purpose.** Chrome (tab strip, status
  bar, sidebars) and the vault views paint their own backgrounds, which
  is what keeps text readable over an arbitrary wallpaper. There is no
  "whole window" mode: winit exposes no window-level opacity, so
  simulating one by alpha per layer would make regions with more layers
  visibly more opaque.

`theme::alpha_for_opacity()` is the single authority and gates on BOTH
the setting and `window_transparent()`, which records how the window was
actually created (`main.rs`, before the runtime builds it). A window born
opaque can never composite, which is why the first step away from 100%
offers a restart and every later change is live.

The **background image** is its own canvas UNDER the grid canvas
(`widget/backdrop.rs` draws base colour + picture, `widget/background.rs`
keeps the pure fit geometry; the app stacks the two per pane in
`render_pane_canvas`, and the grid runs `transparent_bg`). It CANNOT be
drawn inside the grid's frame: within one render layer both iced
renderers draw by primitive KIND (quads → meshes → images → text), so a
picture in the same frame sits over every `fill_rectangle` no matter the
call order — it buried the fade veil, the selection, the cursor and the
cell backgrounds, and only the glyphs survived (the original "Fade image
does nothing" bug). The fade itself is baked into the picture as
`opacity = 1 - dim`, never veiled over it with a translucent fill (a
veil is a mesh and loses to the same stage order); over the base fill
the two composite identically. Three consequences worth keeping: the
measured size is PART of the backdrop's cache key (it is `None` until
the picture decodes, and a cached blank frame would never invalidate),
`Stack` is what gives the grid its own layer above the picture (children
after the first get `with_layer`), and a picture beats opacity, so
`resolve_terminal_appearance` reports opaque while one is set rather
than compositing a layer nobody can see.

Per-host overrides live in `Connection.terminal_appearance`
(`TerminalAppearance`, the `quirks` column pattern). Every field is
`Option` and resolves INDEPENDENTLY against the global setting, so a
host overriding the fade still follows the global picture; `Some("")`
on the image is the explicit "no picture on this host", which is a
different answer from `None` once a global picture exists. Only the
path is stored, never the pixels.

### Highlight rules + triggers (convention)

The user's own colouring rules (C6). Two consumers, two mechanisms, and
mixing them up is the mistake this section exists to prevent:

- **Colour reads the GRID.** `widget/highlight.rs::detect_rule_highlights`
  runs per frame in its own list, separate from the automatic
  URL / IP / path detectors, for two reasons: a user rule WINS over a
  heuristic (the draw pass consults the rule list first), and it must
  not join the automatic detectors' overlap negotiation. It is gated
  only by performance mode, NOT by the `keyword_highlight` toggle, which
  governs the automatic detectors alone.
- **Actions read the STREAM.** `trigger.rs` accumulates lines from the
  bytes, in `backend::process()` next to the OSC sniffer, because a
  grid-based trigger would re-fire every time the same text was
  redrawn. It keeps a write cursor so `\r` OVERWRITES instead of
  clearing: every PTY ends its lines with `\r\n`, so "CR discards the
  line" means nothing ever fires (caught by a test, after the UI
  looked fine). Suppressed on the alternate screen, so actions do not
  fire inside tmux / vim / htop; the colouring still works there.

`CompiledRule` carries a `triggers: bool` and NOT the action itself: the
terminal crate never learns what an action is, and the app maps the id
back. The scanner returns immediately when no rule has an action, which
is what keeps the session player and history viewer free.

Both consumers take the SAME `Arc<CompiledRules>`, RESOLVED PER HOST by
`highlight_rules_for(conn_id)` in `app/highlight_rules.rs`: the global
list plus (or replaced by) `Connection.highlight_rules`, which is its
own JSON column and rides sync / portable export on the Connection's
`#[serde(flatten)]`. Append vs replace is the USER's choice per host
(`HostHighlightRules.replace`), because the override is a LIST, so
"inherit" is genuinely two answers; `replace` with an empty list is the
noisy-host off switch, which is why it is not the same as `None`. The
host's rules come first (order is precedence).

The resolution is cached per connection id and keyed by a SIGNATURE of
its inputs (the global digest plus a hash of the host's rules), never
invalidated by hand: manual invalidation would have to be remembered at
every site that edits, imports or syncs a host. The cache is a
`RefCell` because `view()` needs it too, and the widget MUST paint from
the same set the backend watches with. The backend is handed it in the
output funnel (a pointer comparison per batch) rather than at pane
creation, because panes are born down half a dozen paths and one of
them would eventually be missed.

ONE list and ONE editor serve both scopes, and they live in different
places. The LIST is inline in both surfaces
(`highlight_rules_block(scope, rules)`, `views/settings/highlight_rules.rs`):
rows, reorder arrows, the enable checkbox and the delete confirmation,
recording on the ring that owns the surface (`settings_nav_*` vs
`panel_nav_*`, via `hl_nav_slot`), with the narrow host panel stacking
what the wide Settings card puts on one line. The EDITOR is a modal
(`Modal::HighlightRuleEditor`, `views/settings/highlight_rule_modal.rs`)
whose rows record on the MODAL ring; `form.scope` is what tells it which
list to commit to, so one card serves both. `highlight_rule_editor_open()`
is the single authority for "is it up", read by BOTH the render site and
`is_modal_open`, and it gates on the surface that owns the list actually
being on screen (the layout's own predicate, not just the panel flag) so
no async path can leave a modal floating over an unrelated view.

**A modal over the host panel is exposed to a fork bug**: the iced fork's
`text_input` returns its `on_submit` binding for Enter BEFORE the
`is_focused` gate (`core/src/text/editor.rs::from_key_press`), so every
visible `text_input` carrying `on_submit` fires on ANY Enter, focused or
not. `any_modal_blocks_input` only governs the global key subscription,
not the widget tree, so an Enter meant for the modal also reached the
host panel's `EditorSave` (8 of them, one per field) and the empty
state's `QuickHostContinue`, both of which rebuild `editor_form` and
silently discarded the rule the modal had just added. Both handlers now
decline while a modal blocks input. Any new handler reachable from an
`on_submit` needs the same guard until the fork gate moves.

The snippet action is the only dangerous one, and it is guarded
structurally: what decides it fires is text the REMOTE HOST printed, so
it asks once per rule per session (`Pane.triggers`, cleared on
disconnect like any session-scoped consent), remembers a refusal, and
shows the matched line plus the snippet body in the confirmation. The
send goes to ONE pane, never through the broadcast funnel.

### Card action icons (convention)

Per-row / per-card action icons (edit, paste, run, delete, the `⋮`
menu, etc.) are **always floating and hover-revealed**, never inline:

- Wrap the card/row in a `MouseArea` with
  `.on_enter(SomethingHovered(idx)).on_exit(SomethingUnhovered(idx))`
  and track the hovered index in `HoverState` (`state/hover.rs`).
- **The exit carries the same key the enter does, and the handler
  clears THROUGH `HoverState::leave_*`, never `= None`.** Crossing from
  one item to the next fires both events in the SAME frame, in the
  list's build order rather than the order the cursor visited them: a
  `Row` / `Column` updates its children by index, so moving RIGHT TO
  LEFT (or BOTTOM TO TOP) publishes the arriving item's `on_enter`
  FIRST and the departing item's `on_exit` second, and an
  unconditional clear wipes the hover it just gained. A gap between
  items only hides it, since any flick long enough to skip the gap in
  one frame delivers the pair back to back. This has shipped as a bug
  twice: the SFTP rows (a drag that armed "maybe one time in ten",
  `SftpRowExit`) and the tab strip (PR #133, the close button that
  never appeared). A new list without the guard is the third.
  `leave_*` is one line per field; add yours next to the others.
- Render the actions in a `Stack` overlay on top of the card (so they
  don't reserve inline width and the card content stays put), shown
  only when `hovered_* == Some(idx)`.
- Run/Paste-style actions that aren't self-evident get an
  `iced::widget::tooltip` (e.g. `snippet_run` = "Run (+ Enter)",
  `snippet_paste` = "Paste (no Enter)").

See `views/terminal.rs::snippet_row` and `views/snippets.rs` cards for
the reference implementation.

### System clipboard (hard rule)

**Never call `arboard` (or any clipboard API) directly. Ever.** The iced
runtime owns the clipboard and serves one access at a time on its own
worker thread; a second concurrent open in the same process is FATAL on
Windows. Field crash 2026-07-29: Ctrl+V in the SFTP path bar, where
`text_input` asked the runtime for a read while app code read `arboard`
inline on the UI thread. Both threads landed in
`user32!GetClipboardData(CF_UNICODETEXT)`, the Terminal-Services clipboard
monitor (`wtdccm.dll`) called `GlobalSize` on an `HGLOBAL` the other
thread had freed, and `ntdll` raised `STATUS_HEAP_CORRUPTION`: process
gone instantly, no unwinding, no panic hook, no log line. Neither
`oryxis-app` nor `oryxis-terminal` depends on `arboard` any more, and
their `Cargo.toml`s say why.

- App code: `dispatch_global::read_clipboard_text(to_message)` (a `Task`
  whose result comes back as a message) and
  `dispatch_global::write_clipboard_text(text)`. A read RESOLVES LATER, so
  capture any target (tab index, pane) when you request it, never
  re-resolve `active_tab` on delivery (see
  `dispatch_terminal::paste_text_into_tab`).
- `oryxis-terminal` (widget + backend, no `Message` of its own): queue via
  `host_clipboard::{write_text, read_text, paste_into}`; the host drains
  `take_clipboard_requests()` at the end of every `update()`. A gesture
  that queues a copy must produce a message, or the copy waits for the
  next one (that is why a right-click press maps to `NoOp` in
  `subscription.rs`).
- Tests: the harness emulates the clipboard, so `clipboard "text"` seeds
  it and `clipboard is "text"` asserts it, in `.ice` files too
  (`tests/e2e/clipboard-paths.ice` covers the three paths). App copies are
  visible to those asserts precisely because they go through the runtime.
- **PRIMARY is a second buffer, and only on X11 / Wayland.** Finishing a
  selection in the terminal publishes it there
  (`host_clipboard::write_primary_text`), and middle-click / Shift+Insert
  resolve against the SYSTEM primary first, the pane's own remembered
  selection second, the clipboard last
  (`TerminalPasteSelection` -> `TerminalPasteSelectionResolved`). Both
  halves gate on `oryxis_terminal::has_primary_selection()` and nothing
  else: off Linux the runtime serves a PRIMARY request from the ordinary
  clipboard, so an ungated write would wipe the user's Ctrl+C every time
  they highlighted a word. The ghost band still draws the pane's own last
  selection, which is a hint, not a promise: another window can own
  PRIMARY by then.

### Button feedback (convention)

Every clickable button / icon affordance must give visual feedback on
**hover** and on **press (click)**, always. No flat, feedback-less
controls, the user treats a button with no hover state as broken.

- Use iced `button` and branch its style closure on `button::Status`
  (`Hovered` / `Pressed`) to fill `bg_hover` (or an accent tint for a
  selected tab). Set the icon/text color explicitly (it doesn't reliably
  inherit from `button::Style.text_color`); let the **background** carry
  the feedback. References: `styled_button_opt` (`widgets.rs`),
  `sidebar_tab_btn` / `chat_header_btn` (`views/terminal.rs`).
- Icon-only controls also get a `tooltip` (`icon_tooltip` in
  `views/terminal.rs`) and a selected/active style where it applies.
- **Terminal-sidebar gotcha (resolved):** `button` once appeared to "eat
  clicks" in the terminal sidebar, the actual cause was the terminal
  canvas (`oryxis-terminal/src/widget.rs`) capturing *every* left-button
  release, including ones over sibling widgets, so a `button` (which
  fires on release) never saw its release. The canvas now only captures
  releases that finish a selection or land over itself; `button` works in
  the sidebar. If sidebar clicks ever die again, suspect a widget
  unconditionally capturing mouse releases before that check.

### Keyboard navigation / keynav (convention)

Oryxis has a unified keyboard-navigation framework (issue #52) and
**every new interactive surface must be wired into it in the same
change**, not deferred: a mouse-only view, modal, menu or row list is
an incomplete feature here, same rule as i18n keys. The code lives in
`keynav/` (`mod.rs` = types + pure movement math, `slots.rs` = the
modal/settings recording slots, `movement.rs`, `tests.rs`) with the
key routers in `dispatch_keynav.rs` / `dispatch_keynav_modal.rs` /
`dispatch_keynav_panel.rs`.

Two layers, two models:

- **Vault area (focus zones).** Tab / Shift+Tab cycle Search → sub-nav
  → toolbar → content; arrows move within a zone, Enter activates, Esc
  idles. Search is "zone zero" (`focus == None`) because iced can't
  report a text_input's focus. Views record their navigable items into
  `KeyNavState`'s RefCells **during `view()`** (render order, post
  filter/sort) using semantic ids, so a re-render can't strand the
  selection. Adding a vault view = record its items + extend the
  routers.
- **Modal / settings / side-panel layer (`RowAction` slots).** These
  surfaces record INDEX-based `RowAction`s per frame under a
  `ModalSurface` tag (stale tag = no selection, so a surface swap
  drops the selection for free). `RowAction::activate(msg)` for
  buttons/toggles/menu rows, `::picker(prev, next)` for pick_lists
  (Left/Right cycle), `::input(id)` for text inputs (Enter focuses).
  Enter also confirms destructive confirm dialogs.

Practical rules:

- Settings rows: use the `nav_toggle_row` / `nav_pick_row` helpers
  (`keynav/slots.rs`), never a raw toggler/pick_list row, they record
  the `RowAction` for free (see `views/settings/terminal.rs`).
- New modal / context menu / confirm / picker: record its rows under a
  new `ModalSurface` variant and let the modal router drive it.
- The terminal sidebar (ALL its tabs: Chat / Snippets / History /
  Files / Monitor / Tmux / Host config / Hosts tree) is a third mini
  layer (`SidebarRow` slots, which embed
  a `RowAction`, + `dispatch_keynav_sidebar.rs`): rows coexist with a
  live PTY, so the layer is opt-in. Since issue #102 the sidebar is
  TWO regions (left + right; each tab picks its side in Settings, a
  physical edge RTL never flips): row recordings, widths, drags and
  the open flags are all per-region, `sidebar_regions.rs` is the
  single authority for what a region offers/shows, and the keyboard
  engages ONE region at a time (the ring's region, else the cursor's;
  the ring's tab names its region via its dock side). Scroll ids are
  per TAB (`keynav::sidebar_scroll_id`), because both regions can
  mount a list in the same frame. Entry points: the
  `FocusSidebarList` hotkey (Ctrl+Shift+H, cycles every available tab
  across both regions, left first, opening the target's region;
  landing focuses Chat's editor / History's + Hosts' search, rings the
  first row elsewhere), Up/Down with the cursor over a LIST tab, or
  Tab/Shift+Tab with the cursor over the sidebar (a plain Tab over
  the terminal must stay a literal `\t`, that's why the walk is
  cursor/ring gated). Tab walks every recorded row with the panel
  contract (inputs get real iced focus, the rest ring); Up/Down hop
  over inputs. On list rows Enter = RUN (owner call 2026-07-03:
  there's no keyboard path back to the terminal's Enter),
  Shift+Enter = paste without newline, Delete = remove THROUGH ITS
  CONFIRM (an unconfirmed hover-trash click once silently wiped a
  host's whole history), Esc disengages AND blurs any sidebar input
  (the terminal never holds iced focus, so blurring IS "focus the
  terminal"), and Ctrl+F under the same ownership gate opens/focuses
  the active tab's search (Snippets/History; others decline). The
  ring also DISENGAGES on
  any terminal interaction (typing that reaches the PTY, clicking a
  pane, switching terminal tabs, closing the sidebar); a lingering
  ring silently eats Enter, that was a real QA bug. Ring-originated
  injections write through `write_ring_injection_to_tab` so their own
  Enter doesn't drop the ring. The apply-sudo row IS recorded (owner
  overrode the earlier exclusion 2026-07-03: Tab+Enter is the same
  intent bar as a click). New sidebar controls = record via
  `sidebar_nav_slot` in DISPLAY order (build order == record order;
  the Host config view builds its theme cards last for exactly this
  reason, and `view_terminal_sidebar` builds ONLY the active tab's
  body so inactive tabs can't record) + lists share the
  "sidebar-list-scroll" scrollable id.
- Password fields use `password_input_with_eye_nav` on navigable
  surfaces: record the FIELD's `RowAction::input` row BEFORE building
  the widget (`panel_nav_record` / `settings_nav_record` +
  `*_nav_ring_at` / `modal_nav_record`), then pass a `wrap_eye`
  closure that records the eye as the next slot
  (`RowAction::activate(toggle_msg)`). The eye is a stop of the walk
  right after its field: Tab/arrows reach it, Enter/Space toggle.
- Known not-yet-covered surfaces: the Security set/change-master-
  password forms (deliberate: they keep iced's native Tab between
  fields and the keyboard router is disabled while open, so their
  eyes are mouse-only; revisit when D1 migrates those forms), the
  vault lock screen and the onboarding password step (own screens,
  outside the framework). If you touch one of these, wiring keynav
  in is part of the job.

### Mouse buttons are bindings, not settings (convention)

`hotkeys.rs` binds MOUSE buttons alongside chords:
`PrimaryKey::Mouse(MouseButton)` (Middle / Back / Forward /
`Other(n)`), modifiers included, serialized as `mouse_middle` &c. so
the `mouse_` prefix can never collide with a named key or a punct
token. Left and Right are NOT in the set: they are the canvas's own
select / right-click-scheme gestures.

- **Side buttons are free window-wide, the wheel click is not.** No
  WIDGET reacts to Back / Forward / `Other(n)` (iced's `button` /
  `scrollable` / `text_input` act on the primary; the canvas claims
  primary / secondary / middle), so a side button binds to ANY action
  and fires anywhere. The middle click stays `terminal_only`: the
  canvas spends it on mouse reports and the X11 paste, and a middle
  click over a list is a gesture users expect elsewhere.
  `accepts_mouse()` (= `primary_editable`) drives the chip placeholder;
  `accepts_mouse_button(button)` is the per-button gate the capture
  enforces.
- **Back / Forward yield to a visible file surface.**
  `file_surface_nav` runs FIRST in `handle_mouse_button_press`, so
  SFTP (standalone tab or hybrid Files mode) and the sidebar Files tab
  walk their directory history with the thumb pair, and a user binding
  gets those buttons on every other screen. The pair is genuinely
  contested (X11 maps buttons 8/9 to them, Wayland `BTN_SIDE` /
  `BTN_EXTRA`, Windows `XBUTTON1` / `XBUTTON2`, so they ARE the thumb
  buttons of an ordinary five-button mouse, not exotic extras), which
  is why the answer is a context yield rather than reserving them.
  Same shape as a bare Ctrl+letter binding yielding to the PTY.
  A visible file surface CONSUMES the press even with nowhere to go
  (`Some(Task::none())`), so the rule stays "which surface is up",
  never "how deep its history is".
- **One owner per (action, button).** `mouse_binding_owner()` is the
  single authority, called by BOTH layers so they can't drift: a pair
  claimed twice fires twice, a pair claimed by neither is a dead
  button. Widget = the five `widget_dispatched` gestures (they need
  canvas state) plus every middle-click binding; App = side buttons on
  everything else, dispatched from `shortcuts::dispatch_mouse_binding`
  with the keyboard router's exact view gates.
- The widget gets a matcher, never a table:
  `views/terminal.rs::terminal_mouse_resolver` -> `MouseResolver`,
  returning `MouseGesture::Widget(TerminalChordAction)` or
  `MouseGesture::Publish(msg)` (`RunHotkeyAction`). Same contract as
  `ChordResolver`: ONE implementation of binding matching. Declining
  leaves the press uncaptured, which is what hands it to the app.
- The mouse arm in `widget/events.rs` sits AFTER the mouse-report
  path on purpose, so a binding can't steal a button from a TUI that
  holds mouse tracking. Don't move it.
- Presses arrive through the global subscription as
  `SettingsMessage::MouseButtonPressed` (unconditional: the closure is
  built once and outlives any captured flag, so gating it on app state
  would go stale). `handle_mouse_button_press` then either RECORDS
  (a Shortcuts capture is armed, and it re-proves the editor is the
  visible surface) or FIRES. Conflict resolution is shared with the
  keyboard path (`commit_captured_binding`).
- **Middle-click paste is a chord, not a setting.** It is
  `TerminalPasteSelection`'s second factory input; Settings > Terminal's
  toggle adds / removes that one chord (`middle_click_pastes()` /
  `set_middle_click_paste()`), so the two surfaces can't disagree. The
  old `middle_click_paste` setting is migrated once at boot
  (`middle_click_paste_migrated`), applied to whatever list resolved so
  a user who had rebound paste-selection doesn't lose the gesture.

### Deep links (`oryxis://`)

The OS-registered URL scheme (module `deep_link.rs`; registration in
both `.desktop` files via `MimeType=x-scheme-handler/oryxis` + `%u`,
both NSIS scripts via `Software\Classes\oryxis`, and the MSIX
manifest's `windows.protocol`). Routes today: `oryxis://pair/<id>/<code>`
(prefills Settings > Sync join) and `oryxis://theme/<base64url JSON>`
(opens the matching theme-import panel prefilled; the
`oryxis_ui_theme` marker picks terminal vs UI). Rules:

- **Parse is strict and size-capped** (`deep_link::parse`), and every
  route lands on an existing confirm surface with the payload
  prefilled. Never auto-execute from a link: any web page can launch
  these URLs.
- **Delivery**: cold start stashes argv in `app::PENDING_DEEP_LINK`
  (drained like `--connect`, incl. post-unlock via
  `pending_deep_link`); with a running instance the launcher process
  drops the URL in `~/.oryxis/runtime/deeplink/` (`tray_ipc`), waits
  ~2 s for a claim and exits, else boots with the link itself. Claims
  are rename-based so N windows consume each link exactly once; the
  `deep_link_stream` subscription (all platforms) yields only on
  arrival, so the idle inbox never re-renders the app.
- **Every window registers a PID file** (`tray_ipc::Child::register`,
  all platforms since deep links) and Unix liveness is `kill(pid, 0)`;
  that registry is how the launcher detects a live instance.
- **macOS is NOT wired**: LaunchServices delivers URLs as Apple Events
  (`kAEGetURL`), not argv, and the handler's interaction with winit's
  NSApplication delegate is unverified without hardware. The scheme is
  deliberately absent from Info.plist until both land together.
- New routes: add a variant + arm in `deep_link.rs` (parse + route +
  tests), nothing else; the transport is route-agnostic. `ssh://`
  quick-connect and the future `oryxis://share` (team vaults) ride
  this rail.

### Hybrid tab + sidebar Files (issue #61)

Every SSH terminal tab has two file-browsing surfaces, both
multiplexed over the tab's live `client::Handle` via
`session.open_sftp()`:

- **Sidebar Files tab** (`views/sidebar_files.rs` +
  `dispatch_sidebar_files.rs`): per-PANE state in `Pane.files`
  (`PaneFiles`), lazily mounted, follows the shell's OSC 7 cwd
  (`pane.cwd`; manual navigation unpins, the pin re-enables). Every
  entry point calls the idempotent `sidebar_files_sync()`. Reset on
  disconnect (`reset_for_disconnect`, preferences survive). The ⛶
  action promotes to a full SFTP surface at the current directory via
  the one-shot `Oryxis.sftp_open_at_path` hint, consumed by
  `dispatch_sftp::initial_remote_listing` (home fallback).
- **Files mode** (`TerminalTab.files_mode`): the whole tab content
  becomes the dual-pane SFTP surface. Its state
  (`TerminalTab.files_state: Box<SftpState>`) rides the standalone
  SFTP tabs' swap-on-focus invariant: hoisted into the live
  `Oryxis::sftp` buffer while shown, tracked by
  `Oryxis.hybrid_sftp_owner` (mutually exclusive with `active_sftp`;
  `park_hybrid_sftp` / `hoist_hybrid_sftp` in `sftp_methods.rs`).
  `route_sftp_async` + `current_sftp_owner` route transfer
  continuations to hybrid owners too. Switch affordances: mode glyph
  chip on the tab, status-bar segment (redundant by design, the bar
  is optional), tab context menu entry, `ToggleTabFiles` hotkey
  (Ctrl+Shift+F). While Files mode is up the PTY keeps running but
  its byte routing is gated off (`write_ring_injection_to_tab`), and
  anything that used to gate on `active_view == View::Sftp`
  (type-ahead, row drags, OS drops) gates on
  `sftp_surface_visible()` instead. Toggling a tab without a live
  SSH session only ever toggles OFF (the way back never disappears).

Standalone SFTP tabs are unchanged and remain the server-to-server
(two different hosts) surface.

### SSH agent server (issue #54, shipped)

Oryxis serves the standard ssh-agent protocol so external tools
(`git`, VS Code, WSL) authenticate with vault keys, and (opt-in)
accepts keys pushed in by tools like KeePassXC. Module:
`crates/oryxis-app/src/agent_server/` (`protocol.rs` wire framing,
`source.rs` key sources, `listener.rs` transports, `mod.rs`
`AgentRuntime`, app glue in `dispatch_agent.rs` / `state/agent.rs`).

- We own the (frozen, draft-miller) wire protocol. russh's
  `agent::server::serve` was rejected: its `Agent` trait has no
  identity-supply hook, so backing it with the vault would hold every
  decrypted key in memory for the whole unlocked window. russh's
  `AgentClient` is the protocol-test oracle, with two known client
  bugs (russh 0.61): `remove_all_identities` sends a zero-length
  frame and `remove_identity` never checks the response, so those two
  paths are asserted at the message layer (`respond_sync`) instead.
- Vault keys are read-only over the wire and decrypted per signature
  (`VaultKeySource`, a dedicated `VaultStore` handle like sync's).
  Per-key `expose_via_agent` flag filters the roster. REMOVE of a
  vault blob answers FAILURE.
- `agent_server_allow_add` (default off) accepts external ADD/REMOVE
  into the in-memory `EphemeralStore`: never persisted, swept on
  vault lock / toggle-off / exit (locked also refuses adds), lifetime
  and confirm constraints honored (lifetime 0 = no deadline, OpenSSH
  semantics; an unrecognized constraint refuses the whole add), and a
  re-add of the same public blob replaces the entry.
- Confirm: the UI channel always exists. `agent_server_confirm`
  prompts on every signature; a CONFIRM-constrained added key prompts
  even when that global is off, and a constrained add with no UI to
  ask is refused at add time. "Always allow" grants are per
  fingerprint per session, swept on lock and toggle-off.
- Transports: unix socket `~/.oryxis/agent.sock` (0600, liveness
  probe before unlinking a stale file). Windows named pipe
  `\\.\pipe\oryxis-ssh-agent` with a per-user DACL
  `D:P(A;;GA;;;<sid>)` (GA is required so a second concurrent
  instance can be created; do not narrow it) and
  `first_pipe_instance(true)` anti-squat. The opt-in
  `agent_server_openssh_pipe` additionally serves
  `\\.\pipe\openssh-ssh-agent` when the name is free, so tools with a
  hardcoded agent target (KeePassXC in OpenSSH mode, stock `ssh.exe`)
  need zero config; a busy name (the real agent service) surfaces as
  a non-fatal inline `alias_error`.
- RELEASE GATE (Windows): no artifact ships the agent until the DACL
  acceptance test passes on a real machine, a SECOND local user
  connecting to the pipe must be DENIED (a Linux cross-check cannot
  prove the DACL).

## Settings table

Live in the SQLite `settings` table — accessed via
`vault.get_setting("key")` / `vault.set_setting("key", value)`. Values
are `String`. Booleans use `"true"` / `"false"`. The vault opens
without unlocking for settings reads, so the lock screen can hydrate
theme + language before the master password is entered.

Boot logic in `boot.rs::load_data_from_vault` reads settings into
`Oryxis` state once. Mutations go through dispatch handlers that both
update in-memory state and persist via `set_setting`.

Notable settings:

- `sync_enabled`, `sync_mode`, `sync_passwords`, `sync_device_name`,
  `sync_signaling_url`, `sync_relay_url`, `sync_listen_port`
- `mcp_server_enabled`, `mcp_server_port`
- `language`, `app_theme`, `terminal_theme`
- `download_mirror` (`"auto"` / `"github"` / an https base URL): the
  China-mirror routing for every GitHub-bound download (CJK fonts,
  plugin manifests + binaries, update check + installer). Module
  `net_mirror.rs`, applied ONLY to the four GitHub hosts. `Auto`
  (default) = GitHub first, per-request fallback to the project ASSET
  HOST `dl-cn.oryxis.app` (Tencent EdgeOne with China ISP peering, in
  front of `dl.oryxis.app` = Cloudflare R2 bucket `oryxis-dl`; owner
  decision 2026-07-11: GitHub stays primary so the mirror only carries
  traffic that needs it). The asset host is a static layout
  (`fonts/<file>`, `releases/<tag>/<asset>`, plus
  `releases/{latest,nightly,index}.json` snapshots of the GitHub API
  responses), populated by `publish-mirror.yml` which every release
  workflow calls after publishing (kept in lockstep with
  `net_mirror::asset_path`; seed old releases via workflow_dispatch).
  Custom = the user's prefix proxy (`<base>/<full-url>`) first, direct
  fallback. Mirrors are untrusted by design: sha256 pins fonts,
  sha256+Ed25519 pin plugins and updates; a hostile mirror can only
  withhold or replay release METADATA (stale-version pin), never
  execute unsigned code. Settings > Advanced has the picker + custom
  URL + reachability Test. Workflows need the `R2_ACCESS_KEY_ID` /
  `R2_SECRET_ACCESS_KEY` repo secrets (R2 API token scoped to the
  bucket).
- `auto_lock_minutes` (vault idle auto-lock, "0" = off; idle anchor is
  `Oryxis.last_user_activity`, reset in `dispatch.rs::update` from the
  global input events; fires `AutoLockVault`, a SOFT lock that zeroizes
  the key and shows the lock screen but keeps live SSH sessions + tabs,
  unlike the manual `LockVault` teardown; the session-log flush and
  auto-reconnect subscriptions unmount while locked). The
  `clipboard_clear_seconds` credential-wipe setting was REMOVED with
  its only trigger (the card menu's "Copy password", replaced by "Copy
  SSH URL"); a stale row may linger in old vaults, harmless. Re-add the
  timer machinery together with any future credential-copy action.
- `ai_provider`, `ai_model`, `ai_api_key` (the API key is encrypted
  per-field inside the value via `set_user_password` machinery)

### MCP as a plugin

`oryxis-mcp` no longer ships inside the OS installers (no `.deb`
asset, no NSIS `File` line, no tarball / AppImage copy). It's
distributed via the existing plugin pipeline: `mcp-v*` release tags
build the binary across 5 platforms, sign with the Ed25519 key, and
publish a `mcp.json` manifest alongside. The app downloads on first
enable of "MCP Server" in Settings, or auto-installs on boot for v0.6
users who already had the toggle on.

The distinction from cloud plugins (`oryxis-cloud-aws-plugin`): MCP
is **not** spawned by the app, external clients (Claude Desktop,
Claude Code, Cursor) spawn it. So the `PluginHost` / `PluginProvider`
machinery doesn't apply, only the distribution half
(`plugins::cache`, `plugins::download`, `plugins::verify`,
`plugins::manifest`). `crates/oryxis-app/src/mcp_install.rs` glues
those to a stable launcher copy at `~/.oryxis/bin/oryxis-mcp[.exe]`
that external clients can hardcode in their config without breaking
across plugin updates. Windows .exe-in-use during update is handled
by renaming the live binary to `oryxis-mcp.old.exe` and sweeping it
on the next boot.

The `protocol_versions: [1]` field in `mcp.json` is a no-op
placeholder, MCP's own JSON-RPC contract is entirely separate from
`oryxis-plugin-protocol`, but the manifest filter requires a
non-empty intersection so we declare `[1]` to satisfy it.

## When adding a new model entity

1. Add the type to `oryxis-core/src/models/<name>.rs` and re-export
   from `models.rs`.
2. Add a SQLite table to `store/schema.rs::create_tables`
   (`CREATE TABLE IF NOT EXISTS <name>s`).
3. Add CRUD methods in a new (or existing) `oryxis-vault/src/store/<name>.rs`
   module: `save_*`, `list_*`, `delete_*`, plus a password getter / setter
   if any field is encrypted. Declare it as `mod <name>;` in `store/mod.rs`.
4. If sync should cover it: add `EntityType::<Name>` to
   `oryxis-sync/src/protocol.rs`, plus arms in
   `engine::build_manifest`, `collect_records`, `apply_records`. If
   it has a password, add a `Sync<Name>` wrapper next to the existing
   ones and respect the `sync_passwords` setting.
5. If portable export should cover it: add `Export<Name>` to
   `portable.rs`, include in `ExportPayload`, populate during export,
   apply during import.
6. UI: dispatcher (`dispatch_<area>.rs`), view, messages enum, app
   state fields, boot defaults, i18n keys × all 23 languages.

## Roadmap notes (não-implementado)

Standalone port forwarding (`-L`/`-R`/`-D` as `PortForwardRule`
entities) shipped in v0.8.0. The remaining roadmap is below; the
authoritative scope lives in the README roadmap table, this section
holds implementation pointers only.

### v0.9: planned

GCP (Compute Engine + GKE) and Azure (VMs + AKS) cloud providers
(subprocess plugins like AWS/K8s); **RDP/VNC over SSH in one click**
(launcher on top of a `-L` `PortForwardRule` that spawns the
OS-native client: `mstsc` / FreeRDP / Remmina / Microsoft Remote
Desktop); **command history** (resurrect the deferred terminal-sidebar
History tab next to Snippets; removed as a placeholder in `c0e8d13`,
zero code today). Agreed design: per-host capture (hybrid OSC prompt
markers + raw input heuristic), top-3 most-frequent + a recent list,
click-to-re-insert like a snippet, new `command_history` vault table
(separate from `session_logs`). Pairs with the local-text-file ask
from the GitHub logging request: add an optional export / live-append
of executed commands to a plain `.txt` for offline reference and
support sharing (reuse `get_session_data` + `strip_ansi`); biometric
unlock (local app unlock, *not* SSH auth); Windows ConPTY local shell;
Windows JumpList; XChaCha20-Poly1305 wire format (192-bit nonce) on a
sync v6 bump.

**Telnet protocol.** A per-host protocol selector, not a per-host stack
of protocols (Termius lets a host carry SSH + Telnet blocks at once;
the whole `Connection` model is single-endpoint, so we add one field
instead of a `Vec`). New `protocol: ConnectionProtocol` enum on
`Connection` (`#[serde(default)]` -> `Ssh`; add a legacy-payload test
mirroring `keepalive_interval_legacy_payload_defaults_to_none`). The
Telnet password reuses the existing encrypted connection-password
column, no migration, rides sync + portable export for free.
New `oryxis-telnet` crate mirroring `oryxis-ssh`'s session shape: a
`TelnetSession` exposing the same surface the terminal pane consumes
(`write` / `resize` / `is_alive` / `close` plus an
`UnboundedReceiver<Vec<u8>>`). Protocol: raw TCP + RFC 854/855 IAC
option negotiation (answer WILL/WONT/DO/DONT, ECHO, SGA), RFC 1073
NAWS for resize, RFC 1091 TERMINAL-TYPE = `xterm-256color`; strip /
respond to IAC inline before forwarding bytes to alacritty. Unit-test
the negotiation state machine. Integration boundary: the terminal pane
session (`state.rs` `session: Option<Arc<SshSession>>` and the
`Connected` / `SshConnected` messages) is the only `SshSession` user
that must generalize, the SFTP path (`ssh_session`,
`SftpHostMounted`, `open_sftp`) stays SSH-only. Blast radius is 11
refs across 4 files (`state.rs`, `dispatch_ssh.rs`, `dispatch_sftp.rs`,
`messages.rs`); prefer an enum (`TerminalTransport { Ssh(Arc<SshSession>),
Telnet(Arc<TelnetSession>) }`) over `Arc<dyn _>` since only the pane
path branches. Host editor swaps to a reduced form when
`protocol = Telnet`: username / password / encoding / terminal theme
only; hide keys / identities / agent-forwarding / jump-chain / proxy /
SFTP / port-forwards / OS-detect / MCP / AI-exec, default port to 23,
show a one-line cleartext-credentials note (honest UX, not a security
lecture, the user is the only one on the path that lacks a secure
option). New i18n keys ("Telnet", the protocol picker, the cleartext
note) across all 23 language modules.

**TOTP 2FA in the vault (in flight).** Per-connection TOTP secret in a
new encrypted `totp_secret` BLOB column on `connections` (ALTER TABLE
migration in `schema.rs`), tri-state API mirroring passwords
(None preserve / `Some("")` clear / `Some(s)` encrypt+store). The host
editor field accepts either a bare base32 secret or a full
`otpauth://` URI; store the input verbatim, parse digits / period /
algorithm (SHA-1/256/512) at generation time, RFC 6238 with RFC test
vectors. Autofill: when a keyboard-interactive prompt arrives and the
connection has a secret and the prompt text matches OTP keywords
(verification code / OTP / token / one-time), answer automatically,
once per auth attempt; a second OTP prompt in the same attempt falls
back to the manual modal so a rejected code can't loop. Rides sync via
a `#[serde(default)]` field on `SyncConnection` gated by
`sync_passwords`, rides portable export like passwords, and gets a
structural leak test (`totp_secret` must never appear in plaintext
columns).

**Vault auto-lock + clipboard clear (in flight).**
`auto_lock_minutes` setting (0 = off): any user input event resets the
idle clock; expiry fires a SOFT lock (`AutoLockVault`): master key
zeroized + lock screen shown, but live SSH sessions and tabs survive
and are back after unlock (established channels never need the key;
credentials are only read at connect). The manual Lock Vault button
remains a full teardown. Secret-bearing UI (editor form, revealed
secrets, pending KBI prompt) is swept on soft lock.
The clipboard-clear half was REMOVED 2026-07-03: its only trigger was
the card menu's "Copy password", which became "Copy SSH URL"
(`CopyHostSshUrl` -> `host_ssh_url()`); the timed-wipe pattern
(generation counter + compare-before-clear) lives in git history at
`dispatch_history.rs` if a credential-copy action ever returns.

**Snippet variables (planned, small).** `{name}` / `{name:default}`
placeholders in snippet bodies, parsed at run/paste time; a small
modal prompts for values before send (reuse the careful-paste modal
shell). Touch points: the snippet run/paste paths in
`views/terminal.rs::snippet_row` and `dispatch_terminal.rs`. Plain
text on disk, so sync/export ride free. No major competitor has this
(Termius docs say unsupported).

**Paste guard extension (planned, small).** The careful-paste check
(`paste_text_into_active`) gains content heuristics beyond
multi-line: Unicode homograph/bidi characters, `curl|sh`-style
pipe-to-shell patterns, hidden control sequences. Same modal, extra
warning line. Mirrors MobaXterm's malicious-paste detection.

**PuTTY parity pack (planned, small).** From the 2026-07 PuTTY
config-panel audit (full notes in `COMPETITOR_PARITY.md`, local):
middle-click paste (the widget only uses Middle for mouse reports
today; add an X11-style paste, plus a setting for the right-click
action: context menu / paste / extend, PuTTY's three schemes);
SSH pre-auth banner (implement russh's `auth_banner` handler callback
and surface it in the connection progress / terminal; today banners
with legal notices or MFA instructions are silently dropped);
TCP_NODELAY on the session socket (PuTTY defaults it on; check where
the engine dials and whether russh exposes the stream);
per-host IPv4/IPv6 preference (Auto/4/6, filter resolved addrs).
Known limitations recorded by the same audit: GSSAPI/Kerberos needs
russh support (watch upstream); check whether russh exposes ML-KEM /
PQ hybrid kex and list it in the algorithm overrides if so; CJK
ambiguous-width and bidi/Arabic shaping are alacritty_terminal
limitations, not config gaps.

**Session recording / asciinema export (planned).** Not a new
subsystem: the encrypted session logs ARE the recording, `.cast` and
transcript are export formats added to the existing session-logs
screen. Storage: `session_log_chunks` gains `offset_ms INTEGER`
(milliseconds since `started_at`, stamped at capture time, ALTER TABLE
migration in `schema.rs`) and `kind TEXT NOT NULL DEFAULT 'o'`;
resize events are recorded as `kind='r'` rows whose data is
`"<cols>x<rows>"`. The capture point is the existing session-log
flush path, which just starts stamping; no parallel recorder.
Export .cast = asciicast v3 (asciinema CLI 3.0, the current spec;
shipped as v2 originally, bumped 2026-07-08): JSON header line with
`version: 3` and a required `term` object (`cols` / `rows` +
`type` from the connection's terminal_type + `theme`), then
`[interval, "o"|"r", data]` event lines where the interval is the
time since the PREVIOUS event (v3 semantics; stored `offset_ms` are
integer ms, so deltas sum exactly, no rounding drift; the
non-decreasing clamp guarantees intervals >= 0). `term.theme`
embeds the effective terminal theme (`fg` / `bg` + 16-color
`:`-separated `palette`, resolved per-host override -> global like
the live pane, via `color_to_hex` in `theme.rs`): agg and the
asciinema player auto-apply it, so the v1.0 GIF export inherits
correct colors with no extra plumbing. v3 also allows `m` marker /
`x` exit events and `#` comments (not emitted today; markers pair
with the OSC 133 work). Output-only by design: `"i"` input
events are deliberately never recorded (echo-off passwords never hit
output anyway, and omitting input removes the keystroke-leak class
entirely). Pre-migration chunks have NULL offsets; export those with
a small fixed delta so old logs still replay, just without real
timing. Transcript export (.txt) = `strip_ansi` (the
`dispatch_history.rs` helper) over the concatenated `'o'` chunks;
once the v0.9 command-history OSC 133 prompt markers land, segment
the transcript per command instead of one flat dump (same pipeline,
planned together). UI: Export `.cast` / Export transcript actions on
the session-logs screen + a per-tab record affordance; i18n keys
across all 23 languages. Privacy Mode caveat: masking is
render-only, recordings carry raw bytes; say so in the export
tooltip/confirm. Scope decisions (owner, 2026-07-03): upload to
asciinema servers is permanently OUT of scope; GIF export is
deferred to v1.0+ as an optional plugin, pending its own analysis
(see the v1.0 list).

### v1.0: planned (stable)

Everything queued for the stable release:

- **Advanced auth.** SSH certificate auth (signed user certs);
  FIDO2 / security-key keys (`sk-ssh-ed25519`, `sk-ecdsa-sk`);
  PKCS#11 / smartcard / YubiKey. The engine today does
  password / publickey / keyboard-interactive / agent / auto
  (`engine.rs` auth path); these are additive auth methods.
- **In-app key generation UI.** Crypto already exists in
  `oryxis-vault/src/keygen` (only `generate_ed25519` is exposed and
  there is no UI). Expose a Generate flow for Ed25519 / RSA / ECDSA
  with optional passphrase, alongside the existing Import flow in
  `dispatch_keys.rs`.
- **Terminal.** Scrollback search (Ctrl+F find-in-buffer with match
  highlight, currently `FocusViewSearch` skips the terminal);
  broadcast / synchronized input across split panes; zmodem / lrzsz
  in-terminal transfer; OSC 8 hyperlinks (regex URL scan exists,
  escape-sequence links don't); ad-hoc quick-connect (parse
  `user@host` in the new-tab picker without saving); command palette.
- **GIF export of session recordings (analysis DONE 2026-07-08).**
  Renders a v0.9 `.cast` recording into a shareable GIF. Decision:
  optional plugin via the existing distribution pipeline
  (`plugins::cache` / `download` / `verify` / `manifest`, the
  `oryxis-mcp` precedent), downloaded on first use. Analysis findings
  (agg 1.9.0): agg is now a proper library, one call,
  `agg::run(input: BufRead, output: Write, Config)`, so the plugin is
  a thin wrapper crate (~200-300 lines: read `.cast`, build `Config`
  with `show_progress_bar: false`, run) plus a `gif-v*` release
  workflow cloned from mcp/gcp and an "Export GIF" action on the
  History screen that ensures the plugin is installed and spawns it.
  Corrections to the earlier note: license is GPL-3.0-or-later, NOT
  Apache-2.0 (fine for the AGPL app, same GPLv3 s13 rationale as
  mosh; the subprocess boundary keeps extra distance anyway), and the
  renderer is swash/resvg + fontdb, not fontdue. Font bundling is
  solved upstream: agg embeds Noto Emoji + Nerd Font Symbols and
  resolves the monospace face from system fonts via fontdb
  (JetBrains Mono / Consolas / Menlo / DejaVu fallback chain covers
  all 3 OSes; `font_dirs` is the escape hatch). Binary weight: agg's
  standalone release binaries run 13-16 MB (swash + resvg +
  tiny-skia + gifski + embedded fonts), which confirms plugin over
  in-core. Theme mapping: `Theme::Custom` takes 18 comma-separated
  hex values (bg, fg, 16 ANSI), direct 1:1 from the terminal themes;
  better, agg auto-uses a theme embedded in the cast header
  (`term.theme`), and the exporter already emits asciicast v3 with
  the theme embedded (bumped 2026-07-08, see the v0.9 recording
  section), so the plugin needs zero theme plumbing and third-party
  agg/player users get correct colors. Pin agg by tag. Caveats:
  Privacy Mode raw-bytes warning applies to GIF like `.cast`.
  Upload to asciinema servers stays out of scope permanently (owner
  decision 2026-07-03).
- **In-app session player.** Play back a recorded session inside the
  app, on the session-logs screen next to the `.cast` / transcript /
  GIF exports. Zero new dependencies: `oryxis-terminal`'s `Backend` is
  already transport-agnostic (`Backend::new(cols, rows)` +
  `process(&[u8])` + `resize()`, `backend.rs`), so the player is a
  playback clock (iced tick subscription) feeding `session_log_chunks`
  rows by `offset_ms`: `kind='o'` chunks go to `process()`, `kind='r'`
  to `resize()`; render with the existing widget + themes. Read-only
  for free: just never call `set_pty_write_tx`. Seek = recreate the
  `Backend` and replay from zero up to the target time (`process()` is
  fast enough; keyframes only if very long sessions ever hurt). Speed
  control = scale the clock before comparing against `offset_ms`.
  Pre-migration chunks with NULL `offset_ms` use the same fixed-delta
  fallback as the `.cast` export. Once OSC 133 markers are in the
  recordings the backend's mark sniffer segments the timeline per
  command (jump-to-command on the scrubber). `avt` REJECTED for this
  (owner decision 2026-07-08): it would be a second terminal emulator
  in the binary with subtly different rendering than the live session;
  avt stays relevant only inside the GIF plugin (via `agg`). Player
  controls get keynav + i18n keys x23 like any new surface.
- **AI ops toolkit (GloriaOps-class, local-first).** Full spec in
  `AI_OPS_AGENT_SPEC.md` (repo root, local). The per-tab assistant
  stops composing raw shell and calls typed operations from a new
  `oryxis-agent` crate (schema + `Access::ReadOnly/Write` + per-OS
  command synthesis + mandatory dry-run `preview` on every write
  tool), executed on a dedicated exec channel multiplexed on the
  session's `client::Handle` (the SFTP / `mcp/handlers.rs` pattern),
  never typed into the user's PTY. Risk becomes structural, so the
  auto-exec judge remains only on the legacy `execute_command` PTY
  tool (which survives as an always-gated escape hatch). On top:
  capability-declared approvals (write ops surface a plain-language
  card with the rendered preview, reusing the careful-paste modal
  shell; ALWAYS RUN evolves to per-(tool, host) grants), reviewable
  multi-step change plans in Plan mode, an `agent_runs` audit table,
  and a `HostFacts` entity (per-host OS / init / package manager /
  runtime facts from a read-only probe pack, persisted in a new
  vault table, injected as context and driving command synthesis;
  not synced, not exported). Secrets invariant made structural: a
  test asserts no credential material ever serializes into a
  model-bound request. Background autonomy / alert ingestion is NOT
  planned (owner decision 2026-07-03): it needs an always-running
  process + unlocked vault without UI, same parked problem as the
  ssh-agent note. Strategic contrast with Termius GloriaOps: same
  agent class, but local-first, BYO-key, no hosted backend.
- **Consistency.** Unified form system. The proxy-identity form
  (`views/settings.rs`) is the de-facto standard (uses `panel_field`
  + a `styled_button` Cancel/Save pair + inline error). Add the two
  missing shared helpers (a form footer and an inline-error slot) and
  migrate the hand-rolled editors (identity, snippet, key,
  port-forward, cloud, theme modals) onto them. This is adoption of
  existing helpers, not a redesign.
- **Stability tests.** `oryxis-vault/src/portable.rs` (export/import,
  ~630 lines) has zero tests; add a roundtrip in the spirit of the
  `proxy_password_does_not_leak_into_proxy_column` invariant tests
  (export encrypts secrets, import restores every field).
- **Quick-connect surfaces.** Besides the in-app `user@host` picker:
  register Oryxis as the OS `ssh://` URL handler (mirror the existing
  `oryxis://pair` registration) and accept `oryxis user@host` on the
  CLI (parse in `main.rs` before iced boots, reuse the single-instance
  IPC if one exists so a second invocation lands a tab in the running
  window).
- **Group settings inheritance.** Groups carry per-parameter optional
  defaults (identity/credentials, proxy, env vars, terminal theme,
  startup snippet, port; every field `None` = inherit). Resolution
  walks host -> parent chain -> app defaults at connect AND in the
  editor (show inherited values greyed with an "inherited from
  <group>" hint, override on edit; mirror the `customized_fields`
  pattern from cloud imports). Storage: new nullable columns / JSON on
  `groups` (migration in `schema.rs`); rides sync + portable export on
  the existing Group entity. Termius semantics reference:
  per-parameter (not all-or-nothing), nested merge up the chain.
- **Config importers.** PuTTY (registry `HKCU\Software\SimonTatham`),
  WinSCP (registry / portable ini), mRemoteNG (confCons.xml),
  Termius (its export format). Same second-pass alias resolution
  pattern as `ssh_config.rs::link_proxy_jumps`; unresolved bits land
  in `Connection.notes`, never fail the import. Plus generic CSV
  import/export of hosts (Termora audit 2026-07-24): header-mapped
  columns, optional password column on import (straight into the
  encrypted columns); CSV export omits secrets by design, portable
  export stays the only secrets-bearing path.
- **Wake-on-LAN.** Per-host optional MAC field; card action sends the
  magic packet (UDP 9 broadcast). Trivial, no crate needed.
- **Monitor tab GPU gauges (Termora audit).** Extend the agentless
  Monitor sidebar (#83) with an `nvidia-smi --query-gpu
  --format=csv,noheader` probe on the same exec channel; render the
  GPU section only when the probe answers. AMD via
  `/sys/class/drm/*/device/gpu_busy_percent` can ride along if
  trivial. Feeds the multi-host dashboard row too.
- **Network tools panel (optional, from the Marix audit).** DNS record
  lookups (A/AAAA/MX/TXT/SPF/CNAME/NS/SOA/PTR), ping + traceroute, TCP
  port test, HTTP/HTTPS check with certificate info, SMTP test, WHOIS,
  RBL blacklist check. Pure client-side Rust (hickory-resolver for
  DNS; ICMP needs raw-socket privileges on some platforms, so shell
  out to the system `ping`/`traceroute` where unprivileged sockets
  aren't available). Toggle-hidden per the optional-features rule:
  one `network_tools_enabled` setting, off by default, ALL its UI
  hidden when off.
- **SFTP archive operations (from the Marix audit).** Compress /
  extract zip and tar.gz. Remote side runs `tar`/`unzip` on an exec
  channel multiplexed on the live handle (the `mcp/handlers.rs`
  pattern); local side uses the `zip` + `flate2`/`tar` crates.
  Context-menu entries in `views/sftp/menus.rs`, refresh the pane on
  completion. (chmod UI and open-in-OS-editor already shipped.)
- **Argon2id auto-tuning (from the Marix audit).** `derive_key` in
  `oryxis-vault/src/store/mod.rs` uses `Argon2::default()` today.
  Calibrate at vault creation targeting ~1s on the user's machine
  (bounded memory range), persist the chosen params next to the salt,
  and read them back at unlock. Existing vaults keep their implicit
  default params until a key rotation re-derives (the rotation path
  already exists); never change params silently on an existing vault.
- **Terminal theme expansion (from the Marix audit).** Two halves:
  (1) file-picker import (`rfd`) feeding the existing
  `theme_import.rs` parsers (iTerm `.itermcolors` / Windows Terminal
  JSON / base16 already parse from pasted text; the file path is just
  read-then-reuse); (2) a curated built-in set (~20-30 hand-picked,
  contrast-verified) instead of a 400+ gallery. Owner decision
  2026-07-05: curation over mass import, no paginated theme store;
  the file importer covers the long tail.
- **Extra sync snapshot backends: DELIVERED as folder / Git / WebDAV.**
  Three transports next to `Sftp`, all reusing the SAME encrypted
  snapshot blob (`oryxis-sync/src/engine/snapshot.rs`) and the SAME
  group passphrase, so a device moves between them without leaving its
  sync group. `dispatch_folder_sync.rs`, `dispatch_git_sync.rs`,
  `dispatch_webdav_sync.rs`; the transport token lives in the
  `sync_transport` setting and `Oryxis::sync_uses_p2p()` is the single
  authority for "does this transport run the background engine" (an
  ALLOWLIST: the earlier `!= "sftp"` denylist shipped a live QUIC/mDNS
  engine under the folder and Git transports). Git drives the system
  `git` and keeps HISTORY; WebDAV is `If-Match`/ETag and the only file
  transport with real conflict DETECTION (a 412 redoes the round);
  folder subsumes every cloud client's directory at zero provider cost.
  GitHub's own API was REJECTED in favour of plain Git: same
  compare-and-swap, every forge instead of one vendor. Google Drive
  REJECTED (owner 2026-07-05, revisit only on demand): needs a Google
  Cloud project + consent screen maintained forever, embedded client
  secret, refresh-token machinery, 7-day token expiry until Google app
  verification; device flow doesn't cover the Drive scope. The folder
  transport covers it anyway through the Drive desktop client.
- **One-click relay deploy (wizard level 2).** "Install the relay on
  this host": pick a vault host, the app SSHes in, downloads the
  `relay-v*` release binary (Ed25519-verified like plugins), installs
  the systemd unit the level-1 wizard already generates, health-tests
  and adopts the endpoint. Explicit consent showing the exact script
  before running (dry-run philosophy from the AI ops spec); v1 scope
  is Linux + systemd + user-provided domain (or HTTP with a warning),
  TLS via Caddy snippet. On-brand: the SSH client deploys the user's
  own sync infra over SSH.
- **Connection health.** Per-tab latency indicator (russh keepalive
  round-trip time) and ControlMaster-style reuse: a second tab to the
  same host opens a new channel on the existing `client::Handle`
  (the SFTP path already multiplexes this way) instead of a fresh
  TCP + auth round trip.
- **Legacy keyboard modes + feature toggles.** Per-host backspace
  (^H vs ^? / 127), rxvt Home/End, function-key styles (the PuTTY
  Keyboard panel set), and per-host "disable X" toggles (mouse
  reporting, remote resize, title change). The key-encoding fork is in
  the terminal widget's key-to-bytes path; the toggles gate the
  respective handlers. Same buyer as Telnet/serial (network
  appliances); rekey limits (russh `Config`) can ride along as an
  advanced field.
- **Mosh.** Native Rust client half of mosh's State Synchronization
  Protocol in a new `oryxis-mosh` crate, interoperating with the stock
  C++ `mosh-server` (1.3.x / 1.4.0). Researched 2026-07: still no
  interoperable Rust implementation anywhere (moshpit et al. speak
  their own protocols), the wire protocol is frozen
  (`MOSH_PROTOCOL_VERSION = 2` for over a decade, upstream dormant but
  alive), and the C++ source is the spec (no written one; issue #1087).
  The interop core is small: ~2.3k lines of network/transport C++ plus
  ~0.5k statesync and 77 lines of protobuf. Crypto is AES-128-OCB3
  (RFC 7253; RustCrypto `ocb3` matches exactly; OCB patents abandoned
  2021). Phases: (1) datagram layer (UDP + OCB3 + nonce/seq) against
  vectors extracted from the C++ code; (2) transport layer
  (fragmentation, zlib via flate2, protobuf via prost, ack pruning)
  with a CI interop test against a real `mosh-server` container;
  (3) integration: bootstrap over the existing russh session
  (`mosh-server new -c 256`, parse `MOSH CONNECT <port> <key>`), a
  `TerminalTransport::Mosh` arm next to Telnet, feed bytes into
  alacritty; (4) predictive local echo: port of mosh's
  `terminaloverlay` (~1.2k lines, the piece no path gives for free)
  drawn as an overlay pass on the grid (Privacy Mode's per-span
  overlay is the precedent). GPL-3.0 -> AGPL-3.0 port is explicitly
  permitted (GPLv3 s13). Estimate 6-10 focused weeks; risks: no spec
  (expect fragmentation / ack edge cases), `ocb3` is an unaudited RC
  (mosh's own OCB code is equally unaudited; key is per-session over
  SSH), prediction tuning is fiddly. Requires `mosh-server` on the
  host + UDP 60000-61000 (surface in the host editor). Upside: first
  OPEN-SOURCE native-Windows mosh client (corrected 2026-07-10: Termius
  ships Mosh on Windows via a proprietary in-house library, so the
  unqualified claim was wrong). Fallback if it slips: spawn the system
  `mosh-client` in a PTY behind the protocol picker (macOS/Linux only,
  same pattern as the SSM plugin).

### Beyond 1.0: exploring

**History-fed autocomplete.** WindTerm and Termius (Helium) both ship
command completion fed by shell history, arg specs and stored
snippets; Termius even autofills stored passwords at password prompts.
Strong pull in both audits. Builds on the v0.9 command-history capture
(same OSC 133 + input-heuristic pipeline); an inline ghost-suggestion
UI in the terminal widget is the hard part. Direction, not committed.

**Storage browser plugins (Termora audit 2026-07-24).** S3-compatible,
SMB and Chinese-cloud object storage (Huawei OBS / Tencent COS /
Alibaba OSS) browsing as optional subprocess plugins on the existing
signed-plugin pipeline (S3's 2nd audit appearance, Marix + Termora;
the Chinese trio pairs with the J-series audience). Reuse the SFTP
pane surface where it fits; heavy SDK deps stay out of core per the
plugin rule.

**Multi-host workspace agent (AI ops phase D).** The v1.0 typed-tool
agent detached from a single tab: an engine that resolves hosts from
the vault, opens ad-hoc exec sessions via `oryxis-ssh`
(`resolve_proxy` + jump chains like `dispatch_ssh.rs` /
`mcp/handlers.rs`), pools `client::Handle`s with an idle TTL and
reuses a live tab's handle when one exists (pairs with the
connection-health reuse item). Workspace chat surface with vault
tools + the typed catalog taking an explicit host param; per-host
opt-in flag gates reachability (mirror the MCP-enabled flag, never
all-vault by default); the per-tab chat becomes a scoped view of the
same engine. Details in `AI_OPS_AGENT_SPEC.md`. Alert-driven
autonomy stays out of scope here too.

**Team vaults over P2P sync.** Hard constraint: Oryxis never runs
mandatory infrastructure, so sharing must work with only the members'
own devices plus, at most, a **self-hosted** `oryxis-relay`. Design
direction (builds on the multi-vault draft, vault = collection):

- **Replication.** A shared vault is a replicated collection riding
  the existing sync engine (QUIC + mDNS on LAN, STUN + self-hosted
  signaling/relay across networks, LWW + tombstones as today).
  Real-time when peers overlap online, convergent when they don't.
  Manifests become scoped per vault-id on a sync protocol bump
  (natural pairing with the planned XChaCha20 v6 bump).
- **Membership & keys.** Each shared vault gets its own symmetric
  vault key, wrapped per member with X25519 (device keys already
  exist from sync pairing). Invite = an `oryxis://share` link / QR
  mirroring the pairing handshake. Roles start minimal (admin /
  member); read-only cannot be cryptographically enforced against a
  malicious client without much heavier machinery, so document it as
  cooperative, not adversarial.
- **Revocation = re-key.** Removing a member rotates the vault key,
  re-wraps it for the remaining members and bumps a key epoch. The
  removed member keeps whatever they already saw (unavoidable in any
  E2E design) and loses everything after the rotation.
- **Availability without a server.** Any online member is a replica;
  for teams whose devices never overlap, an optional store-and-forward
  mailbox on the self-hosted relay holds ciphertext blobs only (the
  relay never sees keys), TTL-swept. No hosted SaaS, no central
  accounts, no server-side ACLs, ever.

## When in doubt

- **Never take shortcuts. The goal is always the state of the art.**
  No stubs, no `// TODO` gaps, no hard-coded placeholders, no
  half-wired features, no skipped edge cases or error paths. When two
  approaches exist, pick the more correct/durable one even if it's
  more work, or surface the trade-off explicitly rather than silently
  choosing the easy one.
- Keep CRUD APIs consistent with the `identities` family — same
  signatures, same behaviors (preserve-vs-clear semantics, cascade
  NULL on delete).
- Match the file's existing style by hand. Don't rely on rustfmt for
  a clean diff.
- Test passwords don't leak: structural tests > documentation.
- See `feedback_*` files in `~/.claude/projects/-home-wilson-oryxis/memory/`
  for user preferences (comments in English,
  i18n discipline, split big files, integration tests outside repo
  tree, etc.).
