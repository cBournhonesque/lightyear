# Client replication

Client to server entity replication is not supported right now.

With the current Replicon-backed replication, the supported entity replication path is server to client.

That does not mean clients cannot ask for things. They should send intent.

## Use inputs for simulation

Use inputs when the request belongs to a simulation tick: movement, aiming, jump, fire, ability buttons, and similar controls.

The server reads the input on the right tick, applies game rules, and replicates the resulting world state.

## Use messages for commands

Use messages when the request is not a continuous input stream:

- choose a character
- request a respawn
- buy an item
- start matchmaking
- spawn a cosmetic preview
- send chat

The server should validate the message before changing replicated state.

```rust,ignore
#[derive(Serialize, Deserialize, Clone)]
pub struct RequestRespawn;

app.register_message::<RequestRespawn>()
    .add_direction(NetworkDirection::ClientToServer);
```

## Server rebroadcast

If a client request should create something visible to other clients, create it on the server:

```rust,ignore
fn handle_fire_request(mut commands: Commands, request: FireRequest) {
    if !request_is_valid(&request) {
        return;
    }

    commands.spawn((
        ProjectileBundle::from_request(request),
        Replicate::to_clients(NetworkTarget::All),
    ));
}
```

That keeps authority simple. The projectile may appear one round trip later unless you add prediction or prespawning, but the replicated entity still comes from the server.

## What about pre-spawned client entities?

Pre-spawning is matching, not general client replication. The client may create a local entity immediately, but the server still creates the authoritative replicated entity. The hard part is matching those two without creating duplicates.

If you need reliability today, start with server-spawned entities and add prediction only where the latency is unacceptable.
