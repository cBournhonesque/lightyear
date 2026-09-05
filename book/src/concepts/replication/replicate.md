# Replication

You add the `Replicate` component to an entity to replicate it from the local `World` to the remote `World`.

```rust,ignore
commands.spawn((
    PlayerBundle::new(client_id, Vec2::ZERO),
    Replicate::to_clients(NetworkTarget::All),
));
```

`Replicate` decides who the entity goes to. There are two sibling components:
- `PredictionTarget` controls which clients run client-side prediction for the entity (they get a `Predicted` copy)
- `InterpolationTarget` controls which clients interpolate the entity (they get an `Interpolated` copy)

By default, every component on the entity that was registered with `app.component::<C>().replicate()` gets replicated, and every change gets sent.
The remote copy always converges to a consistent past state of the local entity: same set of components, same values, just delayed.

A few more pieces you can attach to a replicated entity:
- `ControlledBy` so the server can track which client owns the entity (the owning client gets a `Controlled` marker on its copy, which is how it knows where to put its `InputMarker`)
- `ReplicateLike` / `DisableReplicateHierarchy` to control whether children of the entity are replicated similarly to the parent
- Per-component behavior is chosen at registration time instead: `replicate_once()` for insert-only components, `replicate_filtered::<With<RigidBody>>()` to only replicate on matching entities, `replicate_with_priority(n)` for bandwidth management

Adding `Replicate` also adds the required `Replicating` marker. You can remove `Replicating` to pause replication
without changing the target. This can be useful when you want to despawn the entity on the server without replicating the despawn.
(e.g. an entity can be despawned immediately on the server, but needs to remain alive on the client to play a dying
animation). Reinsert `Replicating` to resume replication.

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
