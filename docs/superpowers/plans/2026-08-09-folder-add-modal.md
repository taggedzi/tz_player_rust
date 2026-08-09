# Folder-Add Modal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the free-text "add path" prompt in the TUI with a directory-browser modal that lets the user navigate the filesystem and pick a file (added directly) or a folder (added recursively) — no more typing an absolute path blind.

**Architecture:** A single-pane directory browser, driven through the existing `Command`-dispatch pattern (like `CursorUp`/`PageDown`). Browse state (current directory, listed entries, cursor) lives in `AppRuntime` (`tz-core`, headless, shared by TUI/CLI); the TUI (`tz-tui`) only renders it and forwards keys as `Command`s, mirroring how the playlist pane already works. No new dependencies.

**Tech Stack:** Rust, `ratatui` 0.29 (existing dependency), `std::fs` for directory listing (no new crate).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-09-folder-add-modal-design.md` — read it before starting; every task below implements a specific section of it.
- Single selection only — no multi-mark/batch add (spec "Decisions").
- The old free-text `add_path` prompt (`Command::RequestAddPath`, `input_mode == "add_path"`) is deleted outright, not kept alongside the new modal. Confirmed safe: it's TUI-only, never used by the CLI (`add_paths_cli`) or any other consumer.
- Session-only "last browsed directory" memory — a new `AppRuntime` field, **not** an `AppState`/persisted-config change.
- Follow existing codebase conventions: manual `List`/`ListItem` row highlighting (no `ListState`), `Clear` + bordered `Block` overlay sized via `centered_fixed_rect` (see `draw_help_overlay` in `crates/tz-tui/src/lib.rs:556-571`), and TDD with `ratatui::backend::TestBackend` for render assertions.
- Every task must leave `cargo build --workspace` and `cargo test --workspace` green before moving to the next task.

---

### Task 1: `FsEntry` + directory listing (`tz-core`)

**Files:**
- Modify: `crates/tz-core/src/runtime.rs` (add near `expand_media_paths`/`is_media_extension`, around line 716)
- Modify: `crates/tz-core/src/lib.rs:14` (export `FsEntry`)
- Test: same `#[cfg(test)] mod tests` block in `crates/tz-core/src/runtime.rs`

**Interfaces:**
- Produces: `pub struct FsEntry { pub name: String, pub path: PathBuf, pub is_dir: bool }` (derives `Debug, Clone, PartialEq, Eq`)
- Produces: `pub fn list_dir(dir: &Path) -> Vec<FsEntry>`
- Produces: `pub fn drive_list() -> Vec<FsEntry>` (Windows: mounted drive letters as synthetic dir entries; empty on other platforms)
- Consumes: existing private `fn is_media_extension(path: &Path) -> bool` (`crates/tz-core/src/runtime.rs:743`) — same file, no visibility change needed.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/tz-core/src/runtime.rs` (reuse the existing `temp_dir(name: &str) -> PathBuf` helper already defined there at line 792):

```rust
#[test]
fn list_dir_sorts_directories_before_files_case_insensitively() {
    let dir = temp_dir("list_sort");
    std::fs::write(dir.join("zebra.mp3"), b"").unwrap();
    std::fs::write(dir.join("Alpha.mp3"), b"").unwrap();
    std::fs::create_dir_all(dir.join("Zeta")).unwrap();
    std::fs::create_dir_all(dir.join("beta")).unwrap();

    let entries = list_dir(&dir);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

    assert_eq!(names, vec!["beta", "Zeta", "Alpha.mp3", "zebra.mp3"]);
    assert!(entries[0].is_dir && entries[1].is_dir);
    assert!(!entries[2].is_dir && !entries[3].is_dir);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_dir_filters_out_non_media_files() {
    let dir = temp_dir("list_filter");
    std::fs::write(dir.join("song.mp3"), b"").unwrap();
    std::fs::write(dir.join("cover.jpg"), b"").unwrap();
    std::fs::write(dir.join("readme.txt"), b"").unwrap();

    let entries = list_dir(&dir);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "song.mp3");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_dir_on_unreadable_or_missing_path_returns_empty() {
    let missing = std::env::temp_dir().join("tz_runtime_does_not_exist_12345");
    assert!(list_dir(&missing).is_empty());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tz-core --lib list_dir`
Expected: FAIL with "cannot find function `list_dir` in this scope"

- [ ] **Step 3: Implement `FsEntry`, `list_dir`, `drive_list`**

Add just above `fn expand_media_paths` (`crates/tz-core/src/runtime.rs:716`):

```rust
/// One entry in a directory listing shown by the folder-browser modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// List `dir`'s contents for the folder-browser modal: every subdirectory,
/// plus files recognized by `is_media_extension` (non-media clutter stays
/// out of the pane). Directories sort first, then alphabetically
/// (case-insensitive) within each group. An unreadable or missing
/// directory yields an empty list rather than erroring — callers fall back
/// to the previous, still-valid directory.
pub fn list_dir(dir: &Path) -> Vec<FsEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for ent in rd.flatten() {
        let path = ent.path();
        let name = ent.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            dirs.push(FsEntry {
                name,
                path,
                is_dir: true,
            });
        } else if path.is_file() && is_media_extension(&path) {
            files.push(FsEntry {
                name,
                path,
                is_dir: false,
            });
        }
    }
    let by_name_ci = |a: &FsEntry, b: &FsEntry| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    };
    dirs.sort_by(by_name_ci);
    files.sort_by(by_name_ci);
    dirs.extend(files);
    dirs
}

