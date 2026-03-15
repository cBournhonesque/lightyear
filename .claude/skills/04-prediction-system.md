# Prediction System

## Overview

The prediction system masks latency by running game logic locally before server confirmation, then correcting via rollback when the server state diverges from the prediction. It lives primarily in `lightyear_prediction/src/`.

## Key Types

### From bevy_replicon (not defined in lightyear)

**`ServerMutateTicks`** (Resource)
- Tracks which server ticks have had ALL mutation messages received
- `last_tick()` returns the latest tick where all messages are confirmed
- Used in: `lightyear_prediction/src/rollback.rs:1-33`, `manager.rs:175-180`
- Re-exported in `lightyear_replication/src/lib.rs:36`

**`ConfirmHistory`** (Component on every replicated entity)
- Tracks the last confirmed tick per entity
- Automatically added by replicon to all replicated entities
- Used in: `lightyear_prediction/src/rollback.rs:310,640,660,669`
- Type-aliased as `Replicated` in `lightyear_replication/src/receive.rs:11`

### From lightyear

**`PredictionHistory<C>`** (`lightyear_prediction/src/predicted_history.rs:115-120`)
```rust
struct PredictionHistory<C> {
    buffer: VecDeque<(Tick, PredictionState<C>)>  // Front=oldest, Back=newest
}

enum PredictionState<C> {
    Removed,            // Component was removed (predicted locally)
    ConfirmedRemoved,   // Component removal confirmed by server
    Predicted(C),       // Locally computed value
    Confirmed(C),       // Server-confirmed value
}
```
Key invariant: **confirmed values are preserved during rollback**.

**`PredictionManager`** (`lightyear_prediction/src/manager.rs`)
- Component on the manager entity controlling prediction
- Contains `RollbackPolicy`, rollback state, tick tracking

**`StateRollbackMetadata`** (`lightyear_prediction/src/manager.rs`)
- Tracks earliest mismatch tick across all predicted entities
- Set by `write_history` (replicon marker function) when mismatch detected
- Read by `check_rollback` to determine if rollback needed

**`Predicted`** (`lightyear_core/src/prediction.rs:5-17`)
- Marker component on client-side entities that should be predicted
- Replicated from server to client (registered with `app.replicate::<Predicted>()`)

**`PredictionTarget`** (`lightyear_replication/src/send.rs:297`)
- Type alias: `ReplicationTarget<Predicted>`
- Server-side component declaring an entity should be predicted on clients

## The Key Insight: ServerMutateTicks Guarantee

From `lightyear_prediction/src/rollback.rs:1-33` and `lightyear_replication/src/PLAN.md`:

> `ServerMutateTicks.last_tick = T` guarantees that for entities not updated at tick T, their value equals their last confirmed value.

**Proof sketch**: If entity B had no update at tick T but `ServerMutateTicks.last_tick = T`:
- Either B was updated at T-1 (so B at T = B at T-1)
- Or `ServerMutateTicks` confirmed T-1 (so B at T = last confirmed)
- Or a message for B was in-flight, but then the server wouldn't have received the ack, so it would resend at tick T (contradiction)

This means once `ServerMutateTicks` advances, unchanged entities can be treated as confirmed at their last known value.

## Prediction Flow

```
1. LOCAL PREDICTION (FixedUpdate each tick)
   - Game logic runs on Predicted entities
   - After FixedUpdate: update_prediction_history records
     (Tick, Predicted(value)) in PredictionHistory<C>

2. RECEIVE SERVER UPDATE (PreUpdate)
   - Replicon receives server component update
   - Replicon's write_history marker function is called
   - Stores (Tick, Confirmed(value)) in PredictionHistory<C>
   - If mismatch with predicted value: sets StateRollbackMetadata

3. ROLLBACK CHECK (PreUpdate, RollbackSystems::Check)
   a) Check if ServerMutateTicks advanced
   b) For unchanged entities (ConfirmHistory.tick < ServerMutateTicks.tick):
      - Compare confirmed vs predicted histories
      - If mismatch: record in StateRollbackMetadata
   c) If should_rollback: set Rollback component with rollback_tick

4. PREPARE ROLLBACK (PreUpdate, RollbackSystems::Prepare)
   For each Predicted component:
   - Get restore_value = history.get(rollback_tick)
   - Clear old entries < server_confirmed_tick
   - Clear predicted values > rollback_tick (preserve confirmed)
   - Set component = restore_value

5. RUN ROLLBACK (PreUpdate, RollbackSystems::Rollback)
   - Run FixedMain schedule from rollback_tick to current_tick
   - Each FixedPreUpdate: snap_to_confirmed_during_rollback()
     checks PredictionHistory.get_confirmed_at(tick)
     if confirmed value exists at this tick, snap component to it
   - Each FixedUpdate: normal game simulation
   - Each FixedPostUpdate: record new prediction history

6. VISUAL CORRECTION (optional)
   - Compute error between previous visual & corrected state
   - Smooth transition over time via interpolation
```

