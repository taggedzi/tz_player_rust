# Dual-Pane Playlist Editor Design

Status: Implemented (portable path rebasing deferred)
Date: 2026-08-09
Supersedes: `docs/superpowers/specs/2026-08-09-folder-add-modal-design.md`
Context: `docs/superpowers/specs/2026-08-09-folder-browser-redesign-handoff.md`

## Summary

Pressing `a` opens a full-screen, keyboard-first playlist editor styled like
the existing tz-player TUI. The editor uses the broad layout and interaction
model of Midnight Commander without copying its appearance: a current-directory
filesystem listing on the left and an editable staged playlist on the right.

The editor is deliberately transactional. Add, insert, remove, reorder, and
load operations change only a transient SQLite draft. The active playback
playlist is unchanged until the user explicitly applies the draft. Cancelling
deletes the draft. Applying first stops playback successfully, then atomically
replaces the working playlist so the player never observes a partially updated
list or stale item ordering.

Saved playlists are internal, named SQLite records. M3U/M3U8 import and export
are not part of this feature. They may be added later if a real use case emerges.

## Goals

- Make filesystem browsing and playlist construction one coherent workflow.
- Keep the filesystem and staged playlist visible at the same time.
- Allow several additions, insertions, removals, and reorders in one session.
- Protect the active playlist and current playback until Apply.
- Support internal named-playlist save, load, rename, and delete with conflict
  rejection.
- Make drive switching, pane focus, commands, errors, and dirty state obvious.
- Remain fully usable at 80x24 and without a GUI or mouse.
- Reuse the accepted filesystem and playlist infrastructure from the rejected
  v1 browser while replacing its single-pane popup interaction.

## Non-goals

- M3U, M3U8, PLS, or other playlist-file import/export.
- A permanently expanded hierarchical tree widget.
- Multiple selection or marked batches in either pane.
- Mouse support in this pass.
- Persisting an abandoned editor draft across application restarts.
- Reproducing Midnight Commander's menus, colors, or F-key bar.

## Screen and focus model

The editor owns the entire terminal while open; it is not a centered popup and
does not retain the normal header, transport, or visualizer panes. A compact
now-playing label may appear in the editor title bar, but playback continues in
the background until Apply or an explicit normal-mode action after Cancel.

```text
+ tz-player · Playlist Editor -------------------- staged: Road Trip * --+
| FILES · D:\Music\Albums       | STAGED PLAYLIST · 38 tracks          |
|-------------------------------+--------------------------------------|
| ..                    <DIR>    |  1  Artist — First Track       03:21 |
| Ambient               <DIR>    |  2  Artist — Second Track      04:08 |
| > Boards of Canada    <DIR>    | >3  Artist — Third Track       05:17 |
| Jazz                  <DIR>    |  4  filename-without-tags.flac 06:02 |
| song.mp3              5.2 MB   |                                      |
| ...                           |                                      |
+-------------------------------+--------------------------------------+
| FILES focused · Enter open · a append · i insert · ~ drives · Tab    |
| s Save  S Save As  l Load  F10 Apply  Esc Cancel  ? Help              |
+-----------------------------------------------------------------------+
```

The mockup is structural only. Borders, highlights, foreground colors, status
severity, and typography reuse the existing application style.

- `Tab` switches focus between Files and Staged Playlist.
- Each pane retains its cursor and scroll offset when focus changes.
- The focused pane has an unmistakable border/title treatment in addition to
  its cursor row.
- The first footer line is contextual help for the focused pane.
- The second footer line contains editor-wide commands.
- Status and errors occupy a bounded footer row and never render over a pane.
- The layout is designed against the complete terminal area at 80x24; if the
  terminal is smaller than the defined minimum, the editor shows a bounded
  resize message instead of clipping controls or overlapping panes.
- `?` opens complete editor help and returns to the same editor state.

## Filesystem pane

The left pane is an MC-style listing of one current directory, not an expanded
tree. It shows all child directories and only supported media files.

- Directories sort first, then files; both groups sort case-insensitively.
- `..` is always visible where a parent is meaningful.
- `Enter` on a directory descends into it.
- `Enter` on a media file stages it below the current playlist cursor.
- `Backspace` or `Left` navigates to the parent.
- `~` opens an explicit drive/root chooser. On Windows it lists available drive
  roots. On Unix-like systems it offers `/` (and can grow mount discovery later
  without changing the editor model).
