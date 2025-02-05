# Replication

You can use the `Replicate` bundle to initiate replicating an entity from the local `World` to the remote `World`.

It is composed of multiple smaller components that each control an aspect of replication:
- `ReplicationTarget` to decide who to replicate to
- `VisibilityMode` to enable interest management
- `ControlledBy` so the server can track which entity is owned by each client
- `ReplicationGroup` to know which entity updates should be sent together in the same message
- `ReplicateHierarchy` to control if the children of an entity should also be replicated
- `DisabledComponents` to control which specific components will be replicated for a given entity (if you only want to replicate a subset of the registered components)
- `ReplicateOnceComponent<C>` to specify that some components should not replicate updates, only inserts/removals
- `OverrideTargetComponent<C>` to override the replication target for a specific component

By default, every component in the entity that is part of the `ComponentRegistry` will be replicated. Any changes in
those components will be replicated.
However the entity state will always be 'consistent': the remote entity will always contain the exact same combination
of components as the local entity, even if it's a bit delayed.

You can remove the `ReplicationTarget` component to pause the replication. This can be useful when you want to despawn the
entity on the server without replicating the despawn.
(e.g. an entity can be despawned immediately on the server, but needs to remain alive on the client to play a dying
animation)

You can find some of the other usages in the [advanced_replication](../advanced_replication/title.md) section.


### Replicating resources

You can also replicate bevy `Resource`s. This is useful when you want to update a `Resource` on the server and keep synced
copies on the client. This only works for `Resources` that also implement `Clone`, and should be limited to resources which are cheap to clone.

To replicate a `Resource`:
- Ensure that you have a [`Channel`](../reliability/channels.md) that the resource can be replicated over.
  This can be done by creating a public struct that derives `Channel` and adding it to your protocol.
- Next, define your resource and register it:
    ```rust
    #[derive(Channel)]
    pub struct Channel1;

    #[derive(Resource, Clone, Serialize, Deserialize)]
    pub struct MyResource(pub f32);

    pub fn plugin(app: &mut App) {
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..Default::default()
        });

        app.register_resource::<MyResource>(ChannelDirection::ServerToClient);
    }
    ```
- Finally, to replicate the `Resource`, you can use the `commands.replicate_resource::<R>(replicate)` method.
  You will need to provide the channel and a `NetworkTarget` to specify how the replication should be done
  (e.g. to which clients should the resource be replicated):
    ```rust
    commands.replicate_resource::<MyResource, Channel1>(NetworkTarget::All);
    // This will be replicated to all clients; any changes to the resource will also be replicated
    commands.insert_resource(MyResource(1.0));
    ```
- To stop replicating a `Resource`, you can use the `commands.stop_replicate_resource::<R>()` method.
  Note that this won't delete the resource from the client, but it will stop updating it.