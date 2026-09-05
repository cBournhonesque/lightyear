# Bandwidth management

By default, lightyear sends everything that's ready every time the replication timer fires, without any regard for the bandwidth available to the client.

But in some situations you might want to limit the bandwidth used by the client or the server, for example to limit
server traffic costs, or because the client's connection cannot handle a very high bandwidth.

This page will explain how to do that. There are several options to choose from.

## Limiting the number of replication objects

The simplest thing you can do is to carefully choose which entities and components you need to replicate.
For example, rendering-related components (particles, assets, etc.) do not need to spawned on the server and replicated to the client.
They can be created on the client and only the necessary information (position, rotation, etc.) can be replicated.

This also saves CPU costs on the server.

## Updating the send interval

Another thing you can do is to update the replication interval. The `ReplicationMetadata` resource controls how often replication updates go out:

```rust,ignore
app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
```

A longer interval means the `Send` systems run less often, which saves both bandwidth and server CPU. The tradeoff is that clients see updates less frequently (which is exactly what prediction and interpolation are for).

## Capping bandwidth per link

You can put a hard cap on how many bytes go through a connection by adding a configured `Transport` to the link entity:

```rust,ignore
commands.entity(client_link).insert((
    ReplicationSender,
    // limit to 3KB/s
    Transport::new(PriorityConfig::new(3000)),
));
```

Once the cap is hit, something has to give. That's where priorities come in.

## Prioritizing entities and components

When there are more updates ready than fit in the budget, lightyear sends the most important ones first and defers the rest. Importance comes from two places:

- per entity, with the `ReplicatePriority` component (see the [priority example](https://github.com/cBournhonesque/lightyear/tree/main/examples/priority), where the middle row updates less often than the edges):

```rust,ignore
commands.spawn((
    position,
    ReplicatePriority(priority),
    Replicate::to_clients(NetworkTarget::All),
));
```

- per component type, at registration: `app.component::<C>().replicate_with_priority(n)`.

Only the relative values matter: an entity with priority 10 is sent twice as often as one with priority 5. Deferred updates aren't dropped, they just wait for the next send (unlike unreliable messages, entity updates keep being retried until the remote world is consistent).