- The browser starts at the last directory used during the current application
  session, otherwise the process working directory.
- Adding a directory recursively collects supported media in deterministic,
  case-insensitive relative-path order. The scan is iterative and does not
  follow symlinked directories/junctions, so cycles cannot recurse forever.
- Duplicate track paths remain allowed, matching the existing playlist store.
- Unreadable directories and scan failures produce bounded warnings and leave
  the editor open with its previous valid state. A root scan failure adds
  nothing; nested failures produce a warning and a clearly reported partial
  result only after the user confirms the add.

### Append and insert

- `a` stages the selected media file, or all supported files under the
  selected directory, below the current playlist cursor.
- `i` inserts the same selection immediately above the highlighted row in the
  staged playlist. If the staged playlist is empty, it behaves like append.
- The right-pane cursor remains remembered while Files has focus, making the
  insertion point visible and predictable.
- Both operations keep the editor open and report the number of staged tracks.

## Staged playlist pane

The right pane is a draft, initially copied from the active working playlist.
It is not visible to `PlayerService` until Apply.

- Rows reuse the normal playlist's title/artist fallback behavior when metadata
  is already available; raw filename/path is the fallback.
- `Up`/`Down`, `PageUp`/`PageDown`, `Home`, and `End` navigate.
- `d` or `Delete` removes the highlighted staged row immediately. This is safe
  without a confirmation because the entire editor can still be cancelled.
- `Ctrl+Up`/`Ctrl+Down` moves the highlighted row one position.
- There is no multi-selection state or Space-to-mark behavior.
- Playback commands are not active inside the editor; staged rows do not yet
  have valid working-playlist item IDs.

## Draft and dirty-state semantics

Opening the editor creates a session-owned transient draft in SQLite by copying
the ordered working-playlist item references. The draft also tracks:

- cursor and scroll positions for both panes;
- focused pane;
- optional source saved-playlist ID and name;
- whether its contents differ from the last opened or saved baseline;
- any active prompt/chooser/confirmation state.

The draft is not a user-visible saved playlist. It is addressed by a session
token, excluded from all saved-playlist choosers, deleted on Cancel/Apply, and
cleaned at startup if a prior process crashed. This keeps Cancel exact while
preserving windowed rendering and low memory use for large playlists. New paths
are staged as draft rows and only become shared track rows when Save or Apply
commits them. Cursor, scroll, focus, and prompt state remain runtime session
state rather than being persisted in the draft table.

`Esc` immediately discards the transient draft without stopping playback or
changing the working playlist. Explicitly completed Save, Rename, and Delete
operations are durable and are not undone by Cancel.

## Internal saved playlists

The existing `playlists` and `playlist_items` SQLite tables are the canonical
store. `Default` is the protected working playlist and is excluded from the
saved-playlist chooser.

### Save

- `s` updates the currently loaded saved playlist when one exists.
- If the draft did not come from a saved playlist, `s` prompts for a unique
  name and creates it.
- `Shift+S` always performs Save As and prompts for a new name.
- Saving writes the complete ordered draft in one transaction.
- Saving does not Apply, stop playback, or close the editor.
- A conflicting name is rejected rather than silently overwritten; the user can
  choose a different name or load the existing saved playlist.
- The concrete working-playlist ID and the protected working name (`Default`,
  case-insensitively) can never be saved over, renamed, or deleted.
- Names are trimmed, non-empty, compared case-insensitively for conflicts, and
  display with their user-entered casing.

### Load chooser

`l` opens a bounded chooser over the editor showing saved name, track count, and
last-updated time.

- `Enter` loads the highlighted saved playlist into the draft.
- Loading replaces only the draft; it never changes playback or the working
  playlist.
- If the current draft has unsaved content changes, replacement requires
  confirmation.
- `r` renames the highlighted saved playlist after validating the new name.
- `d` or `Delete` deletes the highlighted saved playlist after confirmation.
- `Default` cannot appear, be renamed, or be deleted in this chooser.
- Deleting the saved source currently loaded in the draft detaches the draft;
  its staged contents remain and the next `s` behaves as a new save.
- `Esc` closes the chooser without changing the draft.

## Apply and Cancel

### Apply (`F10`, with `Ctrl+Enter` accepted when the terminal reports it)

1. Request a stop. If the backend refuses to stop, leave the working
   playlist and draft untouched, keep the editor open, and report the failure.
   Do not clear player context or begin a database replacement in that case.
