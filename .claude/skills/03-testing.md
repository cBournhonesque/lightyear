# Testing Guide

## Running Tests

**CRITICAL: Always use `--test-threads=1`**. Tests crash with SIGABRT when run multi-threaded due to bevy's shared `ComputeTaskPool`.

```bash
# Run all tests
cargo test -p lightyear_tests -- --test-threads=1

# Run a specific test module
cargo test -p lightyear_tests -- --test-threads=1 host_server::replication

# Run a specific test
cargo test -p lightyear_tests -- --test-threads=1 test_setup_host_server

# CI uses tarpaulin for coverage
cargo tarpaulin --locked -p lightyear_tests --engine llvm --out lcov -- --test-threads=1
```

## Test Infrastructure

### Crate: `lightyear_tests`

All tests live in `lightyear_tests/src/`. The crate is feature-gated on `test_utils` (enabled by default).

### Module Structure

```
lightyear_tests/src/
  lib.rs
  protocol.rs          -- Test protocol: components, messages, channels, inputs
  stepper.rs           -- ClientServerStepper test harness
  client_server/       -- Standard client-server tests
    mod.rs
    base.rs            -- Setup validation
    avian/             -- Physics replication
    connection.rs      -- Connection lifecycle
    hierarchy.rs       -- Entity hierarchy replication
    input/             -- Input sync (bei, native; leafwing commented out)
    messages.rs        -- Message send/receive
    prediction/        -- Prediction, rollback, prespawn, history, despawn
    replication.rs     -- Entity/component replication
    visibility.rs      -- Client visibility control
    # authority.rs     -- COMMENTED OUT (replicon migration)
    # delta.rs         -- COMMENTED OUT (removed)
    # replication_advanced.rs -- COMMENTED OUT
  host_server/         -- Host-server (listen server) tests
    mod.rs
    base.rs
    messages.rs
    input/
    replication.rs
  multi_server/        -- Multi-server tests
    steam.rs           -- Feature-gated on steam
```

### ClientServerStepper (`stepper.rs`)

The test harness that creates controlled client-server environments.

```rust
pub struct ClientServerStepper {
    pub client_apps: Vec<App>,              // Separate client bevy Apps
    pub server_app: App,                    // Server bevy App
    pub client_entities: Vec<Entity>,       // Client link entities (in client apps)
    pub server_entity: Entity,              // Server entity
    pub client_of_entities: Vec<Entity>,    // Per-client entities on server
    pub host_client_entity: Option<Entity>, // Host client entity (in server app)
    pub frame_duration: Duration,
    pub tick_duration: Duration,
    // ...
}
```

**Configuration presets:**

```rust
// Single netcode client (most common)
StepperConfig::single()

// Host server: 1 host client + 1 netcode client
StepperConfig::host_server()

// N netcode clients
StepperConfig::with_netcode_clients(n)

// Custom client/server types
StepperConfig::from_link_types(vec![ClientType::Host, ClientType::Netcode], ServerType::Netcode)
```

**Key methods:**

| Method | Purpose |
|--------|---------|
| `from_config(config)` | Create stepper, connect and sync all clients |
| `frame_step(n)` | Advance n frames (clients update first, then server) |
| `frame_step_server_first(n)` | Advance n frames (server updates first) |
| `tick_step(n)` | Advance n ticks |
| `flush()` | Flush command buffers on all apps |
| `server_app.world_mut()` | Direct access to server world |
| `client_app()` | Access first client app (asserts exactly 1) |
| `client_apps[i]` | Access i-th client app |
| `server()` / `server_mut()` | EntityRef/EntityWorldMut for server entity |
| `client(i)` / `client_mut(i)` | EntityRef/EntityWorldMut for client i |
| `client_of(i)` / `client_of_mut(i)` | EntityRef/EntityWorldMut for server-side client-of entity i |
| `host_client()` / `host_client_mut()` | EntityRef/EntityWorldMut for host client |
| `host_client_entity` | Option<Entity> for host client |
| `disconnect_client()` | Remove last client |

**Important**: Host clients live in the `server_app`, not in a separate client app. In `StepperConfig::host_server()`, `client_apps` has length 1 (the netcode client), and the host client entity is accessed via `stepper.host_client_entity`.

### Protocol (`protocol.rs`)

Test components registered for replication:

| Component | Type | Features |
|-----------|------|----------|
| `CompA(f32)` | Simple | Basic replication |
| `CompS(String)` | String | Basic replication |
| `CompFull(f32)` | Full | Prediction + linear interpolation |
| `CompSimple(f32)` | Simple | Basic replication |
| `CompCorr(f32)` | Correction | Prediction + correction + interpolation + Diffable |
| `CompMap(Entity)` | Entity mapping | Entity reference replication |
| `CompDelta(usize)` | Delta | Delta compression support |
| `CompNotNetworked(f32)` | Non-replicated | Rollback component only |

Messages: `StringMessage(String)`, `EntityMessage(Entity)`
Triggers: `StringTrigger(String)`, `EntityTrigger { entity: Entity }`
Channels: `Channel1` (UnorderedUnreliable), `Channel2` (UnorderedUnreliableWithAcks)
Inputs: `NativeInput(i16)`, `LeafwingInput1`, `LeafwingInput2`, `BEIContext`, `BEIAction1`

### Typical Test Pattern

```rust
use crate::stepper::*;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_replication::prelude::*;
use test_log::test;

#[test]
fn test_something() {
    // 1. Create stepper
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    // 2. Spawn entities on server
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((Replicate::to_clients(NetworkTarget::All),))
        .id();

    // 3. Step frames to let replication happen
    stepper.frame_step(2);

    // 4. Find replicated entity on client via entity mapper
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity should be replicated");

    // 5. Assert
    assert!(stepper.client_app().world().get::<SomeComponent>(client_entity).is_some());
}
```

### Host-Server Test Pattern

```rust
#[test]
fn test_host_server_something() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(1);

    // Host client entity is in the SERVER app
    let host_client_entity = stepper.host_client_entity.unwrap();

    // Check components on the server-side entity
    let entity_ref = stepper.server_app.world().entity(server_entity);
    assert!(entity_ref.contains::<HasAuthority>());
    assert!(entity_ref.contains::<Predicted>());
}
```

### Common Imports

```rust
use crate::stepper::*;
use crate::protocol::*;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prediction::Predicted;
use lightyear_core::interpolation::Interpolated;
use lightyear_replication::authority::HasAuthority;
use lightyear_replication::control::{Controlled, ControlledBy};
use lightyear_replication::prelude::*;
use lightyear_replication::send::ReplicatedFrom;
use lightyear_messages::MessageManager;
use test_log::test;
```

### Debugging Tests

Tests use `test-log` crate. Set `RUST_LOG` to see tracing output:

```bash
RUST_LOG=info cargo test -p lightyear_tests -- --test-threads=1 test_name
```

The stepper logs frame steps with `info!` and wraps client/server updates in `error_span!("client")` / `error_span!("server")` for structured logging.
