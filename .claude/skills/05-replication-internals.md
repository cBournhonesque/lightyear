# Replication Internals

## Replication Targets

Everything revolves around the `ReplicationTarget<T>` component and the `ReplicationTargetT` trait.

### Component Hierarchy

```
Replicate              = ReplicationTarget<()>         -- Entity-level replication
PredictionTarget       = ReplicationTarget<Predicted>  -- Mark for client-side prediction
InterpolationTarget    = ReplicationTarget<Interpolated> -- Mark for interpolation
```

All three are components added on the **server** side.

### Replicate Modes (`send.rs:40-60`)

```rust
pub enum ReplicationMode {
    SingleSender,   // Find single ReplicationSender (default for server->clients)
    SingleClient,   // Client sends to server
    SingleServer,   // Server sends to all clients
    Sender(Entity), // Specific entity
    Target(NetworkTarget), // Network target
    Manual(Vec<Entity>),   // Explicit entity list
}
```

Common usage:
```rust
Replicate::to_clients(NetworkTarget::All)   // Server -> all clients
Replicate::to_server()                       // Client -> server
Replicate::manual(vec![entity1, entity2])    // Specific targets
```

### What Happens on Insert (`send.rs:434-567`)

When `Replicate` (or `PredictionTarget`/`InterpolationTarget`) is inserted:

1. Hook fires and gets the visibility bit from the resource
2. `T::pre_insert()` runs (for `()`, updates `AuthorityBroker` on server)
3. Queued command processes the replication mode:
   - Finds sender entity(ies) based on `ReplicationMode`
   - Sets visibility bits on `ClientVisibility` for each sender
   - Calls `T::update_replicate_state()` to track per-sender state
   - Calls `T::post_insert()` to add marker components

### ReplicationState (`send.rs:101-169`)

Per-entity component tracking replication metadata:
```rust
pub struct ReplicationState {
    per_sender_state: EntityIndexMap<PerSenderReplicationState>,
}
pub struct PerSenderReplicationState {
    pub authority: Option<bool>,  // Some(true) = has authority, Some(false) = no authority
}
```

### Host-Server Specifics

In host-server mode (server + client in same app):

- `post_insert` for `()` inserts `HasAuthority` (server has authority) and `ReplicatedFrom { receiver: host_sender }` (host-client sees it as replicated)
- `post_insert` for `Predicted` inserts `Predicted` marker (host-client gets prediction)
- `post_insert` for `Interpolated` inserts `Interpolated` marker
- `ControlledBy` has `#[require(Controlled)]` which auto-inserts `Controlled` on spawn

### Visibility System

Each `ReplicationTarget<T>` has an associated `VisibilityBit`:
- `Replicate` -> `ReplicateBit` (entity-scope)
- `PredictionTarget` -> `PredictedBit` (component-scope)
- `InterpolationTarget` -> `InterpolatedBit` (component-scope, shares `ReplicateBit`)

Bits are registered with replicon's `FilterRegistry` and toggled on/off via `ClientVisibility::set()`.

## Authority System

### Components

| Component | Location | Purpose |
|-----------|----------|---------|
| `HasAuthority` | `lightyear_replication/src/authority.rs:107` | Marker: this peer has authority |
| `AuthorityBroker` | `lightyear_replication/src/authority.rs` | Tracks entity ownership per PeerId |
| `AuthorityTransfer` | `lightyear_replication/src/authority.rs` | Policy: can authority be stolen? |

### Authority Events

| Event | Purpose |
|-------|---------|
| `GiveAuthority { entity, peer }` | Transfer authority to another peer |
| `RequestAuthority { entity }` | Request authority from current owner |

**Note**: Authority tests are commented out (`lightyear_tests/src/client_server/authority.rs` exists but `mod authority` is commented in `mod.rs`). The system exists but isn't fully integrated with replicon yet.

## Control System (`control.rs`)

```rust
#[derive(Component)]
#[require(Controlled)]              // Auto-inserts Controlled on spawn
#[relationship(relationship_target = ControlledByRemote)]
pub struct ControlledBy {
    pub owner: Entity,              // Entity with ReplicationSender
    pub lifetime: Lifetime,         // SessionBased or Persistent
}

#[derive(Component)]
pub struct Controlled;              // Receiver-side marker

#[derive(Component)]
#[relationship_target(relationship = ControlledBy)]
pub struct ControlledByRemote(Vec<Entity>);
```

`Controlled` is replicated. When a client sees an entity with `Controlled`, it knows it controls that entity.

## Transport Bridge

### Server Side (`server.rs`)

```
on_client_connected: Connected added -> insert replicon ConnectedClient + NetworkId
sync_server_state: Started present -> ServerState::Running
receive_server_packets: Read from client_channels -> ServerMessages
send_server_packets: Drain ServerMessages -> write to server_channels
sync_entity_map: Sync replicon ServerEntityMap -> lightyear MessageManager.entity_mapper
```

### Client Side (`client.rs`)

```
sync_client_state: Connected present -> ClientState::Connected
receive_client_packets: Read from server_channels -> ClientMessages
send_client_packets: Drain ClientMessages -> write to client_channels
sync_entity_map: Sync replicon ServerEntityMap -> lightyear MessageManager.entity_mapper
```

## Host-Server Connection (`lightyear_connection/src/host.rs`)

```rust
pub struct HostClient {
    pub buffer: Vec<(Bytes, TypeId)>,  // Local message loopback
}

pub struct HostServer {
    client: Entity,                     // The host client entity
}
```

On `Connect` event:
- Inserts `Connected`, `LocalId(PeerId::Local(0))`, `RemoteId(PeerId::Local(0))`, `HostClient`
- Server gets `HostServer { client }` marker

Host clients live in the **server app**, not a separate client app.

## Component Registration

Components are registered for replication in `protocol.rs` (for tests) or user code:

```rust
app.register_component::<MyComp>(ChannelDirection::Bidirectional)
    .add_prediction(ComponentSyncMode::Full)
    .add_interpolation_fn(linear_interpolation::<MyComp>);

// Replicon-level registration (done automatically by register_component):
app.replicate::<MyComp>();
```

The `Predicted` and `Interpolated` markers are also registered:
```rust
app.replicate::<Predicted>();      // send.rs:622
app.replicate::<Interpolated>();   // send.rs:624
```
