# Server

A server is an entity with the `Server` marker component, a connection component (`NetcodeServer`) and a server IO component (`ServerUdpIo`, ...).

It doesn't hold connections itself. Every time a new link is established with a remote peer, lightyear spawns a child entity with `LinkOf` pointing at the server. You customize each connection with an observer:

```rust,ignore
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        ReplicationSender,
        Name::from("Client"),
    ));
}
```

At that point the client is only *linked*, not *connected*: netcode authentication still has to succeed. Only when the `Connected` component is added is the client real, and that's where game behaviour starts (spawn a player, etc.).

The server's per-frame jobs mirror the client's: read inputs and step simulation in `FixedUpdate`, replicate the world in `PostUpdate` (`ReplicationSystems::Send`) at the rate set by the `ReplicationMetadata` resource.
