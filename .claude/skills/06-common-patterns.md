# Common Patterns & Recipes

## Spawning Replicated Entities

### Server to All Clients
```rust
// Basic replication
stepper.server_app.world_mut().spawn((
    Replicate::to_clients(NetworkTarget::All),
));

// With prediction
stepper.server_app.world_mut().spawn((
    Replicate::to_clients(NetworkTarget::All),
    PredictionTarget::to_clients(NetworkTarget::All),
));

// With interpolation
stepper.server_app.world_mut().spawn((
    Replicate::to_clients(NetworkTarget::All),
    InterpolationTarget::to_clients(NetworkTarget::All),
));

// With control
let client_of_entity = stepper.client_of(0).id();
stepper.server_app.world_mut().spawn((
    Replicate::to_clients(NetworkTarget::All),
    ControlledBy {
        owner: client_of_entity,
        lifetime: Default::default(),
    },
));
```

### Client to Server
```rust
stepper.client_app().world_mut().spawn((
    Replicate::to_server(),
));
```

### Manual Targets
```rust
stepper.server_app.world_mut().spawn((
    Replicate::manual(vec![sender_entity]),
));
```

## Finding Replicated Entities on Client

After replication, find the client-side entity via the entity mapper:
```rust
use lightyear_messages::MessageManager;

stepper.frame_step(2);  // Allow replication to happen

let client_entity = stepper
    .client(0)
    .get::<MessageManager>()
    .unwrap()
    .entity_mapper
    .get_local(server_entity)
    .expect("entity should be replicated");
```

## Sending Messages

```rust
use lightyear_messages::prelude::{MessageSender, MessageReceiver};

// Client -> Server
stepper.client_mut(0)
    .get_mut::<MessageSender<StringMessage>>()
    .unwrap()
    .send::<Channel1>(StringMessage("Hello".to_string()));

// Server -> Client
stepper.client_of_mut(0)
    .get_mut::<MessageSender<StringMessage>>()
    .unwrap()
    .send::<Channel1>(StringMessage("Hello".to_string()));
```

## Checking Component Presence

```rust
// On a server entity
let entity_ref = stepper.server_app.world().entity(server_entity);
assert!(entity_ref.contains::<HasAuthority>());
assert!(entity_ref.contains::<Predicted>());

// On a client entity
let comp = stepper.client_app().world().get::<CompA>(client_entity);
assert_eq!(comp, Some(&CompA(1.0)));
```

## Authority Transfer

```rust
use lightyear_replication::prelude::*;
use lightyear_core::id::PeerId;

// Server gives authority to client 0
stepper.server_app.world_mut().trigger(GiveAuthority {
    entity: server_entity,
    peer: Some(PeerId::Netcode(0)),
});

// Client requests authority back
stepper.client_app().world_mut().trigger(RequestAuthority {
    entity: client_entity,
});
```

**Note**: Authority tests are currently disabled (module commented out in mod.rs).

## Entity Hierarchy

```rust
// Server spawns parent + child
let server_child = stepper.server_app.world_mut().spawn_empty().id();
let server_parent = stepper
    .server_app
    .world_mut()
    .spawn((
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::All),
    ))
    .add_child(server_child)
    .id();
```

## Waiting for Specific Conditions in Tests

The stepper's `init()` already calls `wait_for_connection()` and `wait_for_sync()`. For custom waits:

```rust
// Step multiple frames and check each time
for _ in 0..10 {
    stepper.frame_step(1);
    if some_condition(&stepper) {
        break;
    }
}
```

## Key Import Paths

```rust
// Replication
use lightyear_replication::prelude::*;  // Replicate, PredictionTarget, InterpolationTarget, etc.
use lightyear_replication::send::ReplicatedFrom;
use lightyear_replication::authority::HasAuthority;
use lightyear_replication::control::{Controlled, ControlledBy};

// Core types
use lightyear_core::prediction::Predicted;
use lightyear_core::interpolation::Interpolated;
use lightyear_core::id::PeerId;

// Connection
use lightyear_connection::network_target::NetworkTarget;
use lightyear_connection::client::Connected;
use lightyear_connection::host::{HostClient, HostServer};
use lightyear_connection::server::Started;

// Messages
use lightyear_messages::MessageManager;
use lightyear_messages::prelude::{MessageSender, MessageReceiver, EventSender};

// Sync
use lightyear_sync::prelude::{InputTimeline, IsSynced};

// Prediction
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_prediction::manager::PredictionManager;
```

## Adding a New Test Module

1. Create the test file: `lightyear_tests/src/<category>/<name>.rs`
2. Add `mod <name>;` to `lightyear_tests/src/<category>/mod.rs`
3. Use `use crate::stepper::*;` and `use test_log::test;`
4. Run with: `cargo test -p lightyear_tests -- --test-threads=1 <category>::<name>`

## Common Gotchas

- **Always `--test-threads=1`**: Tests share bevy's ComputeTaskPool and crash if parallel
- **Host client is in server_app**: Don't look for it in `client_apps`
- **`frame_step(1)` vs `frame_step(2)`**: Some operations need 2 frames (e.g., spawn -> replicate -> receive)
- **Entity indices differ**: Server and client entity IDs are different. Always use the entity mapper.
- **`client_of(i)` indices**: These map to `client_apps[i]` for netcode clients, but the host client has no corresponding `client_of` entry
- **`flush()` is not automatic**: If you insert components and need them visible immediately (before a frame step), call `stepper.flush()`
