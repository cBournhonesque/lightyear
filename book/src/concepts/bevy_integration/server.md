# Server

A server is an entity with the `Server` role marker, a connection component such as `NetcodeServer`,
and an accepting transport component such as `UdpEndpoint`. `ServerUdpIo` is shorthand for
`UdpEndpoint` plus `Server`.

`Server` requires `Endpoint`, which owns the per-peer link collection and optional receive
conditioner. A P2P peer can own an `Endpoint` without being a `Server`.

Every accepted link has `LinkOf { endpoint }` pointing at its owning endpoint. In an application
that also has P2P endpoints, filter for the `Server` role before applying server-specific behavior:

```rust,ignore
pub(crate) fn handle_new_client(
    trigger: On<Add<LinkOf>>,
    links: Query<&LinkOf>,
    servers: Query<(), With<Server>>,
    mut commands: Commands,
) {
    let Ok(link_of) = links.get(trigger.entity) else {
        return;
    };
    if !servers.contains(link_of.endpoint) {
        return;
    }
    commands.entity(trigger.entity).insert((
        ReplicationSender,
        Name::from("Client"),
    ));
}
```

At that point the client is only *linked*, not *connected*: netcode authentication still has to succeed. Only when the `Connected` component is added is the client real, and that's where game behaviour starts (spawn a player, etc.).

The server's per-frame jobs mirror the client's: read inputs and step simulation in `FixedUpdate`, replicate the world in `PostUpdate` (`ReplicationSystems::Send`) at the rate set by the `ReplicationMetadata` resource.
