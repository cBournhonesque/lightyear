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
copies on the client. In Bevy 0.19, resources are components stored on Bevy's resource entities, and Lightyear relies on
Replicon's resource replication API for this.

To replicate a `Resource`:
- Define your resource and register it with Replicon on both peers:
    ```rust
    use bevy_replicon::prelude::AppRuleExt;

    #[derive(Resource, Serialize, Deserialize)]
    pub struct MyResource(pub f32);

    pub fn plugin(app: &mut App) {
        app.replicate_resource::<MyResource>();
    }
    ```
- Insert the resource on the server:
    ```rust
    commands.insert_resource(MyResource(1.0));
    ```

Replicon also provides `replicate_resource_once`, `replicate_resource_as`, and diff-based variants. If a client creates a
local copy of the same resource before the server replicates it, use Replicon's resource-entity mapping support to avoid
spawning a duplicate resource entity.
