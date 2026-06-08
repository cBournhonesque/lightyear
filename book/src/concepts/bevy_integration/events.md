# Events

Lightyear uses Bevy observers and events for most "something changed" moments.

Connection state is component-driven. For example, adding `Connected` means the peer is ready, and adding `Disconnected` means the link is no longer active. Observers are a natural way to react to that:

```rust,ignore
fn on_connected(trigger: On<Add, Connected>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(MyConnectionState);
}
```

Messages are different. A registered message type is read from a `MessageReceiver<M>` or sent through a `MessageSender<M>` / `ServerMultiMessageSender`. Use messages when you want a typed payload that is not simply "this component changed on this entity".

Remote events are useful when you want event-style code on the receiver. They still travel through the networking layer, but the receiving side can handle them like Bevy events or observers.

As a rule of thumb:

- use replicated components for durable world state
- use inputs for player intent that belongs to a simulation tick
- use messages for requests, commands, chat, loading state, or other typed payloads
- use events for one-shot notifications that fit Bevy's event style

If a client wants to change replicated world state, send intent to the server. The server should make the actual change, then replicate the result.
