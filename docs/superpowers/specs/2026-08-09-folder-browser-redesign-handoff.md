# Handoff: Folder-Add Modal Needs Redesign as a Dual-Pane Tree/Playlist Screen

Status: **Needs redesign — do not extend the current single-pane modal without
re-reading this doc.** Written for a handoff to a fresh AI session/agent
because the current user is out of tokens. Read this whole document before
touching code.

## TL;DR

A single-pane folder-browser modal was built, reviewed, and merged to `main`
(commits `5b93624..1f6f6a5`, see `git log --oneline 76b25a5..1f6f6a5`). It
technically implements what `docs/TODO.md`'s Tier 3 bullet 1 said in isolation
("a modal that allows tree navigation and file/folder selection"), following
the design at `docs/superpowers/specs/2026-08-09-folder-add-modal-design.md`
and the plan at `docs/superpowers/plans/2026-08-09-folder-add-modal.md`.

**The user rejected it after seeing it in use.** The gap: the user's actual
mental model fuses Tier 3 bullet 1 (folder-add modal) with Tier 3 bullet 3
(a Norton/Midnight-Commander-style dual-pane playlist screen) into *one*
feature, not two sequenced separately. The existing TODO.md treats these as
separate, sequenced items ("Largest single item... Sequence last"); the user
does not experience them that way and wants them combined now.

**Do not just patch the existing modal.** This needs a new brainstorming
pass (design doc → plan → implementation) scoped correctly this time. The
existing merged code is a reasonable *starting point* for the tree-navigation
half, but the overall shape (single centered popup, no playlist visible, no
insert/remove/reorder from within the view) does not match what's wanted.

## What the user said, verbatim (lightly reformatted, nothing dropped)

> 1. modal opens with no instructions on how to navigate tree.
> 2. single pane tree not dual pane like Norton or Midnight commander,
> 3. no clear way to switch between seperate drives.
> 4. when adding errors display overlapping tree,
> 5. playlist not displayed in modal view,
>
> [screenshot of Midnight Commander attached — see below]
>
> This is a screenshot of Midnight commander. It does not have to be
> identical to this, but this was more what I was envisioning: file tree on
> left, playlist on right, navigation in tree, files/folders Added or
> inserted into playlist, individual items removable from playlist, items in
> playlist can be moved up/down, for future work I wanted playlist to be
> saveable/loadable (we were not there yet). On playlist side instead of
> "file details" display song info from metadata if avail (also future work).

### Screenshot description (Midnight Commander, since the image itself isn't embeddable here)

Classic MC layout on an 80x24-ish terminal:
- Top: two-letter menu bar (`Left File Command Options Right`).
- **Two side-by-side panels**, each with its own header row (`.`, `Name`,
  `Size`, `Modify time` columns) and its own blue-highlighted cursor row
  showing the current directory path (`~`) in the panel's top border.
- Each panel lists directory entries with size and modified-time columns;
  `/..` is the first row (parent-directory navigation).
