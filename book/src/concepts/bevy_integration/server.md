# Server

The server is the authority for entity replication. It receives links from clients, accepts inputs or messages, updates the game world, and replicates the resulting state back to clients.

When a client connects, Lightyear creates a `ClientOf` entity for that connection. This entity is the per-client link on the server. Add the components you need to that link entity, such as `ReplicationSender`, message senders, message receivers, or per-client metadata.

```rust,ignore
fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert((ReplicationSender, Name::new("Client")));
}
```

Wait for `Connected` before spawning gameplay state for that client. A link can exist before the connection is accepted.

```rust,ignore
fn handle_connected(
    trigger: On<Add, Connected>,
    remotes: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = remotes.get(trigger.entity) else {
        return;
    };

    commands.spawn((
        PlayerBundle::new(client_id.0),
        Replicate::to_clients(NetworkTarget::All),
    ));
}
```

From there the server is normal Bevy code. Read inputs in fixed simulation, apply your game rules, and let replication send the changed registered components.

Client to server entity replication is not supported right now. Use inputs, messages, or remote events for client requests.
