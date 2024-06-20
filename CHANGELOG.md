# Changelog

## [Unreleased]

### Added

- Spaceships example

### Changed

- Exposed `rtt()` and `jitter()` via server's `Connection`
- `InputBuffer` bits made pub, so clients can query how many inputs are buffered for remote players
- `Rollback.is_rollback()` and `KeepaliveSettings` (for wasm) made public.

### Fixed 

- Conditionally compile steam bits only if cargo's `steam` feature is enabled. (steamworks not building on linux at the mo)