/// Windows drive letters currently mounted, as synthetic browse entries
/// (e.g. `C:\`). Reached only by going "up" from a drive root — no single
/// filesystem parent spans drives. Always empty on non-Windows targets,
/// where `/` has no such concept.
#[cfg(windows)]
pub fn drive_list() -> Vec<FsEntry> {
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let path = PathBuf::from(&root);
        if std::fs::metadata(&path).is_ok() {
            out.push(FsEntry {
                name: root,
                path,
                is_dir: true,
            });
        }
    }
    out
}

#[cfg(not(windows))]
pub fn drive_list() -> Vec<FsEntry> {
    Vec::new()
}
```

- [ ] **Step 4: Export `FsEntry` from the crate root**

In `crates/tz-core/src/lib.rs:18`, change:

```rust
pub use runtime::{open_runtime, AppRuntime, RuntimeError, StatusLevel};
```

to:

```rust
pub use runtime::{list_dir, open_runtime, AppRuntime, FsEntry, RuntimeError, StatusLevel};
```

(`drive_list` stays crate-internal for now — only `AppRuntime`'s own command handling in Task 3 needs it, from within the same file.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tz-core --lib list_dir`
Expected: PASS (3 tests: `list_dir_sorts_directories_before_files_case_insensitively`, `list_dir_filters_out_non_media_files`, `list_dir_on_unreadable_or_missing_path_returns_empty`)

Also run: `cargo build --workspace` to confirm the new export doesn't break anything downstream yet (nothing consumes it until Task 3).

- [ ] **Step 6: Commit**

```bash
git add crates/tz-core/src/runtime.rs crates/tz-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(tz-core): directory listing for the folder-browser modal

Adds FsEntry/list_dir (dirs-first, media-filtered) and drive_list
(Windows drive-root probing) as pure, unit-tested building blocks for
the Tier 3 folder-add modal. Not wired into AppRuntime yet.
EOF
)"
```

---

### Task 2: `Command` variants for the browser (`tz-control`)

**Files:**
- Modify: `crates/tz-control/src/lib.rs:46-50`
- Test: `crates/tz-control/src/lib.rs` (existing `#[cfg(test)] mod tests`, `command_json_round_trip`)

**Interfaces:**
- Produces: `Command::RequestAddFolder`, `Command::BrowseUp`, `Command::BrowseDown`, `Command::BrowseEnter`, `Command::BrowseSelect`, `Command::BrowseParent`, `Command::BrowseCancel` (all unit variants, serde tag `command`, snake_case: `request_add_folder`, `browse_up`, `browse_down`, `browse_enter`, `browse_select`, `browse_parent`, `browse_cancel`).
- Removes: `Command::RequestAddPath`.

- [ ] **Step 1: Write the failing test**

Add to `crates/tz-control/src/lib.rs`'s existing `mod tests` block (after `command_json_round_trip`):

```rust
#[test]
fn browse_commands_json_round_trip() {
    for cmd in [
        Command::RequestAddFolder,
        Command::BrowseUp,
        Command::BrowseDown,
        Command::BrowseEnter,
        Command::BrowseSelect,
        Command::BrowseParent,
        Command::BrowseCancel,
    ] {
        let json = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, back);
    }
    let json = serde_json::to_string(&Command::RequestAddFolder).unwrap();
    assert!(json.contains("request_add_folder"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p tz-control --lib browse_commands_json_round_trip`
Expected: FAIL with "no variant named `RequestAddFolder` found for enum `Command`" (or similar)

- [ ] **Step 3: Update the `Command` enum**

In `crates/tz-control/src/lib.rs`, replace:

```rust
    AddPaths {
        paths: Vec<String>,
    },
    /// Prompt/path for interactive add (TUI fills path string).
    RequestAddPath,
```

with:

```rust
    AddPaths {
        paths: Vec<String>,
    },
    /// Open the folder-browser modal (TUI fills its own navigation state).
    RequestAddFolder,
    /// Move the browser cursor up/down within the current directory listing.
    BrowseUp,
    BrowseDown,
    /// Descend into the highlighted directory, or add-and-close on a file.
    BrowseEnter,
    /// Add the highlighted file/folder (recursively, for a folder) and close.
    BrowseSelect,
    /// Go up one directory level (or to the drive list, at a drive root).
    BrowseParent,
    /// Close the browser without adding anything.
    BrowseCancel,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p tz-control --lib`
