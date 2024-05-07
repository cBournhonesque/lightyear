# Client replication

It is also possible to use lightyear to replicate entities from the client to the server.
There are different possibilities. Note that regardless of the approach taken, you will
need to explicitly enable sending replication from the client, and receiving replication
on the server:

```rust
let client_config = client::ClientConfig {
    /* other fields */
    replication: lightyear::client::replication::ReplicationConfig {
        enable_send: true,
        enable_receive: true,
    },
    ..default()
};

let server_config = server::ServerConfig {
    /* other fields */
    replication: lightyear::server::replication::ReplicationConfig {
        enable_send: true,
        enable_receive: true,
    },
    ..default()
};
```

For a detailed example, please see the `client_replication` example.

## Client authoritative

To replicate a client-entity to the server, it is exactly the same as for a server-entity.
Just add the `Replicate` component to the entity and it will be replicated to the server:
```rust
fn handle_connection(
    mut connection_event: EventReader<ConnectEvent>,
    mut commands: Commands,
) {
    for event in connection_event.read() {
        let local_client_id = event.client_id();
        commands.spawn((
            /* your other components here */
            Replicate {
                replication_target: NetworkTarget::All,
                interpolation_target: NetworkTarget::AllExcept(vec![local_client_id]),
                ..default()
            },
        ));
    }
}
```

Note that `prediction_target` and `interpolation_target` will be unused as the server doesn't do any 
prediction or interpolation.

If you want to then broadcast that entity to other clients, you will have to add a `Replicate` component
on the server entity. This should generally happen in the `ServerReplicationSet::ClientReplication` SystemSet on the server, so that it happens right 
after receiving the client entity.

Be careful to not replicate the entity back to the original client, as it would create a duplicate entity on the client.

Example flow:

```mermaid
---
title: Client Authoritative
---
sequenceDiagram
    participant Client1
    participant Server
    participant Client2
    participant Client3
    Client1->>Server: Connect()
    Server->>Client1: Connected
    Client1->>Client1: ConnectEvent
    Client1->>Client1: SpawnPredicted(PlayerID: 1)
    Client1->>Server: Replicate(PlayerID: 1)
    Server-->>Client2: Replicate(PlayerID: 1)
    Client2->>Client2: SpawnConfirmed(PlayerID: 1)
    Server-->>Client3: Replicate(PlayerID: 1)
    Client3->>Client3: SpawnConfirmed(PlayerID: 1)
```

## Pre-spawned predicted entities

Sometimes you might want to spawn a predicted entity on the client, but then replicate it to the server
and let the server get control over the entity (server-authoritative).

For example you might have a predicted character that spawns another predicted entity (a projectile that other clients
should see, for example).

You could wait for the user input to reach the server, then for the server to spawn the projectile and for the projectile
to be replicated back to the client, but that would mean a delay of 1-RTT on the client to see the projectile when they pres
a button.

A solution is to spawn the predicted projectile on the client; but then replicate back to the server.
When the server replicates back the projectile, it creates a Confirmed entity on the client, but
**will re-use the existing predicted projectile as the Predicted entity**.

The way to do this is to add a `PrePredicted` component on the client entity that you want to predict.
On the server, to replicate back the entity to the client, you will need to manually add a `Replicate` component to the entity
to specify to which clients you want to rebroadcast it.
**Note that you must add the `Replicate` component in the `ServerReplicationSet::ClientReplication` `SystemSet` for proper handling of 
pre-spawned predicted entities!**

When the server replicates back the entity, the client will check if the entity has a `PrePredicted` component.
If not present, that means this is not a pre-spawned Predicted entity, and the client will spawn both the Confirmed 
and Predicted entities.
If it's present, the client will spawn a new Confirmed entity, but will re-use the `entity` as the Predicted entity.


Note that pre-spawned predicted entities will give authority to the server's entity immediately, the client to server
replication will stop immediately after the initial replication, and the server entity should be the authoritative one.

Example flow:

```mermaid
---
title: Client PrePredicted
---
sequenceDiagram
    participant Client1
    participant Server
    participant Client2
    participant Client3
    Client1->>Server: Connect()
    Server->>Client1: Connected
    Client1->>Client1: ConnectEvent
    Client1->>Client1: SpawnPredicted(PlayerID: 1)
    Client1->>Server: Replicate(PlayerID: 1)
    Server-->>Client1: Replicate(PlayerID: 1)
    Client1->>Client1: SpawnConfirmed(PlayerID: 1)
    Server-->>Client2: Replicate(PlayerID: 1)
    Client2->>Client2: SpawnConfirmed(PlayerID: 1)
    Server-->>Client3: Replicate(PlayerID: 1)
    Client3->>Client3: SpawnConfirmed(PlayerID: 1)
```


Notes to myself:
- one thing to be careful for is that we want to immediately stop replicating updates from the pre-spawned predicted entity
  to the server; because that entity should be server-authoritative. Right after the first time the `Send` `SystemSet` runs,
  run `clean_prespawned_entities` to remove `Replicate` from those entities.
- another thing we have to be careful of is this: let's say we receive on the server a pre-predicted entity with `ShouldBePredicted(1)`.
  Then we rebroadcast it to other clients. If an entity `1` already exists on other clients; we will start using that entity
  as our Prediction target! That means that we should:
  - even if pre-spawned replication, require users to set the `prediction_target` correctly
  - only broadcast `ShouldBePredicted` to the clients who have `prediction_target` set.
  - be careful that `ShouldBePredicted` can be added once during spawn, and once from replication. In that case the second one should win out!