## Rollback Modes (`manager.rs:53-69`)

```rust
pub struct RollbackPolicy {
    pub state: RollbackMode,        // Check / Always / Disabled
    pub input: RollbackMode,        // Check / Always / Disabled
    pub max_rollback_ticks: u16,    // Default: 100
}
```

**State-based rollback:**
- `Always` (`rollback.rs:363-377`): Rollback whenever new replication messages received, no mismatch check
- `Check` (`rollback.rs:379-458`): Only rollback if mismatch detected

**Input-based rollback:**
- `Always` (`rollback.rs:466-485`): Rollback on new remote input to `last_confirmed_input.tick`
- `Check` (`rollback.rs:488-506`): Only rollback if input mismatch, to `earliest_mismatch_input.tick - 1`

## Rollback System Stages (`rollback.rs:84-106`)

| Stage | System | Purpose |
|-------|--------|---------|
| `Check` | `check_rollback` | Determine if rollback needed |
| `RemoveDisable` | `remove_prediction_disable` | Restore PredictionDisable entities |
| `Prepare` | `prepare_rollback` | Restore histories and component values |
| `Rollback` | `run_rollback` | Re-run FixedMain schedule N times |
| `EndRollback` | `end_rollback` | Post-rollback cleanup |
| `VisualCorrection` | correction system | Smooth visual transition |

## PredictionHistory Key Methods (`predicted_history.rs`)

| Method | Line | Purpose |
|--------|------|---------|
| `add_predicted(tick, value)` | 227 | Record locally computed value |
| `add_confirmed(tick, value)` | 259 | Record server-confirmed value |
| `add_confirmed_unchanged(tick)` | 241 | Mark unchanged entity as confirmed |
| `get(tick)` | 175 | Get value at or before tick |
| `get_confirmed_at(tick)` | 197 | Get confirmed value exactly at tick |
| `clear_predicted_from(tick)` | 362 | Clear predicted values >= tick, preserve confirmed |
| `pop_until_tick(tick)` | 339 | Pop and return value at tick, clear older |

## How Prediction Integrates with Replicon

1. Server spawns entity with `Replicate` + `PredictionTarget`
2. `PredictionTarget` causes `Predicted` marker to be replicated to client
3. On client, `Predicted` component triggers `add_prediction_history` observer
4. Each predicted component gets a `PredictionHistory<C>`
5. Replicon's marker system calls `write_history` / `remove_history` on component updates
6. `write_history` stores confirmed values and detects mismatches
7. `check_rollback` reads `ConfirmHistory`, `ServerMutateTicks`, and `StateRollbackMetadata`

## Component Registration

```rust
// In protocol setup (e.g., protocol.rs):
app.register_component::<MyComponent>(ChannelDirection::Bidirectional)
    .add_prediction(ComponentSyncMode::Full)      // Enable prediction
    .add_should_rollback(custom_check_fn)          // Optional custom rollback check
    .add_correction_fn(lerp_fn);                   // Optional visual correction
```

## Key Files Reference

| File | Content |
|------|---------|
| `lightyear_prediction/src/rollback.rs` | Core rollback logic, check_rollback, prepare_rollback |
| `lightyear_prediction/src/predicted_history.rs` | PredictionHistory<C>, PredictionState enum |
| `lightyear_prediction/src/manager.rs` | PredictionManager, RollbackPolicy, StateRollbackMetadata |
| `lightyear_prediction/src/plugin.rs` | System scheduling and plugin registration |
| `lightyear_prediction/src/correction.rs` | Visual smooth correction |
| `lightyear_prediction/src/registry.rs` | Component prediction registration, write_history/remove_history |
| `lightyear_prediction/src/archetypes.rs` | PredictedArchetypes resource |
| `lightyear_prediction/src/resource_history.rs` | Resource rollback support |
| `lightyear_core/src/prediction.rs` | Predicted marker component |
| `lightyear_replication/src/send.rs:298-348` | PredictionTarget -> Predicted hook |
| `lightyear_replication/src/PLAN.md` | Design notes and ServerMutateTicks proof |