Expected: PASS, including the existing `command_json_round_trip` test (unaffected) and the new `browse_commands_json_round_trip`.

- [ ] **Step 5: Confirm downstream compile errors are the expected ones**

Run: `cargo build --workspace`
Expected: FAIL — `crates/tz-core/src/runtime.rs` and `crates/tz-tui/src/lib.rs` still reference `Command::RequestAddPath`, which no longer exists. This is expected; Tasks 3 and 4 fix it. Do not paper over it here.

- [ ] **Step 6: Commit**

```bash
git add crates/tz-control/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(tz-control): add folder-browser Command variants

Replaces RequestAddPath with RequestAddFolder plus Browse{Up,Down,
Enter,Select,Parent,Cancel}. Downstream crates (tz-core, tz-tui) are
intentionally left broken until the next two tasks wire them up —
this isolates the API-surface change from its consumers.
EOF
)"
```

---

### Task 3: `AppRuntime` browse state + command handlers (`tz-core`)

**Files:**
- Modify: `crates/tz-core/src/runtime.rs` (struct fields ~32-64, construction ~133-159, `handle()` match ~353-534)
- Test: `crates/tz-core/src/runtime.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `FsEntry`, `list_dir`, `drive_list` (Task 1); `Command::RequestAddFolder`/`BrowseUp`/`BrowseDown`/`BrowseEnter`/`BrowseSelect`/`BrowseParent`/`BrowseCancel` (Task 2); existing private `fn add_paths_internal(&mut self, paths: &[PathBuf]) -> Result<(), ControlError>` (`crates/tz-core/src/runtime.rs:562`); existing test helper `async fn test_runtime(name: &str) -> AppRuntime` (`crates/tz-core/src/runtime.rs:837`).
- Produces: `AppRuntime.browse_dir: Option<PathBuf>`, `AppRuntime.browse_entries: Vec<FsEntry>`, `AppRuntime.browse_cursor: usize` (all `pub`, read by `tz-tui` in Tasks 4-5).

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/tz-core/src/runtime.rs`:

```rust
#[tokio::test]
async fn request_add_folder_opens_browser_at_last_dir_or_cwd() {
    let mut runtime = test_runtime("browse_open").await;
    let dir = temp_dir("browse_open_target");
    std::fs::write(dir.join("track.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(dir.clone());

    runtime.handle(Command::RequestAddFolder).await.unwrap();

    assert_eq!(runtime.input_mode, "browse");
    assert_eq!(runtime.browse_dir, Some(dir.clone()));
    assert_eq!(runtime.browse_cursor, 0);
    assert!(runtime.browse_entries.iter().any(|e| e.name == "track.mp3"));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}

#[tokio::test]
async fn browse_enter_descends_into_a_directory() {
    let mut runtime = test_runtime("browse_descend").await;
    let root = temp_dir("browse_descend_root");
    let sub = root.join("Album");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("song.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(root.clone());
    runtime.handle(Command::RequestAddFolder).await.unwrap();
    assert_eq!(runtime.browse_cursor, 0); // "Album" sorts as the only entry

    runtime.handle(Command::BrowseEnter).await.unwrap();

    assert_eq!(runtime.browse_dir, Some(sub.clone()));
    assert_eq!(runtime.last_browse_dir, Some(sub.clone()));
    assert!(runtime.browse_entries.iter().any(|e| e.name == "song.mp3"));
    assert_eq!(runtime.input_mode, "browse", "descending should not close the modal");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}

#[tokio::test]
async fn browse_enter_on_a_file_adds_it_and_closes() {
    let mut runtime = test_runtime("browse_add_file").await;
    let dir = temp_dir("browse_add_file_target");
    std::fs::write(dir.join("only.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(dir.clone());
    runtime.handle(Command::RequestAddFolder).await.unwrap();

    runtime.handle(Command::BrowseEnter).await.unwrap();

    assert_eq!(runtime.input_mode, "normal");
    assert_eq!(runtime.playlist_count(), 1);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}

#[tokio::test]
async fn browse_select_adds_a_folder_recursively_and_closes() {
    let mut runtime = test_runtime("browse_add_folder").await;
    let root = temp_dir("browse_add_folder_root");
    let sub = root.join("Album");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("one.mp3"), b"").unwrap();
    std::fs::write(sub.join("two.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(root.clone());
    runtime.handle(Command::RequestAddFolder).await.unwrap();
    assert_eq!(runtime.browse_cursor, 0); // cursor is on "Album"

    runtime.handle(Command::BrowseSelect).await.unwrap();

    assert_eq!(runtime.input_mode, "normal");
    assert_eq!(runtime.playlist_count(), 2);

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}

#[tokio::test]
async fn browse_parent_goes_up_one_level() {
    let mut runtime = test_runtime("browse_parent").await;
    let root = temp_dir("browse_parent_root");
    let sub = root.join("Album");
    std::fs::create_dir_all(&sub).unwrap();
    runtime.last_browse_dir = Some(sub.clone());
    runtime.handle(Command::RequestAddFolder).await.unwrap();
    assert_eq!(runtime.browse_dir, Some(sub.clone()));

    runtime.handle(Command::BrowseParent).await.unwrap();

    assert_eq!(runtime.browse_dir, Some(root.clone()));

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}

#[tokio::test]
async fn browse_cancel_closes_without_adding_anything() {
    let mut runtime = test_runtime("browse_cancel").await;
    let dir = temp_dir("browse_cancel_target");
    std::fs::write(dir.join("track.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(dir.clone());
    runtime.handle(Command::RequestAddFolder).await.unwrap();

    runtime.handle(Command::BrowseCancel).await.unwrap();

    assert_eq!(runtime.input_mode, "normal");
    assert_eq!(runtime.playlist_count(), 0);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}

#[tokio::test]
async fn browse_up_and_down_clamp_cursor_to_entry_bounds() {
    let mut runtime = test_runtime("browse_clamp").await;
    let dir = temp_dir("browse_clamp_target");
    std::fs::write(dir.join("a.mp3"), b"").unwrap();
    std::fs::write(dir.join("b.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(dir.clone());
    runtime.handle(Command::RequestAddFolder).await.unwrap();

    runtime.handle(Command::BrowseUp).await.unwrap();
    assert_eq!(runtime.browse_cursor, 0, "cannot go above the first entry");

    runtime.handle(Command::BrowseDown).await.unwrap();
    runtime.handle(Command::BrowseDown).await.unwrap();
    assert_eq!(runtime.browse_cursor, 1, "cannot go past the last entry");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tz-core --lib browse_`
