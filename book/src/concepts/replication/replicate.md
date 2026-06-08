# Replicate

Add `Replicate` to a server entity when you want that entity to appear on clients.

```rust,ignore
commands.spawn((
    PlayerBundle::new(client_id),
    Replicate::to_clients(NetworkTarget::All),
));
```

`Replicate` is Lightyear's convenience target component on top of `bevy_replicon` visibility. It tells Replicon which client links should receive the entity.

On the server, every client link that should receive replicated state needs a `ReplicationSender`:

```rust,ignore
fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}
```

Once the link is connected, a replicated entity can be sent to all clients, one client, all except one client, or a manually selected set of link entities:

```rust,ignore
Replicate::to_clients(NetworkTarget::All);
Replicate::to_clients(NetworkTarget::Single(client_id));
Replicate::to_clients(NetworkTarget::AllExceptSingle(client_id));
Replicate::manual(vec![client_link_entity]);
```

Only registered components are sent. Components that are not registered stay local to the server.

## Receiver-side entities

On the client, entities that came from the server are remote entities. Use the `Remote` marker when you want to query for them on the client.

```rust,ignore
fn on_remote_player(players: Query<Entity, (With<Remote>, Added<PlayerId>)>) {
    for entity in &players {
        // Add rendering, UI markers, local-only components, etc.
    }
}
```

## Visibility

Replication target is the coarse filter. Visibility is the fine filter.

You can show or hide an entity for a specific client link:

```rust,ignore
commands.lose_visibility(player_entity, client_link_entity);
commands.gain_visibility(player_entity, client_link_entity);
```

You can also use `Rooms` for semi-static interest management:

```rust,ignore
app.add_plugins(RoomPlugin);

let room = app.world_mut().resource_mut::<RoomAllocator>().allocate();

commands.spawn((
    Replicate::default(),
    Rooms::single(room),
));
```

If the client and entity share a room, the entity is visible to that client. If they no longer share a room, Replicon will stop sending it to that client and the client will despawn its remote copy.

## Pausing and despawning

Removing `Replicate` used to be a way to pause replication without despawning the remote entity. With the Replicon backend this behavior is different: removing the replicated marker can be interpreted as a despawn on the receiver. If you need to hide an entity temporarily, prefer visibility (`lose_visibility`) or a gameplay-level disabled component until a dedicated pause/resume API exists.

To despawn an entity for clients, despawn it on the server while it is visible to those clients. To remove it only for some clients, change visibility for those clients.

## Resources

Resource replication is still useful for global state such as match phase, scoreboards, or map metadata. Keep replicated resources small and cheap to clone.

```rust,ignore
#[derive(Resource, Clone, Serialize, Deserialize)]
pub struct MatchState {
    pub phase: MatchPhase,
}

app.register_resource::<MatchState>(ChannelDirection::ServerToClient);

commands.replicate_resource::<MatchState, ReliableChannel>(NetworkTarget::All);
commands.insert_resource(MatchState {
    phase: MatchPhase::Warmup,
});
```

For frequently changing per-entity state, prefer components. Resources are best for state that really is global.
