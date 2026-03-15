# Replicon Migration Status

## Overview

Branch `cb/lightyear-replicon` replaces lightyear's custom replication internals with [bevy_replicon](https://github.com/projectharmonia/bevy_replicon). The old system (custom serialization, delta compression, change tracking, message buffering) has been removed. Lightyear retains transport, visibility, prediction, interpolation, and connection management.

The full migration document is at `lightyear_replication/src/REPLICON_INTEGRATION.md`.

## How the Bridge Works

### Three-Layer Architecture

```
Lightyear Transport Channels  <-->  RepliconChannelMap  <-->  Replicon Replication
```

Three replicon channels are registered as lightyear transport channels:

| Replicon Channel | Lightyear Mode | Direction |
|-----------------|----------------|-----------|
| `ServerChannel::Updates` | Ordered Reliable | Bidirectional |
| `ServerChannel::Mutations` | Unordered Unreliable | Bidirectional |
| `ClientChannel::MutationAcks` | Ordered Reliable | Bidirectional |

### Key Files

| File | Role |
|------|------|
| `lightyear_replication/src/send.rs` | Core: `Replicate`, `PredictionTarget`, `InterpolationTarget` with on_insert/on_replace hooks |
| `lightyear_replication/src/server.rs` | Server bridge: feeds packets between lightyear transport and replicon's `ServerMessages` |
| `lightyear_replication/src/client.rs` | Client bridge: feeds packets between lightyear transport and replicon's `ClientMessages` |
| `lightyear_replication/src/channels.rs` | `RepliconChannelMap` and channel registration |
| `lightyear_replication/src/receive.rs` | `Replicated` type alias to `ConfirmHistory` |

### Key Design Decisions

- **`AuthMethod::None`**: No ProtocolHash needed. Replicon auto-adds `AuthorizedClient` when `ConnectedClient` is present.
- **`Predicted` and `Interpolated` are replicated**: These marker components have Serialize/Deserialize and are registered with `app.replicate::<T>()` so they propagate server -> client.
- **Visibility via FilterBits**: Component hooks (`on_insert`/`on_replace`) update replicon's `ClientVisibility` per entity/client.
- **Entity mapping synced bidirectionally**: Replicon's `ServerEntityMap` is synced to lightyear's `MessageManager.entity_mapper`.

### ReplicationTargetT Trait (`send.rs:181-190`)

```rust
pub trait ReplicationTargetT {
    type VisibilityBit: Resource + Deref<Target=FilterBit>;
    type Context: Default;
    fn pre_insert(world: &mut DeferredWorld, entity: Entity);
    fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut);
    fn update_replicate_state(context: &mut Self::Context, state: &mut ReplicationState,
                              sender_entity: Entity, host_client: bool);
    fn on_replace(world: DeferredWorld, context: HookContext);
}
```

Three implementations:
- `()` (for `Replicate`): Inserts `HasAuthority` + `ReplicatedFrom` in host-server mode (`send.rs:204-273`)
- `Predicted` (for `PredictionTarget`): Inserts `Predicted` marker on host-client (`send.rs:298-348`)
- `Interpolated` (for `InterpolationTarget`): Inserts `Interpolated` marker on host-client (`send.rs:358-409`)

## What Is NOT Ready Yet

### Disabled Test Modules

In `lightyear_tests/src/client_server/mod.rs`:
```rust
// mod authority;           -- Authority transfer tests disabled
// mod delta;               -- Delta compression tests disabled
// mod replication_advanced; -- Advanced replication tests disabled
```

### Ignored Tests (21 total, 5 priority groups)

**Priority 1 - Prespawn Matching** (5 tests): Replicon creates new entities instead of matching pre-spawned ones. Need a way to pre-populate entity mappings. Tests: `test_multiple_prespawn`, `test_prespawn_local_despawn_match`, etc.

**Priority 2 - Rollback Detection** (4 tests): The `write_history -> StateRollbackMetadata -> check_rollback` chain isn't triggering. `ServerMutateTicks.last_tick()` may not be advancing through the transport bridge. Tests: `test_rollback_time_resource`, `test_despawned_predicted_rollback`, etc.

**Priority 3 - BEI Action Entity Replication** (5 tests): `ActionOf<BEIContext>` entities need entity mapper integration. Entity ID mismatches because replicon creates entities in different order. Tests: `test_actions_on_client_entity`, `test_client_rollback`, etc.

**Priority 4 - Replication Edge Cases** (4 tests): Removing `Replicated` causes despawn instead of pause; `CompReplicateOnce` not registered; `ControlledBy` disconnect behavior not wired up; crossbeam race on `Replicate` re-insertion. Tests: `test_component_remove_not_replicating`, `test_owned_by`, etc.

**Priority 5 - Child Transform Hierarchy** (1 test): `test_replicate_position_child_collider` - parent-child transform propagation broken after replication.

### Removed Systems (No Replacement Yet)

- **Delta compression**: Old system removed. Replicon has its own delta support but it's not wired up.
- **Authority transfer**: The `authority.rs` test module is commented out. `AuthorityBroker`, `GiveAuthority`, `RequestAuthority` exist but aren't fully integrated with replicon.
- **ReplicateOnce**: One-shot replication not implemented.

## TODO Comments of Note

| Location | Issue |
|----------|-------|
| `lightyear_replication/src/control.rs:16,37` | `Controlled` added on sender for replication could cause issues with authority changes |
| `lightyear_replication/src/receive.rs:6` | Components with Predicted/Interpolation should apply differently |
| `lightyear_replication/src/PLAN.md:8` | Replicon needs to give control over when server-tick is incremented |