Expected: FAIL to compile — `browse_dir`/`browse_entries`/`browse_cursor`/`last_browse_dir` don't exist on `AppRuntime` yet, and `Command::RequestAddFolder` etc. aren't handled.

- [ ] **Step 3: Add the new `AppRuntime` fields**

In `crates/tz-core/src/runtime.rs`, update the `input_mode` doc comment and add the new fields to the struct (around lines 48-50):

```rust
    /// "normal" | "find" | "browse" | "help"
    pub input_mode: String,
    pub input_buffer: String,
    /// Current directory shown by the folder-browser modal. `None` means
    /// the synthetic drive-selection level (Windows only, reached by going
    /// up from a drive root — see `Command::BrowseParent`).
    pub browse_dir: Option<PathBuf>,
    pub browse_entries: Vec<FsEntry>,
    pub browse_cursor: usize,
    /// Directory the browser starts at on its *next* open this session.
    /// Not persisted to `AppState` — the first open of a run always starts
    /// at the current working directory.
    last_browse_dir: Option<PathBuf>,
```

Add matching fields to the `AppRuntime` struct literal in `open_runtime` (around line 148-149, right after `input_buffer: String::new(),`):

```rust
        input_mode: "normal".into(),
        input_buffer: String::new(),
        browse_dir: None,
        browse_entries: Vec::new(),
        browse_cursor: 0,
        last_browse_dir: None,
```

- [ ] **Step 4: Implement the `Command` handlers**

In `crates/tz-core/src/runtime.rs`, replace the `Command::RequestAddPath` arm (lines 458-462):

```rust
            Command::RequestAddPath => {
                self.input_mode = "add_path".into();
                self.input_buffer.clear();
                self.set_status("Enter path (Enter=add, Esc=cancel)");
            }
```

with:

```rust
            Command::RequestAddFolder => {
                let dir = self.last_browse_dir.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                self.browse_entries = list_dir(&dir);
                self.browse_dir = Some(dir);
                self.browse_cursor = 0;
                self.input_mode = "browse".into();
                self.set_status(
                    "Browse: Enter=open/add file  a/Space=add folder  Backspace=up  Esc=cancel",
                );
            }
            Command::BrowseUp => {
                self.browse_cursor = self.browse_cursor.saturating_sub(1);
            }
            Command::BrowseDown => {
                if !self.browse_entries.is_empty() {
                    self.browse_cursor =
                        (self.browse_cursor + 1).min(self.browse_entries.len() - 1);
                }
            }
            Command::BrowseEnter => {
                if let Some(entry) = self.browse_entries.get(self.browse_cursor).cloned() {
                    if entry.is_dir {
                        if std::fs::read_dir(&entry.path).is_err() {
                            // Keep the browser at the current (still-valid)
                            // directory rather than switching into one we
                            // can't actually list — an empty pane with no
                            // explanation would look like a bug, not a
                            // permissions issue.
                            self.set_warning(format!("Can't open '{}'", entry.name));
                        } else {
                            self.browse_entries = list_dir(&entry.path);
                            self.browse_dir = Some(entry.path.clone());
                            self.last_browse_dir = Some(entry.path);
                            self.browse_cursor = 0;
                        }
                    } else {
                        let name = entry.name.clone();
                        self.add_paths_internal(&[entry.path])?;
                        self.input_mode = "normal".into();
                        self.set_status(format!("Added '{name}'"));
                    }
                }
            }
            Command::BrowseSelect => {
                if let Some(entry) = self.browse_entries.get(self.browse_cursor).cloned() {
                    let name = entry.name.clone();
                    self.add_paths_internal(&[entry.path])?;
                    self.input_mode = "normal".into();
                    self.set_status(format!("Added '{name}'"));
                }
            }
            Command::BrowseParent => match self.browse_dir.clone() {
                Some(dir) => match dir.parent() {
                    Some(parent) => {
                        if std::fs::read_dir(parent).is_err() {
                            self.set_warning(format!("Can't open '{}'", parent.display()));
                        } else {
                            let parent = parent.to_path_buf();
                            self.browse_entries = list_dir(&parent);
                            self.browse_dir = Some(parent);
                            self.browse_cursor = 0;
                        }
                    }
                    None => {
                        let drives = drive_list();
                        if !drives.is_empty() {
                            self.browse_entries = drives;
                            self.browse_dir = None;
                            self.browse_cursor = 0;
                        }
                    }
                },
                None => {}
            },
            Command::BrowseCancel => {
                self.input_mode = "normal".into();
                self.browse_entries.clear();
                self.browse_cursor = 0;
                self.set_status("Cancelled");
            }
```

