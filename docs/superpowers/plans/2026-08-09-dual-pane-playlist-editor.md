# Dual-Pane Playlist Editor Implementation Plan

Status: Implemented (portable path rebasing deferred)
Date: 2026-08-09
Spec: `docs/superpowers/specs/2026-08-09-dual-pane-playlist-editor-design.md`

## Goal

Replace the rejected single-pane add-and-close browser with a full-screen,
staged, dual-pane filesystem/playlist editor. Add internal named-playlist save,
load, overwrite, rename, and delete. Applying stops playback and atomically
replaces the protected working playlist; cancelling leaves it untouched.

## Constraints

- Do not revert the accepted directory listing, drive enumeration, recursive
  media expansion, or existing playlist-store primitives merely because the v1
  renderer is rejected.
- Do replace the v1 centered browser overlay and add-and-close behavior.
- Preserve the existing application theme and keyboard-first operation.
- Support 80x24 terminals without hiding essential commands or overlapping
  status/error text. The editor owns the entire terminal while open.
- Do not add M3U/M3U8 import/export, mouse input, multi-select, or a tree-widget
  dependency.
- Implement behavior test-first at store, runtime, and render/dispatch seams.
- Keep every saved-list and working-list replacement transactional.
- Do not begin this plan until the user approves the linked design.

## Target screen states

1. **Editor / Files focused** — current directory on left, draft on right;
   contextual filesystem keys visible.
2. **Editor / Playlist focused** — same two panes; reorder/remove keys visible.
3. **Drive chooser** — explicit bounded root selection, returning to Files.
4. **Save or Save As prompt** — name entry plus conflict validation.
5. **Load chooser** — saved names, counts, timestamps; load/rename/delete actions.
6. **Rename prompt** — validates and updates the selected saved name.
7. **Delete confirmation** — identifies the exact saved playlist.
8. **Cancel/load safety** — staged rows make Cancel safe without a destructive
   confirmation dialog; loading replaces only the draft.
9. **Apply failure** — editor stays open, draft preserved, error bounded.
10. **Help overlay** — complete editor bindings, sized for 80x24.

## Final key map

### Editor-wide

| Key | Action |
|---|---|
| `Tab` | Switch Files / Staged Playlist focus |
| `s` | Save current source, or prompt for a name if unsaved |
| `Shift+S` | Save As |
| `l` | Open saved-playlist chooser |
| `F10` | Apply staged playlist |
| `Ctrl+Enter` | Apply alias when reported by the terminal |
| `Esc` | Cancel and discard the transient draft |
| `?` | Editor help |

### Files focused

| Key | Action |
|---|---|
| `Up`/`Down`, `PageUp`/`PageDown`, `Home`/`End` | Navigate listing |
| `Enter` | Descend into directory |
| `Backspace` | Parent directory |
| `~` | Explicit drive/root chooser |
| `Enter` | Descend into a directory or stage a file |
| `a` | Append below the playlist cursor |
| `i` | Insert file/folder contents above playlist cursor |

### Staged Playlist focused

| Key | Action |
|---|---|
| `Up`/`Down`, `PageUp`/`PageDown`, `Home`/`End` | Navigate draft |
| `Ctrl+Up`/`Ctrl+Down` | Reorder highlighted row |
| `d`/`Delete` | Remove highlighted row |

### Saved-playlist chooser

| Key | Action |
|---|---|
| `Up`/`Down`, `PageUp`/`PageDown` | Navigate saved playlists |
| `Enter` | Load into staging |
| `r` | Rename |
| `d`/`Delete` | Delete after confirmation |
| `Esc` | Close chooser |

## Proposed commit sequence

### Task 1 — Add named-playlist store APIs

**Files:**

- Modify `crates/tz-db/src/models.rs`
- Modify `crates/tz-db/src/playlist_store.rs`

**Work:**

- Add `PlaylistSummary { id, name, track_count, updated_at }`.
- Add ordered-path fetch and playlist summary listing.
- Add case-insensitive name resolution with explicit duplicate/conflict errors.
- Add transaction-backed create, rename, and delete operations with conflict
  rejection.
- Add `replace_playlist_tracks(playlist_id, paths)` that deletes and reinserts
  items in one transaction while preserving duplicate paths and order.
