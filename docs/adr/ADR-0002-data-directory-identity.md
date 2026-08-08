# ADR-0002 — Data directory identity

- Status: Accepted
- Date: 2026-04-07

## Context

Python tz-player stores SQLite and JSON state under platformdirs for application `tz-player`. Sharing the same paths risks schema or state corruption while both apps coexist.

## Decision

Use a distinct application identity for the Rust port:

- Organization: `taggedzi`
- Application: `tz-player-rs`

Provide an optional import path later for users who want to migrate Python data.

## Consequences

Positive: safe side-by-side installs.  
Negative: users must re-add playlists unless they import.