Note: the unreadable-directory guard (`std::fs::read_dir(...).is_err()`) in
`BrowseEnter`/`BrowseParent` above is deliberately not covered by an
automated test here — creating a genuinely permission-denied directory
portably (Windows vs. Unix have different, privilege-dependent mechanisms)
would make the test flaky or require elevated CI privileges for a rare edge
case. Verify it manually if needed (e.g. browse into a Windows
`System Volume Information` folder) rather than adding test infra for it.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tz-core --lib`
Expected: PASS — all new `browse_*` tests, plus every pre-existing `tz-core` test (this task doesn't touch unrelated code paths).

- [ ] **Step 6: Commit**

```bash
git add crates/tz-core/src/runtime.rs
git commit -m "$(cat <<'EOF'
feat(tz-core): wire folder-browser Commands into AppRuntime

Adds browse_dir/browse_entries/browse_cursor/last_browse_dir and
implements RequestAddFolder/Browse{Up,Down,Enter,Select,Parent,
Cancel}, replacing the old RequestAddPath text-prompt handler.
Session-only directory memory (no AppState change). tz-tui still
needs updating to stop calling RequestAddPath — next task.
EOF
)"
```

---

### Task 4: TUI key dispatch for the browser (`tz-tui`)

**Files:**
- Modify: `crates/tz-tui/src/lib.rs` (`handle_key`, lines ~621-700)
- Test: `crates/tz-tui/src/lib.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Command::RequestAddFolder`/`BrowseUp`/`BrowseDown`/`BrowseEnter`/`BrowseSelect`/`BrowseParent`/`BrowseCancel` (Task 2); `AppRuntime.input_mode`, `.browse_dir`, `.browse_entries`, `.browse_cursor` (Task 3); existing test helpers `async fn bare_test_runtime(name: &str) -> AppRuntime` and `async fn find_test_runtime(name: &str) -> AppRuntime` (`crates/tz-tui/src/lib.rs:1012, 1033`).
- Produces: `handle_key` now dispatches browser keys while `input_mode == "browse"`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/tz-tui/src/lib.rs` (after `shift_z_toggles_visualizer_hidden`):

```rust
#[tokio::test]
async fn pressing_a_opens_the_browser_instead_of_the_old_text_prompt() {
    let mut runtime = bare_test_runtime("browse_open_key").await;
    let mut viz = VisualizerHost::new(false);

    handle_key(&mut runtime, &mut viz, KeyCode::Char('a'), KeyModifiers::NONE)
        .await
        .unwrap();

    assert_eq!(runtime.input_mode, "browse");

    let _ = std::fs::remove_dir_all(&runtime.paths.data_dir);
}

#[tokio::test]
async fn browse_navigate_descend_select_and_close_via_keys() {
    let mut runtime = bare_test_runtime("browse_keys_flow").await;
    let dir = &runtime.paths.data_dir;
    let sub = dir.join("Album");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("song.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(dir.clone());
    let mut viz = VisualizerHost::new(false);

    handle_key(&mut runtime, &mut viz, KeyCode::Char('a'), KeyModifiers::NONE)
        .await
        .unwrap();
    assert_eq!(runtime.input_mode, "browse");

    handle_key(&mut runtime, &mut viz, KeyCode::Enter, KeyModifiers::NONE)
        .await
        .unwrap();
    assert_eq!(runtime.browse_dir, Some(sub.clone()), "Enter should descend into Album");

    handle_key(&mut runtime, &mut viz, KeyCode::Char('a'), KeyModifiers::NONE)
        .await
        .unwrap();
    assert_eq!(runtime.input_mode, "normal", "'a' on a highlighted file adds and closes");
    assert_eq!(runtime.playlist_count(), 1);

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn esc_cancels_the_browser_without_adding_anything() {
    let mut runtime = bare_test_runtime("browse_keys_cancel").await;
    let dir = &runtime.paths.data_dir;
    std::fs::write(dir.join("track.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(dir.clone());
    let mut viz = VisualizerHost::new(false);

    handle_key(&mut runtime, &mut viz, KeyCode::Char('a'), KeyModifiers::NONE)
        .await
        .unwrap();
    handle_key(&mut runtime, &mut viz, KeyCode::Esc, KeyModifiers::NONE)
        .await
        .unwrap();

    assert_eq!(runtime.input_mode, "normal");
    assert_eq!(runtime.playlist_count(), 0);

    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tz-tui --lib browse`