- Add a session-owned transient draft store with windowed fetch, insert, remove,
  reorder, and cleanup operations. Draft paths are not shared track rows until
  Save or Apply.
- Update `playlists.updated_at` on content/name changes.

**Tests:**

- Ordering and duplicates survive round trips.
- Failed replacement rolls back to the original contents.
- Empty replacement works.
- Create, rename, delete, counts, and timestamps work.
- Case-insensitive conflicts are rejected deterministically.

**Verification:** `cargo test -p tz-db --lib playlist_store`

**Commit:** `feat(tz-db): add transactional named-playlist management`

### Task 2 — Model typed editor state and commands

**Files:**

- Modify `crates/tz-control/src/lib.rs`
- Modify `crates/tz-core/src/runtime.rs`
- Modify `crates/tz-core/src/lib.rs` if editor snapshot types need exporting

**Work:**

- Add typed pane focus, draft row/source, chooser, prompt, confirmation, and
  dirty-state structures in the core.
- Add serializable editor command variants for open, pane navigation,
  filesystem navigation, append, insert, staged navigation/removal/reorder,
  save, save-as, load, rename, delete, Apply, Cancel, and prompt responses.
- Replace the ambiguous v1 browser add-and-close command comments/semantics.
- Keep `FsEntry`, `list_dir`, `drive_list`, and `last_browse_dir` as reused
  building blocks.

**Tests:** command serde round trips and initial editor snapshot from `Default`.

**Verification:** `cargo test -p tz-control --lib` and targeted `tz-core` tests.

**Commit:** `feat(tz-core): model staged playlist editor state`

### Task 3 — Implement isolated draft editing

**Files:**

- Modify `crates/tz-core/src/runtime.rs`

**Work:**

- Open editor on `a` without changing the working playlist.
- Implement independent pane cursors and scrolling inputs.
- Implement a cycle-safe iterative recursive scan through a background job.
- Implement append and insert-above using transient draft rows.
- Implement single-row remove and move-up/down.
- Track source playlist and content dirty state against the last open/save/load
  baseline.
- Preserve missing loaded paths and expose a missing marker to rendering.

**Tests:**

- Working playlist is unchanged throughout editing.
- Append is below the right-pane cursor; insert is above it.
- Folder recursion order is deterministic.
- Remove/reorder affect only draft.
- Duplicate paths remain allowed.
- Cancel restores normal mode without database mutation.
- Cancelling an in-flight scan or write cannot mutate a later editor session.

**Verification:** `cargo test -p tz-core --lib playlist_editor`

**Commit:** `feat(tz-core): implement isolated playlist draft editing`

### Task 4 — Implement saved-playlist workflows

**Files:**

- Modify `crates/tz-core/src/runtime.rs`

**Work:**

- Populate and navigate the load chooser.
- Load a saved playlist into staging; the transient draft can be discarded safely.
- Save a new draft, update its loaded source, and Save As.
- Reject case-insensitive name conflicts rather than overwriting silently.
- Rename and delete saved playlists; protect the concrete working ID and the
  `Default` name, including overwrite/Save As.
- Detach a draft whose source is deleted without losing staged contents.
- Ensure explicit Save/Rename/Delete survive editor Cancel.

**Tests:** complete new/update/save-as/load/rename/delete flows,
including protected working list and empty saved playlist.

**Verification:** `cargo test -p tz-core --lib saved_playlist`

**Commit:** `feat(tz-core): add saved-playlist editor workflows`

### Task 5 — Make Apply playback-safe and atomic

**Files:**

- Modify `crates/tz-core/src/player.rs`
- Modify `crates/tz-core/src/runtime.rs`
- Possibly modify `crates/tz-control/src/lib.rs` if a dedicated reset command is
  cleaner than an internal service method

**Work:**

- Add a deliberate stop-and-clear-playlist-context operation while preserving
  ordinary Stop's existing last-played marker behavior.
- Stop successfully before clearing current item, atomically replace
  the working playlist, clear find state, reset cursor, persist cleared current
  item, and close editor. A stop failure aborts before database mutation.
- On replacement failure, keep the editor/draft open and original list intact.

**Tests:**