2. After a successful stop, clear the player's current/last item context.
3. Replace all working-playlist items with the ordered draft paths in one
   SQLite transaction. The old list remains intact if the transaction fails.
4. Clear find state and reset the normal playlist cursor to the first row (or
   zero for an empty list).
5. Persist a cleared `current_item_id`, delete the draft, close the editor, and
   report success.

If database replacement fails, playback remains stopped, the editor and draft
remain open, and the original working playlist remains intact.

### Cancel (`Esc`)

Cancel never stops playback and never changes the working playlist. The draft
is disposable, so no additional dirty-discard prompt is required. Previously
completed saved-playlist management operations remain completed.

## Normal-player mutation policy

The implementation centralizes playlist mutation in this editor. Remove
the normal screen's direct `d`/Delete, `Ctrl+Up`/`Ctrl+Down`, and clear
playlist mutation bindings (and their help text), because those commands bypass
staging and can alter the list being used by active playback. Normal mode keeps
playlist navigation, playback, find, locate-playing, metadata refresh, and `a`
to open the editor.

The Rust application currently has only single-row removal/reorder; it does not
have the Python UI's true multi-select set. Thus “remove multi-select” mostly
means not introducing it into the editor and removing stale documentation or
code if any is found during implementation.

## Architecture

### `tz-control`

Replace browser-specific add-and-close commands with editor commands that
express pane navigation, append/insert, staged mutation, saved-playlist actions,
Apply, and Cancel. Keep commands frontend-neutral and serializable.

### `tz-db`

Add transaction-backed operations for:

- listing playlist summaries;
- resolving a playlist by case-insensitive name;
- renaming and deleting saved playlists with `Default` protection enforced by
  the core workflow;
- fetching all ordered paths for a playlist;
- atomically replacing a playlist's complete ordered contents;
- creating or overwriting a named playlist from ordered paths.
- creating, windowing, mutating, and deleting the session-owned transient draft
  without exposing it through saved-playlist queries.

Name checks use `BEGIN IMMEDIATE` around lookup plus write so another process
cannot race an overwrite. Existing databases may theoretically contain duplicate
names, so listing remains ID-based and ambiguous conflicts are reported instead
of guessed.

### `tz-core`

Own the complete `PlaylistEditorState` and all filesystem/database mutations.
The TUI renders snapshots and dispatches commands; it does not edit SQLite or
the draft directly. Filesystem scans run as background jobs polled from `tick`,
while draft and playlist writes remain transactional and draft rendering stays
windowed. Each scan job carries the editor session token and
cannot mutate a later editor session after Cancel.

### `tz-tui`

When editor mode is active, render the full-screen two-pane layout instead of
the normal playlist/visualizer layout. Reuse existing row rendering, cursor
highlight, severity colors, fixed help overlay, and viewport clamping patterns.

## Error handling and recovery

- All saved-list and Apply writes are transactional.
- A failed backend stop aborts Apply before any database mutation.
- Name conflicts and protected-list operations are user-facing validation
  messages, not panics.
- Missing media files loaded from a saved playlist remain visible and staged,
  marked as missing; this preserves the saved ordering and lets the user remove
  or repair entries deliberately rather than silently changing a playlist.
- Scan errors identify the directory that failed and retain successfully
  collected files only if the scan completes as an accepted operation; a fatal
  root scan failure adds nothing.
- The current database stores normalized absolute paths. Portable drive-letter
  rebasing is explicitly deferred to a future path-identity feature; this
  editor must not add a second incompatible path format.
- Status text is clipped to its own row. Full error details may use a bounded
  error overlay that returns to the editor.

## Testing and acceptance

- Store tests prove atomic replacement, ordering, duplicates, name conflict,
  rename, delete, and rollback behavior.
- Runtime tests prove draft isolation, append versus insert-above, dirty-state
  confirmation, saved-list workflows, Cancel, and stop-before-Apply.
- TUI `TestBackend` tests cover 80x24 and a wider terminal, focused-pane styling,
  help/status visibility, long error clipping, chooser/prompt overlays, and key
  dispatch.
- Regression tests prove the active playlist and playback context do not change
  before Apply, and failed Apply leaves the original playlist intact.
- Full workspace formatting, clippy, build, and tests must pass.

## Approval checkpoint

Implementation began after the user's approval of the staged dual-pane
workflow. The remaining follow-up work is limited to polish and verification.