Expected: FAIL — either a compile error (`Command::RequestAddPath` no longer exists, since `crates/tz-tui/src/lib.rs:699` still references it) or, once that's fixed by the next step, assertion failures because `input_mode` never becomes `"browse"`.

- [ ] **Step 3: Update `handle_key`**

In `crates/tz-tui/src/lib.rs`, change the `'a'` key arm (line 698-700):

```rust
        KeyCode::Char('a') => {
            let _ = runtime.handle(Command::RequestAddPath).await;
        }
```

to:

```rust
        KeyCode::Char('a') => {
            let _ = runtime.handle(Command::RequestAddFolder).await;
        }
```

Replace the shared find/add_path text-input block (lines 636-681) — it currently reads `if runtime.input_mode == "find" || runtime.input_mode == "add_path"` and has `add_path`-specific arms inside `Enter`. Change the guard to just `"find"` and drop the now-dead `add_path` branches:

```rust
    // Text input mode (find)
    if runtime.input_mode == "find" {
        match code {
            KeyCode::Esc => {
                let _ = runtime.handle(Command::ClearFind).await;
                runtime.input_mode = "normal".into();
                runtime.input_buffer.clear();
                runtime.set_status("Cancelled");
            }
            KeyCode::Enter => {
                let q = runtime.input_buffer.clone();
                let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
                runtime.input_mode = "normal".into();
            }
            KeyCode::Backspace => {
                runtime.input_buffer.pop();
                let q = runtime.input_buffer.clone();
                let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
            }
            KeyCode::Char(c) => {
                runtime.input_buffer.push(c);
                let q = runtime.input_buffer.clone();
                let _ = runtime.handle(Command::SetFindQuery { query: q }).await;
            }
            _ => {}
        }
        return Ok(false);
    }
```

Then add a new block for the browser, placed right after the `input_mode == "help"` block (before the find block above), so it takes the same early-return shape as the other modal modes:

