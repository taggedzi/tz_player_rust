# Folder-Add Modal Design

Status: Approved
Date: 2026-08-09
Source: `docs/TODO.md` Tier 3 — "Folder-add needs a modal that allows tree
navigation and file/folder selection; not just a short prompt line."

## Problem

Today, pressing `a` in the TUI opens a single-line free-text prompt
(`Command::RequestAddPath` → `input_mode = "add_path"`, handled in
`crates/tz-tui/src/lib.rs` around the shared find/add_path text-input block,
lines ~636-681). The user has to already know — and correctly type — the
absolute path of the file or folder they want to add. There is no way to
browse the filesystem from within the TUI.

`expand_media_paths` (`crates/tz-core/src/runtime.rs`) already recurses into
subdirectories once a path is submitted (Tier 0 fix, done), so the missing
piece is purely on the input side: a way to navigate and pick a path
interactively instead of typing one blind.

## Decisions

- **Selection model:** single selection. Highlighting an entry and confirming
  adds just that one file or folder; no multi-mark/batch-add flow.
- **Tree contents:** folders and files are both shown. Files are filtered to
  recognized media extensions (reusing `is_media_extension` from
  `crates/tz-core/src/runtime.rs`) so the listing isn't cluttered with
  irrelevant files; all subdirectories are always shown (you need to walk
  through non-media folders to reach music).
- **Starting root / memory:** first time the modal opens in a given run, it
  starts at the current working directory. Every subsequent open in that same
  session starts at the last directory browsed to. Nothing persists across
  restarts — this is a new session-only `AppRuntime` field, not an `AppState`
  addition (same pattern as the existing `visualizer_hidden` field).
