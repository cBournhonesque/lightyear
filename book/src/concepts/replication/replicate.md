# Replication

You can use the `Replicate` component to initiate replicating an entity from the local `World` to the remote `World`.

By default, every component in the entity that is part of the `ComponentProtocol` will be replicated. Any changes in
those components
will be replicated.
However the entity state will always be 'consistent': the remote entity will always contain the exact same combination
of components as the local entity, even if it's a bit delayed.

You can remove the `Replicate` component to pause the replication. This can be useful when you want to despawn the
entity on the server without replicating the despawn.
(e.g. an entity can be despawned immediately on the server, but needs to remain alive on the client to play a dying
animation)

There are a lot of additional fields on the `Replicate` component that let you control exactly how the replication
works.
For example, `per_component_metadata` lets you fine-tune the replication logic for each component (exclude a component
from being replicated, etc.)

You can find some of the other usages in the [advanced_replication](../concepts/advanced_replication/title.md) section.


### Replicating resources

You can also replicate bevy `Resources`. This is useful when you want to update a `Resource` on the server and keep synced
copies on the client. This only works for `Resources` that also implement `Clone`, and should be limited to resources which are cheap to clone.

- First, you will need to add the component `ReplicateResource<R>` to your `ComponentProtocol`
- Then, to replicate a `Resource`, you can use the `commands.replicate_resource::<R>(replicate)` method. You will need to provide
an instance of the `Replicate` struct to specify how the replication should be done (e.g. to which clients should the resource
be replicated). To stop replicating a `Resource`, you can use the `commands.stop_replicate_resource::<R>()` method. Note that
this won't delete the resource from the client, but it will stop updating it.