- Bottom: a one-line status/hint bar ("Hint: Tab changes your current
  panel."), then a numbered F-key command bar (`1Help 2Menu 3View 4Edit
  5Copy 6RenMov 7Mkdir 8Delete 9PullDn 10Quit`).
- A shell prompt line sits below the whole UI (this is a terminal
  multiplexer scenario, not directly relevant to our TUI).

The user explicitly said it does **not** need to be identical — this is a
reference for the *shape* (dual pane, one side = filesystem, other side =
a working list you build), not a literal spec to clone. In our case the
right pane is a **playlist**, not a second filesystem panel.

## Concrete requirements extracted from the above

1. **Dual-pane layout**: filesystem tree/browser on the left, playlist on
   the right, both visible simultaneously.
2. **Playlist pane is live and editable from within this view**:
   - Insert/add a highlighted file or folder from the left pane into the
     playlist (folder = recursive add, same semantics as today).
   - Remove an individual item from the playlist pane directly.
   - Reorder items in the playlist pane (move up/down) directly.
3. **Drive switching must be discoverable**, not a hidden "go up from a
   drive root shows a synthetic drive list" mechanic. The current
   implementation *has* this mechanic (`Command::BrowseParent`'s `None`
   branch, `drive_list()` in `crates/tz-core/src/runtime.rs`) but nothing
   surfaces it as an obvious, expected action.
4. **On-screen instructions/help must actually be visible** when the view
   opens — the footer hint text exists in the current code
   (`draw_footer`'s `"browse"` branch, `crates/tz-tui/src/lib.rs`) but the
   user reports not seeing usable navigation guidance. Investigate whether
   this is a rendering bug, a footer-height/truncation issue, or genuinely
   insufficient — don't assume the existing hint text is adequate, verify
   by actually running the TUI and looking at it.
5. **Error/status text must not overlap the tree.** This is a real bug
   independent of the redesign: `crates/tz-tui/src/lib.rs`'s `draw_footer`
   and `draw_browse_overlay` currently don't coordinate — a status/error
   message set via `set_warning`/`set_status` while the browse overlay is
   showing renders in the footer *underneath* the popup, but the user's
   report of "overlapping tree" suggests something is actually drawing
   on top of / colliding with the tree pane itself. Needs live reproduction
   and a look at z-order / `Clear` widget usage in `draw_browse_overlay`
   (`crates/tz-tui/src/lib.rs`) — possibly an error surfaces *inside* the
   popup area without clearing/bounding correctly, or a longer error message
   wraps outside its allotted space. Reproduce by triggering an add error
   (e.g. try to add a path that fails) while the browser is open.

## Deferred / future work (user was explicit these are NOT needed now)

- Save/load playlists to/from disk.
- Displaying track metadata (artist/title from tags) instead of raw
  file/path info on the playlist side. (Note: metadata display already
  exists elsewhere in the app — the normal playlist pane, `draw_playlist`
  in `crates/tz-tui/src/lib.rs`, already shows `title`/`artist` when
  available via `tz_db::PlaylistRow`. The new dual-pane screen's right side
  should eventually reuse that same data/logic, but the user does not need
  this polished immediately — reusing whatever's simplest now is fine.)

## Relationship to `docs/TODO.md` Tier 3

Current TODO.md (as of this handoff) has these Tier 3 bullets:
```
- [x] Folder-add modal — ... (marked done, referencing the now-rejected design)
- All of the visualizations from the python version need to be ported...
- Remove multi-select. -> Move to a separate screen for playlist creation
  that allows moving up/down/adding/etc. almost a Norton Commander or
  Midnight commander style interface with ordering controls and the
  ability to save/open playlists. (Largest single item...)
```

**Action needed:** the `[x]` on the first bullet is no longer accurate as a
description of "done and accepted" — the code exists and is merged, but it
does not satisfy the user's actual intent. Whoever picks this up should
likely:
- Un-check or rewrite that first bullet to reflect that a v1 exists but was
  rejected pending redesign, with a pointer to this handoff doc.
- Treat this redesign as effectively **absorbing** the third bullet's core
  interaction model (dual-pane, insert/remove/reorder) minus the
  save/load-playlist part, which stays deferred/future work per the user's
  explicit note ("we were not there yet").
- The third bullet's "Remove multi-select" clause is about the *existing*
  main playlist screen's multi-select feature — it's not yet clear whether
  the user wants that removed as part of this work or later. **This is an
  open question, not yet resolved** (see below).

## What already exists (usable building blocks, not a template to preserve as-is)

All of this is in `main` after commit `1f6f6a5`:

- `crates/tz-core/src/runtime.rs`: `FsEntry`, `list_dir()`, `drive_list()` —
  pure directory-listing helpers, dirs-first + media-file-filtered. Likely
  reusable as-is for the left pane's tree data source.
- `crates/tz-core/src/runtime.rs`: `AppRuntime` fields `browse_dir`,
  `browse_entries`, `browse_cursor`, `last_browse_dir` (session-only, not
  persisted), and `Command` handlers `RequestAddFolder`/`BrowseUp`/
  `BrowseDown`/`BrowseEnter`/`BrowseSelect`/`BrowseParent`/`BrowseCancel`
  in `AppRuntime::handle()`. The navigation *logic* (list a directory,
  move cursor, descend, ascend, synthetic drive list at roots) is probably
  reusable for the left pane, but the *interaction model* needs rework:
  currently "select" always both adds AND closes the whole view — the new
  design needs "insert into playlist" to be a distinct action from "close
  this screen," since the screen should stay open for multiple
  inserts/removes/reorders in one session.
- `crates/tz-control/src/lib.rs`: `Command` enum has the `RequestAddFolder`/
  `Browse*` variants. Will likely need new variants for
  insert-without-closing, remove-from-playlist-pane,
  reorder-within-this-view, and switch-pane-focus (Tab).
- `crates/tz-tui/src/lib.rs`: `draw_browse_overlay()` (rendering),
  `handle_key()`'s `input_mode == "browse"` block (key dispatch),
  `draw_footer()`'s `"browse"` branch. All of this assumed a small popup
  over the normal view — will need to become a full-screen (or much larger)
  two-pane layout instead, per the user's answer to the one design question
  that got asked before they ran out of tokens (see below).
- The *existing* main playlist pane (`draw_playlist()`,
  `PlaylistView` struct, `crates/tz-tui/src/lib.rs`) already has: cursor
  highlighting (manual, no `ListState`), scroll-offset viewport clamping,
  now-playing marker, and remove/reorder commands
  (`Command::RemoveSelected`, playlist reorder — check `crates/tz-control`
  for the exact reorder command names, e.g. `Shift+Up/Down` per
  `docs/usage.md`). **This is the natural source of both the visual pattern
  and the actual commands for the new screen's right pane** — don't
  reinvent remove/reorder, reuse what's there.

## Open design questions (not yet resolved — start here)

Brainstorming was restarted for this redesign and got through exactly one
question before the user ran out of tokens:

**Q1 (asked, answered): Full-screen view vs. large centered modal?**
The question was asked but **no answer was recorded** before the user
stopped the session to conserve tokens — do not assume either answer.
Re-ask this first. Given the Midnight Commander reference and the amount of
information that needs to fit (two panels each with several columns), a
full-screen view is very likely the right call, but confirm with the user
rather than assuming.

Other questions that will need answering during the redesign brainstorm:

- **Pane focus switching**: Tab to switch between tree pane and playlist
  pane (matching MC's own hint, "Tab changes your current panel"), or a
  different key? Check for conflicts with this app's existing global
  keybindings (see `docs/usage.md`'s keybinding table — `Tab` does not
  appear to be bound to anything currently, so it's likely free).
- **Insert key**: what key inserts the tree pane's highlighted file/folder
  into the playlist pane, if Enter is still needed for "descend into
  folder"? Options: a dedicated key (e.g. `Insert`, or keep `a`/`Space` from
  the current design), or make Enter context-sensitive (descend on a
  folder, insert-then-stay-open on a file) with a separate key for
  "insert this folder's contents without descending."
- **Entry point**: does this new screen replace the `a` key entirely (i.e.
  `a` now opens the full dual-pane screen instead of today's modal), or is
  it reached some other way (a new dedicated key, or only from an explicit
  menu/mode)? Given the user's framing ("playlist... this was more what I
  was envisioning" for what `a` should open), replacing `a`'s target is the
  most likely intent, but confirm.
- **Relationship to the main playlist screen's multi-select**: TODO.md's
  third bullet says "Remove multi-select" from the main screen as part of
  moving that functionality to this new screen. Ask the user directly
  whether that's in scope for this redesign or a later, separate step —
  don't assume.
- **Does closing this screen return to the normal player view with the
  playlist already updated** (i.e. it's editing the *live* playlist
  directly, not a staging area that needs a separate "commit" step)? The
  user's phrasing ("individual items removable from playlist... items can
  be moved up/down") reads as live editing of the real playlist, same one
  the normal view shows — confirm this rather than building a staging/copy
  model.

## Recommended next steps for whoever resumes this

1. Read this document fully, plus the two prior docs it references
   (`docs/superpowers/specs/2026-08-09-folder-add-modal-design.md`,
   `docs/superpowers/plans/2026-08-09-folder-add-modal.md`) for
   background on what exists and why — but treat the design doc's actual
   *conclusions* (single-pane, no playlist visible) as **superseded**, not
   as constraints.
2. Re-run brainstorming from scratch with the user for this specific
   feature, starting with Q1 above (full-screen vs. large modal), then the
   other open questions.
3. Actually launch the TUI and interact with the current merged
   implementation before designing further, to directly observe bug #4
   (overlapping error text) and bug #1 (missing/inadequate instructions) —
   don't take this handoff's description as a substitute for seeing it
   live.
4. Decide whether the redesign is best done as new commits on top of the
   current merged code (reusing `list_dir`/`FsEntry`/the `Command`
   plumbing) or a more substantial rewrite of `crates/tz-tui`'s rendering
   for this feature. Given how much of the backend (`tz-core`,
   `tz-control`) is likely reusable, incremental evolution is probably
   right, but the `tz-tui` rendering and key-dispatch code for this
   feature will likely need substantial rework for the dual-pane layout.
5. Once a design is agreed, write a fresh spec doc (don't overwrite the
   existing one — write
   `docs/superpowers/specs/YYYY-MM-DD-playlist-browser-dual-pane-design.md`
   or similar) and a fresh implementation plan, following this repo's
   existing `superpowers:brainstorming` → `superpowers:writing-plans` →
   `superpowers:subagent-driven-development` workflow (see this repo's
   `docs/superpowers/` directory for the established pattern — multiple
   prior features were built this way).
6. Update `docs/TODO.md`'s Tier 3 section once the new design is settled,
   replacing the currently-inaccurate `[x]` on the folder-add bullet.