```rust
    // Folder-browser modal
    if runtime.input_mode == "browse" {
        match code {
            KeyCode::Esc => {
                let _ = runtime.handle(Command::BrowseCancel).await;
            }
            KeyCode::Up => {
                let _ = runtime.handle(Command::BrowseUp).await;
            }
            KeyCode::Down => {
                let _ = runtime.handle(Command::BrowseDown).await;
            }
            KeyCode::Enter => {
                let _ = runtime.handle(Command::BrowseEnter).await;
            }
            KeyCode::Char('a') | KeyCode::Char(' ') => {
                let _ = runtime.handle(Command::BrowseSelect).await;
            }
            KeyCode::Backspace | KeyCode::Left => {
                let _ = runtime.handle(Command::BrowseParent).await;
            }
            _ => {}
        }
        return Ok(false);
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tz-tui --lib`
Expected: PASS — the 3 new tests, plus `typing_in_find_mode_filters_the_playlist_before_enter_is_pressed` and every other pre-existing `tz-tui` test still green (find mode's behavior is unchanged, just no longer sharing a block with `add_path`).

Also run: `cargo build --workspace`
Expected: PASS — this was the last place referencing `Command::RequestAddPath`/`"add_path"` outside of rendering (Task 5 still needs `draw_footer`, which currently branches on `input_mode == "add_path"` — confirm that's the only remaining compile/behavior gap before starting Task 5).

- [ ] **Step 5: Commit**

```bash
git add crates/tz-tui/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(tz-tui): dispatch keys for the folder-browser modal

'a' now opens the browser (RequestAddFolder) instead of the old
free-text prompt. Up/Down move the cursor, Enter descends into a
folder or adds+closes on a file, 'a'/Space adds the highlighted
folder recursively, Backspace/Left goes up a level, Esc cancels.
Rendering (draw_footer's add_path branch, the overlay itself) is
still pending — next task.
EOF
)"
```

---

### Task 5: TUI rendering for the browser modal (`tz-tui`)

**Files:**
- Modify: `crates/tz-tui/src/lib.rs` (`draw_footer` ~415-457, new `draw_browse_overlay` function near `draw_help_overlay` ~556-571, `ui_loop` ~44-190)
- Test: `crates/tz-tui/src/lib.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `AppRuntime.browse_dir`/`.browse_entries`/`.browse_cursor` (Task 3); `centered_fixed_rect` (`crates/tz-tui/src/lib.rs:462`).
- Produces: `fn draw_browse_overlay(f: &mut ratatui::Frame<'_>, area: Rect, runtime: &AppRuntime, scroll_offset: &mut usize)`, called from `ui_loop`'s draw closure when `input_mode == "browse"`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/tz-tui/src/lib.rs` (after `help_overlay_documents_every_previously_undocumented_key_on_a_standard_terminal`):

```rust
#[tokio::test]
async fn browse_overlay_shows_current_dir_and_highlights_cursor_entry() {
    let mut runtime = bare_test_runtime("browse_render").await;
    let dir = &runtime.paths.data_dir.clone();
    std::fs::write(dir.join("alpha.mp3"), b"").unwrap();
    std::fs::write(dir.join("beta.mp3"), b"").unwrap();
    runtime.last_browse_dir = Some(dir.clone());
    runtime.handle(Command::RequestAddFolder).await.unwrap();
    runtime.handle(Command::BrowseDown).await.unwrap(); // cursor -> beta.mp3

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut scroll = 0usize;
    terminal
        .draw(|f| draw_browse_overlay(f, f.area(), &runtime, &mut scroll))
        .unwrap();
    let text = buffer_text(&terminal.backend().buffer().clone());

    assert!(text.contains("alpha.mp3"), "expected both entries listed:\n{text}");
    assert!(text.contains("beta.mp3"), "expected both entries listed:\n{text}");

    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p tz-tui --lib browse_overlay_shows_current_dir`
Expected: FAIL with "cannot find function `draw_browse_overlay` in this scope"

- [ ] **Step 3: Implement `draw_browse_overlay`**

Add just after `draw_help_overlay` (`crates/tz-tui/src/lib.rs:571`):

```rust
/// Folder-browser modal: a single-pane directory listing (dirs then media
/// files, per `list_dir`), with the same manual cursor-highlight convention
/// as `draw_playlist` (no `ListState`). `scroll_offset` is clamped here,
/// against the same popup height used to render — computing both in one
/// place avoids the clamp/render size mismatch that bit `main_layout`
/// before it existed as a single shared function.
fn draw_browse_overlay(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    runtime: &AppRuntime,
    scroll_offset: &mut usize,
) {
    let title = match &runtime.browse_dir {
        Some(dir) => format!(" Add — {} ", dir.display()),
        None => " Add — select a drive ".to_string(),
    };
    let popup_w = (area.width * 7 / 10).clamp(20.min(area.width), area.width);
    let popup_h = (area.height * 7 / 10).clamp(6.min(area.height), area.height);
    let popup = centered_fixed_rect(popup_w, popup_h, area);
    let visible = popup.height.saturating_sub(2).max(1) as usize; // borders

    let cursor = runtime.browse_cursor;
    if cursor < *scroll_offset {
        *scroll_offset = cursor;
    } else if cursor >= *scroll_offset + visible {
        *scroll_offset = cursor + 1 - visible;
    }
    let offset = *scroll_offset;

    let items: Vec<ListItem> = if runtime.browse_entries.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  (empty)",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )))]
    } else {
        runtime
            .browse_entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .map(|(i, entry)| {
                let is_cursor = i == cursor;
                let label = if entry.is_dir {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                let style = if is_cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if entry.is_dir {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Gray)
                };
                ListItem::new(Line::from(Span::styled(format!(" {label}"), style)))
            })
            .collect()
    };

    f.render_widget(Clear, popup);
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(list, popup);
}
```

- [ ] **Step 4: Wire it into `ui_loop` and `draw_footer`**

In `crates/tz-tui/src/lib.rs`, add a second scroll-tracking local next to the existing one (line 49):

```rust
    let mut scroll_offset = 0usize;
    let mut browse_scroll_offset = 0usize;
```

In the draw closure (right after the existing `if runtime.input_mode == "help" { draw_help_overlay(f, f.area()); }` at lines 169-171), add:

```rust
                if runtime.input_mode == "browse" {
                    draw_browse_overlay(f, f.area(), runtime, &mut browse_scroll_offset);
                }
```

In `draw_footer` (`crates/tz-tui/src/lib.rs:415-457`), replace the `add_path` branch:

```rust
    } else if input_mode == "add_path" {
        format!("Add path: {input_buffer}_   (Enter=add Esc=cancel)")
```

with:

```rust
    } else if input_mode == "browse" {
        "Browse: Enter=open/add file  a/Space=add folder  Backspace=up  Esc=cancel".into()
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p tz-tui --lib`
Expected: PASS — the new render test plus every pre-existing test (in particular `help_overlay_documents_every_previously_undocumented_key_on_a_standard_terminal`, which exercises the same overlay-sizing pattern this task follows and must remain unaffected).

Run: `cargo build --workspace`
Expected: PASS — this was the last remaining reference to the deleted `add_path`/`RequestAddPath` names; confirm with:

Run: `git grep -n "RequestAddPath\|\"add_path\""`
Expected: no matches in `crates/` (only, if any, in `docs/superpowers/specs/2026-08-09-folder-add-modal-design.md`, which documents the removal and is expected to mention the old names).

- [ ] **Step 6: Commit**

```bash
git add crates/tz-tui/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(tz-tui): render the folder-browser modal

Adds draw_browse_overlay (Clear + bordered List, same manual
cursor-highlight convention as draw_playlist, popup sized to 70% of
the terminal with in-function scroll clamping) and wires it into
ui_loop and draw_footer. This removes the last reference to the old
add_path text prompt.
EOF
)"
```

---

### Task 6: Docs — help modal, README, usage, PROGRESS, TODO

**Files:**
- Modify: `crates/tz-tui/src/lib.rs` (`help_lines`, line 541)
- Modify: `README.md:60`
- Modify: `docs/usage.md:57, 91`
- Modify: `docs/PROGRESS.md:37`
- Modify: `docs/TODO.md:33`

**Interfaces:**
- None — text-only changes, no new code surface. This task has no failing-test step since there's no new behavior to assert (the existing `help_overlay_documents_every_previously_undocumented_key_on_a_standard_terminal` test already checks for text substrings and continues to pass unchanged since none of its asserted needles is "Add path").

- [ ] **Step 1: Update the in-app help modal**

In `crates/tz-tui/src/lib.rs:541`, change:

```rust
        e2("a", "Add path", "d / Del", "Remove selected"),
```

to:

```rust
        e2("a", "Browse & add", "d / Del", "Remove selected"),
```

- [ ] **Step 2: Update `README.md`**

In `README.md:60`, change:

```
| a | Add path |
```

to:

```
| a | Browse & add files/folders |
```

- [ ] **Step 3: Update `docs/usage.md`**

In `docs/usage.md:57`, change:

```
| `a` | Add file or folder path |
```

to:

```
| `a` | Browse & add files/folders |
```

In `docs/usage.md:91`, change:

```
Press `a`, type a path, Enter — or use `tz-player add` from the shell first.
```

to:

```
Press `a` to open the folder browser (arrows to navigate, Enter to open a
folder or add a file, `a`/Space to add a highlighted folder recursively,
Esc to cancel) — or use `tz-player add` from the shell first.
```

- [ ] **Step 4: Update `docs/PROGRESS.md`**

In `docs/PROGRESS.md:37`, change:

```
| `a` | Add path (type file/folder, Enter) |
```

to:

```
| `a` | Browse & add (navigate, Enter/Space to add) |
```

- [ ] **Step 5: Update `docs/TODO.md`**

In `docs/TODO.md`, change the Tier 3 heading and its first bullet (lines 32-33):

```
## Tier 3 — Larger feature work
- Folder-add needs a modal that allows tree navigation and file/folder selection; not just a short prompt line. (Sequence after the Tier 0 recursion fix so the modal isn't built on top of the broken scan.)
```

to:

```
## Tier 3 — Larger feature work
- [x] Folder-add modal — replaced the free-text `a` prompt with a directory-browser modal (`Command::RequestAddFolder`/`Browse{Up,Down,Enter,Select,Parent,Cancel}` in `crates/tz-control`, state+handlers in `crates/tz-core/src/runtime.rs`, rendering in `crates/tz-tui/src/lib.rs::draw_browse_overlay`). Single-pane browser (not a multi-level expand/collapse tree — no tree widget exists in this codebase or as a dependency); dirs-first, media-filtered listing; Enter descends into a folder or adds+closes on a file; `a`/Space adds a highlighted folder recursively via the existing `expand_media_paths` recursion; Backspace/Left goes up a level, including a synthetic Windows drive-list at a drive root. Last-browsed directory is remembered for the rest of the session only (not persisted to `AppState`). Design: `docs/superpowers/specs/2026-08-09-folder-add-modal-design.md`.
```

- [ ] **Step 6: Verify docs build/tests still pass**

Run: `cargo test -p tz-tui --lib help_overlay`
Expected: PASS — `help_overlay_documents_every_previously_undocumented_key_on_a_standard_terminal` doesn't assert the literal string "Add path" (it checks other needles like "Remove selected", "Cycle visualizer", etc.), so the wording change in Step 1 doesn't break it. Confirm by reading the test's needle list (`crates/tz-tui/src/lib.rs`, the `for needle in [...]` block) before running, not just after.

Run: `cargo build --workspace && cargo test --workspace`
Expected: PASS — full workspace green, closing out the feature.

- [ ] **Step 7: Commit**

```bash
git add crates/tz-tui/src/lib.rs README.md docs/usage.md docs/PROGRESS.md docs/TODO.md
git commit -m "$(cat <<'EOF'
docs: document the folder-browser modal, close out Tier 3 item

Updates the in-app help overlay, README, usage.md, and PROGRESS.md
keybinding tables for the new browse-and-add flow, and marks the
Tier 3 TODO item done with an implementation summary.
EOF
)"
```
