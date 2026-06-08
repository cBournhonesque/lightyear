# Replication

Replication is how the server keeps clients in sync with the ECS world.

In the current backend, Lightyear uses `bevy_replicon` for entity replication and visibility. Lightyear still owns the transport bridge, connection setup, timelines, inputs, prediction helpers, and interpolation helpers, but the low-level "which component changed and how do I apply it remotely?" work goes through Replicon.

The supported direction today is server to client. The server spawns or updates an entity, marks it for replication, and clients receive a matching remote entity with the registered components. Client to server entity replication is not supported yet. For client intent, use inputs, messages, or remote events and let the server change the replicated world.

The basic flow is:

1. Register every component that can be replicated.
2. Add `ReplicationSender` to each server-side client link.
3. Spawn server-owned entities with `Replicate::to_clients(...)`.
4. Use visibility and rooms to decide which clients can see each entity.

Prediction and interpolation do not change the ownership model. They are client-side behavior layered on top of the server stream. The client still receives server state; Lightyear can then use marker components and history buffers to smooth it or predict ahead.
