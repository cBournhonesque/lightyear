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

You can find some of the other usages in
the [advanced_replication](https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/title.html)
section.