- No-op Apply does not interrupt playback.
- Changed Apply calls stop/clear before replacement.
- Successful Apply preserves exact order and duplicates.
- Failed Apply leaves original list intact and draft available.
- Cancel never stops playback.

**Verification:** targeted `tz-core` player/runtime tests.

**Commit:** `feat(tz-core): apply playlist drafts safely and atomically`

### Task 6 — Replace v1 popup with the themed full-screen editor

**Files:**

- Modify `crates/tz-tui/src/lib.rs`

**Work:**

- Replace `draw_browse_overlay` with a full-terminal editor layout; do not draw
  the normal header, transport, or visualizer beneath it.
- Extract/reuse playlist row label and styling logic where practical.
- Render equal or responsive panes, independent viewport clamps, focus styling,
  draft name/dirty marker, missing-path markers, status, and two-line key help.
- Add bounded drive chooser, saved chooser, prompts, confirmations, and error
  overlays using the application's existing style.
- At narrow widths retain both panes if readable; define an explicit minimum
  terminal fallback message rather than clipping commands or overlapping text.

**Tests:** `TestBackend` fixtures at 80x24 and a wider size for every target
screen state, long path/name/error clipping, focused pane, and dirty marker.

**Verification:** `cargo test -p tz-tui --lib playlist_editor_render`

**Commit:** `feat(tz-tui): render full-screen dual-pane playlist editor`

### Task 7 — Wire the complete keyboard contract

**Files:**

- Modify `crates/tz-tui/src/lib.rs`

**Work:**

- Make `a` open the editor from normal mode.
- Route editor-wide, focused-pane, chooser, prompt, confirmation, and help keys.
- Ensure editor mode consumes keys that would otherwise control playback.
- Remove obsolete v1 browse key paths.
- Subject to approval, remove normal-mode direct delete/reorder/clear mutation
  bindings so all playlist editing uses staging.

**Tests:** dispatch tests for every binding and precedence rule, especially
`a` outside versus inside the editor, `Shift+S`, `F10`, optional `Ctrl+Enter`,
and Esc layers.

**Verification:** `cargo test -p tz-tui --lib playlist_editor_keys`

**Commit:** `feat(tz-tui): wire playlist editor keyboard workflow`

### Task 8 — Remove rejected leftovers and update documentation

**Files:**

- Modify `crates/tz-control/src/lib.rs`
- Modify `crates/tz-core/src/runtime.rs`
- Modify `crates/tz-tui/src/lib.rs`
- Modify `README.md`
- Modify `docs/usage.md`
- Modify `docs/PROGRESS.md`
- Modify `docs/TODO.md`

**Work:**

- Remove obsolete v1-only command variants, state, renderer, tests, and wording
  not already replaced by earlier tasks.
- Remove stale multi-select documentation; do not add multi-select state.
- Document the staged transaction model and complete key map.
- Rewrite the Tier 3 TODO entries so the rejected v1 is historical context and
  the accepted editor/save-load feature is accurately marked complete only
  after all acceptance tests pass.

**Verification:** search for stale v1 terminology and run documentation-linked
help tests.

**Commit:** `docs: document the staged dual-pane playlist editor`

### Task 9 — Full verification and live visual QA

**Files:** no planned source changes; fixes discovered here receive focused
commits rather than being hidden in the verification step.

**Work:**

- Run formatting, workspace build, tests, and clippy.
- Launch the TUI and inspect the editor at 80x24 and a wider terminal.
- Exercise an unreadable/missing path, long warning, drive switch, dirty Cancel,
  saved-list overwrite/rename/delete, and Apply while playing.
- Confirm status/error text never overlaps a pane.
- Confirm the active playlist is unchanged before Apply and that Apply stops
  playback before replacement.

**Verification commands:**

```text
cargo fmt --all -- --check
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

**Commit:** only if verification uncovers a focused fix.

## Approval and execution rules

- The user reviews the linked design, screen states, key map, database behavior,
  and this commit sequence before implementation begins.
- Any material deviation discovered during implementation returns to the user
  for a design decision instead of silently changing the workflow.
- Existing unrelated worktree changes, if any, are preserved.
- Implementation commits remain small enough to review and revert independently.
