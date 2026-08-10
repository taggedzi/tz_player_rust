# Security policy

## Temporary dependency exceptions

Exceptions are a last resort. Each exception must name an owner, explain why
the dependency remains, state the compensating controls, and expire within 90
days. The owner must remove, replace, or explicitly renew the exception before
its expiration date.

### RUSTSEC-2024-0436 (`paste` 1.0.15)

- **Status:** accepted, time-bounded technical risk
- **Owner:** Matthew Craig (repository maintainer)
- **Accepted:** 2026-08-10
- **Expires:** 2026-11-08
- **Dependency path:** `tz-player` -> `lofty` 0.25.0 -> `paste` 1.0.15
- **Reason:** the current Lofty release still has a mandatory dependency on
  `paste`. Ratatui was upgraded to 0.30.2, which removed its separate `paste`
  dependency, and Lofty was upgraded to 0.25.0, so this is the only remaining
  path.
- **Risk assessment:** RUSTSEC-2024-0436 reports that `paste` is unmaintained;
  it does not report a known vulnerability. `paste` is a build-time procedural
  macro and does not execute in the media-player process at runtime.
- **Compensating controls:** `Cargo.lock` pins `paste` to 1.0.15; CI audits the
  lockfile; dependency-policy configuration identifies this exact advisory;
  upgrades are reviewed before release.
- **Exit criteria:** upgrade Lofty to a release that removes `paste`, replace
  Lofty, or vendor a reviewed minimal alternative. Do not release after the
  expiration date without recording a new review and a new deadline.

