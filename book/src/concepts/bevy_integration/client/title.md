# Client

A Lightyear client is just a Bevy entity with client, link, transport, timeline, message, and replication components attached by the plugins you choose.

The client has three jobs:

- connect to the server
- send local intent, usually inputs or messages
- apply the server's replicated world state

Replication received from the server is applied before your fixed-tick gameplay systems run. On the receiver side, replicated entities are marked with `Remote`.

For player input, write the input in `FixedPreUpdate` in the input write set. The input plugin buffers it by tick and sends it to the server. Your local prediction systems can then read the same input during `FixedUpdate`.

```rust,ignore
app.add_systems(
    FixedPreUpdate,
    buffer_input.in_set(InputSystems::WriteClientInputs),
);

app.add_systems(FixedUpdate, predicted_movement);
```

Do not make the client authoritative by spawning replicated entities and expecting the server to accept them. Current entity replication is server to client. If the client wants something to happen, send input or a message, let the server decide, then replicate the result back.
