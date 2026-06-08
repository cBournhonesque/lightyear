# Changelog


## [Unreleased] - since 0.26.4

### Major changes

- Switched the replication backend to [`bevy_replicon`](https://github.com/projectharmonia/bevy_replicon).
  - The goal is to reuse the wider Bevy networking ecosystem's work, avoid splitting contribution effort, and benefit from Replicon's stronger optimization and documentation.
  - Lightyear still provides its own higher-level replication API for prediction, interpolation, authority metadata, visibility, hierarchy propagation, and pre-spawning.
  - The old `lightyear_replication` implementation was more tightly integrated with Lightyear and supported general entity-to-entity replication by adding `ReplicationSender` and `ReplicationReceiver` components. Replicon is centered on server-to-client replication, so some client-to-server and distributed-authority use cases now need different patterns.
  - Some old replication-layer features are not yet at parity, including component-level delta compression, authority switching, per-component priority, and some advanced sender/receiver topologies.
  - The move also brings useful Replicon features into Lightyear, including marker-specific replication rules, a flexible visibility-filter system, and mutation/checkpoint information that can be used by prediction and interpolation.

- Added a structured `lightyear_debug` tracing layer through `lightyear_tools`.
  - Debug events are emitted as JSONL rows with stable categories such as timeline, prediction, interpolation, input, sync, messages, entities, transport, components, and manual events.
  - Components can be sampled with typed debug or structured JSON formatters by adding `LightyearDebug`.
  - The `lightyear-debug` CLI can ingest JSONL files into DuckDB and run focused queries for merged ticks, component time series, input flow, per-tick state, and summaries.
  - This is intended to make desync investigations easier to automate and easier to inspect with LLM-assisted analysis.

- Switched `Tick` from `u16` to `u32`.
  - This avoids practical tick wraparound during normal game sessions and removes a lot of complicated sync/rollback edge cases.
  - Replicon's replication tick is now treated as a transport/checkpoint index and mapped back to Lightyear's authoritative simulation `Tick`.

### Migration notes

- Naming between Lightyear and Replicon does not line up one-to-one.
  - Replicon's native send-side marker is `Replicated`, and received entities use `Remote`.
  - Lightyear keeps `Replicate` as the user-facing send-side component. Code that previously queried Lightyear receive-side replication markers may need to move to Replicon's `Remote`, `ConfirmHistory`, or the Lightyear compatibility exports depending on intent.

- The visibility API now uses Replicon's visibility filters under the hood.
  - Room-based visibility changed significantly: use `RoomAllocator` to allocate global `RoomId`s and add `Rooms` to entities/clients.
  - Room ids are no longer ad-hoc local values; they must be allocated globally so Replicon's filter bitsets can reason about them consistently.

- `bevy_enhanced_input` integration no longer relies on general client-to-server entity replication for action entities.
  - Action entities are now expected to use the pre-spawning flow and must be spawned on both the client and server.
  - Input action replication now serializes the action context through `NetworkActionOf` and handles rebroadcasted action entities explicitly.

### Added

- Added deterministic-replication late-join catch-up support.
  - New clients can join a running deterministic game without replaying the full historical input log.
  - The catch-up flow combines deterministic input replication with a one-time state snapshot gated through Replicon visibility filters.
  - `CatchUpRequest`, `CatchUpSnapshotReady`, `CatchUpGated`, `HasCaughtUp`, `CatchUpMode`, and `AppCatchUpExt::register_catchup_components` support the flow.

- Added `register_component_once` and `register_component_once_with`.
  - These register components with Replicon's `ReplicationMode::Once`, so inserts/removals replicate but mutations are ignored.
  - This is useful for initial deterministic state and other data that should be sent once without ongoing mutation traffic.

- Added optional LZ4 transport packet compression.
  - Compression keeps packet headers uncompressed, validates decompressed payload limits, respects MTU constraints, and includes benchmark coverage.

- Added support for setting a connection request handler.

- Added an online-deployable demo game, `lightrider`.

- Added automation helpers and more coverage around examples and demos.

### Prediction and interpolation

- Prediction and interpolation now use Replicon's confirmation and mutation-checkpoint data to reason about entities/components that did not change.
  - This avoids false predictions where an entity was assumed to have changed simply because no correction arrived.
  - Unchanged entities can now still participate in rollback checks once a completed mutate tick confirms that their value stayed the same.

- Interpolation is more robust under packet loss and visibility changes.
  - Interpolated state converges to the latest confirmed value under packet loss.
  - Confirmed/interpolated history is initialized when interpolation is added.
  - Prediction/interpolation targets are re-applied correctly when visibility is regained.

- Fixed several rollback and confirmed-history edge cases.
  - Prediction can now handle predicted entities whose latest confirmed states are not all at the same tick.
  - Rollbacks start from the earliest confirmed tick among all predicted entities, so entities with newer confirmed state keep that state while older entities catch up.
  - Later confirmed values are preserved while rolling back from older confirmed ticks.
  - Confirmed init data for pre-spawned entities is recorded in prediction history without overwriting the live predicted value.
  - Host-client controlled components and authority-gain edge cases have regression coverage.

### Determinism and physics

- Improved deterministic replication examples and tests.
  - Added input-only and state-based catch-up test coverage.
  - Added support for bundled catch-up snapshots so all awaiting entities reconcile from a single coherent server tick.

- Improved Avian integration.
  - Updated Avian 2D/3D to 0.6.
  - Automatically registers rollback state for Avian island resources when rollback resources are enabled.
  - Registered `Transform` as required for `Collider` compatibility.

### Transport and networking fixes

- Replication send ticks now advance once after the fixed loop drains, avoiding multiple replication checkpoints for the same Lightyear fixed tick in catch-up frames.

- Packet-size checks now consider maximum-size edge cases more carefully.

- Crossbeam transport handles `Disconnected` and `Full` errors more gracefully, with clearer transport-pairing documentation.

- UDP receive on Windows now ignores `WSAECONNRESET`.

- Server/link lifecycle handling was tightened so server endpoints can own per-client link entities and tear them down cleanly.

### Input fixes

- Fixed dropped inputs and collapsed `JustPressed` events.

- Restricted the prepare-input rate.

- Fixed server-side pop behavior that wiped `get_predict` fallback state.

- Added local input markers only to entities that have authority.

- Fixed host-client controls in the `bevy_enhanced_inputs` example and host-server input mocking for non-host-owned actions.

### Dependencies, tools, and docs

- Updated `bevy_enhanced_input` to 0.24, Aeronet crates to 0.20, `rand` to 0.10, `hashbrown` to 0.17, and `strum` to 0.28.

- Moved the debug UI implementation into `lightyear_tools`; `lightyear_ui` is now a compatibility shim.

- Improved docs, maturity notes, release process, CI workflows, and GitHub Actions versions.



## 0.18.0 - 2024-12-24