- **Navigation style:** a single-pane directory browser (list current
  directory's contents, `..`/parent navigation, descend on `Enter`), not a
  multi-level expand/collapse tree widget. See Approach below for why.
- **Replaces, not augments:** the old free-text `add_path` prompt is removed
  entirely in favor of the new modal. Confirmed safe — `RequestAddPath` /
  `input_mode == "add_path"` are TUI-only; the CLI adds paths via
  `add_paths_cli` directly and never issues these commands. No dual add-flow
  is kept.

## Approach

**Chosen: single-pane directory browser**, modeled as one "you are here"
listing with up/down navigation — not a real indented multi-level tree.

Rejected alternatives:
- **Full expand/collapse tree widget:** no tree widget exists in this
  codebase or as a dependency (ratatui 0.29 ships none); the codebase's own
  list convention (`draw_playlist`) is a flat, manually-styled `List` with no
  `ListState`. A real tree needs new recursive state (per-node expand
  flags) with no precedent to build on — disproportionate complexity for a
  single-selection add flow.
- **Pull in `tui-tree-widget`:** adds an external dependency and a different
  rendering idiom than the rest of the TUI uses. Rejected for consistency and
  YAGNI.

The single-pane browser still satisfies "tree navigation" in the TODO's
sense — walking the filesystem hierarchy — and "file/folder selection",
without new dependencies or a new state-management pattern.

## Architecture

Follows the existing `Command`-driven pattern already used for e.g.
`CursorUp`/`CursorDown`/`PageUp` — navigation state lives in `AppRuntime`
(the shared headless core used by both TUI and CLI, per its own doc comment),
not in TUI-local variables. This keeps the browser logic unit-testable
without a terminal, matching how the rest of the runtime is tested.

### New data (`crates/tz-core`)

```rust
pub struct FsEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}
```

`list_dir(path: &Path) -> Vec<FsEntry>`: returns all subdirectories plus
files matching `is_media_extension`, sorted directories-first then
alphabetically. On Windows, when `path` is a drive root, "going up" instead
yields a synthetic drive list (probe `A:`..`Z:` via `fs::metadata`, include
only ones that succeed) so a library on another drive is reachable. On Unix,
`/` simply has no parent entry.

### New `AppRuntime` fields (`crates/tz-core/src/runtime.rs`)

```rust
pub browse_dir: PathBuf,
pub browse_entries: Vec<FsEntry>,
pub browse_cursor: usize,
pub browse_scroll: usize,
last_browse_dir: Option<PathBuf>, // session-only, not persisted
```

### New `Command` variants (`crates/tz-control/src/lib.rs`)

```rust
RequestAddFolder,   // replaces RequestAddPath; opens the browser
BrowseUp,           // cursor up within current listing
BrowseDown,         // cursor down within current listing
BrowseEnter,        // descend into highlighted dir, or add+close on a file
BrowseSelect,        // add highlighted folder recursively (or file) and close
BrowseParent,       // go up one level (or to drive list at a drive root)
BrowseCancel,       // close modal, no changes
```

`RequestAddPath` and the `"add_path"` input mode are deleted, along with
their text-input handling block in `crates/tz-tui/src/lib.rs`. New
`input_mode` value: `"browse"`.

`Command::AddPaths` is unchanged. The browser's add action calls it with a
single path — `expand_media_paths` already does the recursive scan for
folders, so no new expansion logic is needed here.

## Keybindings (inside the modal)

| Key | Action |
|---|---|
| `↑`/`↓` (or `j`/`k`) | Move cursor in current listing |
| `Enter` on a folder | Descend into it |
| `Enter` on a file | Add that file, close modal |
| `a` / `Space` on a folder | Add that folder recursively, close modal |
| `Backspace` / `←` | Go up one level (or to drive list at a drive root) |
| `Esc` | Cancel, no changes |

`a` (previously "open add prompt") now opens the browser directly
(`Command::RequestAddFolder`), starting at `last_browse_dir` if set, else the
current working directory.

## Rendering

Reuses the help-modal recipe from `crates/tz-tui/src/lib.rs`
(`draw_help_overlay`): `Clear` widget over the target `Rect`, then a bordered
`Block`/`List` on top, sized via `centered_fixed_rect`. Unlike the help
modal (sized exactly to fixed content), the browser's popup size is capped to
a reasonable max (e.g. 70% of terminal width/height) with the existing
scroll-offset-clamp pattern from `draw_playlist`/`ui_loop`
(`crates/tz-tui/src/lib.rs` lines ~49-69) reused for `browse_scroll` when the
listing overflows the popup.

Row styling follows `draw_playlist`'s manual-highlight convention (no
`ListState`): the cursor row gets the same cyan-background style; directories
get a distinguishing marker/color (e.g. trailing `/`) from files.

## Error handling

- Unreadable directory (permissions, race with deletion, etc.):
  `list_dir` returns what it can; if the target directory itself can't be
  read at all, show a `StatusLevel::Warn` message and keep the browser open
  at the previous (still-valid) directory rather than crashing or showing a
  blank pane.
- Empty directory: show the pane with just a `..` entry (or drive list at a
  root) — not an error state.

## Testing

- `tz-core`: unit tests for `list_dir` (dirs-first sort, media-extension
  filtering, unreadable-dir fallback, Windows drive-root probing behind a
  `cfg(windows)` test or a mockable seam) and for the new `Command` handlers
  on `AppRuntime` using scratch temp directories — headless, no terminal
  needed, consistent with existing `expand_media_paths` tests
  (`crates/tz-core/src/runtime.rs`).
- `tz-tui`: a `TestBackend` render test for the browser overlay (mirrors
  `help_overlay_documents_every_previously_undocumented_key_on_a_standard_terminal`),
  and a `handle_key` dispatch test exercising open → navigate → descend →
  select → close, following the pattern of the Tier 2 real-time-search test.
- Update `help_lines()` and the help modal's fixture test, plus
  `README.md`/`docs/usage.md`/`docs/PROGRESS.md`, for the new keybindings —
  same documentation-parity discipline as prior Tier 2/3 work.

## Out of scope (not this pass)

- Multi-select / batch-add from the browser.
- Persisting the last-browsed folder across restarts.
- Manual path typing/jump within the browser (e.g. a `:`-style path-entry
  shortcut). Could be a future enhancement if the single-pane browser proves
  too slow for deeply nested libraries, but not needed for the MVP.
