# Adding basic functionality

What we want to achieve is this:
- when a client connects to the server, the server spawns a player entity for that client
- that entity gets replicated to all clients
- a client can send inputs to move the player entity that corresponds to them

## Initialization

First, we need to start the connection by calling `client.connect()` on the client (via the [ClientConnection](https://docs.rs/lightyear/latest/lightyear/prelude/client/struct.ClientConnection.html) resource) and `server.start()` on the server (via the [ServerConnections](https://docs.rs/lightyear/latest/lightyear/prelude/server/struct.ServerConnections.html) resource).
We do this in a system that runs in the `Startup` schedule.

Client:
```rust,noplayground
fn init(mut client: ResMut<ClientConnection>) {
    client.connect().expect("Failed to connect to server");
}
app.add_systems(Startup, init);
```

Server:
```rust,noplayground
fn init(mut server: ResMut<ServerConnections>) {
    server.start().expect("Failed to start server");
}
app.add_systems(Startup, init);
```

We also do some setup, like adding a `Camera2dBundle` and displaying some text to let us know
if we are the server or the client.

## Network events

The way you can access networking-related events is by using bevy `Events`. `lightyear` exposes a certain number of events which you can see [here](https://docs.rs/lightyear/latest/lightyear/shared/events/components/index.html):
- `ConnectEvent` / `DisconnectEvent`: when a client gets connected or disconnected. This can be used to access the `ClientId` of the client that connected/disconnected.
- `EntitySpawnEvent` / `EntityDespawnEvent`: the **receiver** emits these when it spawns/despawns an entity replicated from the remote world
- `ComponentInsertEvent` / `ComponentRemoveEvent` / `ComponentUpdateEvent`: the **receiver** emits these when it inserts/removes/updates a component for an entity replicated from the remote world
- `InputEvent`: when a user action gets emitted. This event will be emitted on both the server and the client at the exact `Tick` where the input was emitted.
- `MessageEvent`: when a message is received from the remote machine. This is used to access the message contents.

This is what we'll use to spawn an entity on the server whenever a client connects!

```rust,noplayground
/// We will maintain a mapping from client id to entity id
// so that we know which entity to despawn when the client disconnects
#[derive(Resource)]
pub(crate) struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
}

/// Create a player entity whenever a client connects
pub(crate) fn handle_connections(
    /// Here we listen for the `ConnectEvent` event
    mut connections: EventReader<ConnectEvent>,
    mut global: ResMut<Global>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        /// on the server, the `context()` method returns the `ClientId` of the client that connected
        let client_id = *connection.context();
        
        /// We add the `Replicate` component to start replicating the entity to clients
        /// By default, the entity will be replicated to all clients
        let replicate = Replicate::default(); 
        let entity = commands.spawn((PlayerBundle::new(client_id, Vec2::ZERO), replicate));
        
        // Add a mapping from client id to entity id
        global.client_id_to_entity_id.insert(client_id, entity.id());
    }
```

As you can see above, starting replicating an entity is very easy: you just need to add the `Replicate` component to the entity
and it will start getting replicated.

(you can learn more in the [replicate](./concepts/replication/replicate.md) page)


If you remove the `Replicate` component from an entity, any updates to that entity won't be replicated anymore.
(However the client entity won't get despawned)


## Handle client inputs

Then we want to be able to handle inputs from the user.
We need a system that reads keypresses/mouse movements and converts them into `Inputs` (from our `Protocol`).
You will need to call the [add_input](https://docs.rs/lightyear/latest/lightyear/client/input/struct.InputManager.html#method.add_input) method on the `InputManager` resource to send an input to the server.

There are some ordering constraints for inputs: you need to make sure that inputs are handled in the `BufferInputs` `SystemSet`, which runs in the `FixedPreUpdate` schedule.
Then lightyear will make sure that the server will handle the input on the same tick as the client.

On the client:
```rust,noplayground
pub(crate) fn buffer_input(
    /// You will need to specify the exact tick at which the input was emitted. You can use 
    /// the `TickManager` to retrieve the current tick
    tick_manager: Res<TickManager>,
    /// You will use the `InputManager` to send an input
    mut input_manager: ResMut<InputManager<Inputs>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    let tick = tick_manager.tick();
    let mut input = Inputs::None;
    let mut direction = Direction {
        up: false,
        down: false,
        left: false,
        right: false,
    };
    if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
        direction.up = true;
    }
    if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
        direction.down = true;
    }
    if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
        direction.left = true;
    }
    if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
        direction.right = true;
    }
    if !direction.is_none() {
        input = Inputs::Direction(direction);
    }
    input_manager.add_input(input, tick)
}
app.add_systems(FixedPreUpdate, buffer_input.in_set(InputSystemSet::BufferInputs));
```

Then, on the server, you will want to listen to the `InputEvent` event, in the `FixedUpdate` schedule,
to move the client entity. Any changes to the entity will be replicated to all clients.

We define a function that specifies how a given input updates a given player entity. It is a good idea to define
the simulation logic in a function that can be shared between the client and the server, in case we want to run 
the simulation ahead of time on the client (this is called client-prediction).

```rust,noplayground 
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: &Inputs) {
    const MOVE_SPEED: f32 = 10.0;
    match input {
        Inputs::Direction(direction) => {
            if direction.up {
                position.y += MOVE_SPEED;
            }
            if direction.down {
                position.y -= MOVE_SPEED;
            }
            if direction.left {
                position.x -= MOVE_SPEED;
            }
            if direction.right {
                position.x += MOVE_SPEED;
            }
        }
        _ => {}
    }
}
```

Then we can create a system that reads the inputs and applies them to the player entity.
On the server:
```rust,noplayground
fn movement(
    mut position_query: Query<&mut PlayerPosition>,
    /// Event that will contain the inputs for the correct tick
    mut input_reader: EventReader<InputEvent<Inputs>>,
    /// Retrieve the entity associated with a given client
    global: Res<Global>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            if let Some(player_entity) = global.client_id_to_entity_id.get(client_id) {
                if let Ok(position) = position_query.get_mut(*player_entity) {
                    shared_movement_behaviour(position, input);
                }
            }
        }
    }
}
app.add_systems(FixedUpdate, movement);
```

Any fixed-update simulation system (physics, etc.) must run in the `FixedUpdate` `Schedule` to behave correctly.


## Displaying entities

Finally we can add a system on both client and server to draw a box to show the player entity.

```rust,noplayground
pub(crate) fn draw_boxes(
    mut gizmos: Gizmos,
    players: Query<(&PlayerPosition, &PlayerColor)>,
) {
    for (position, color) in &players {
        gizmos.rect(
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}
```


Now, running the server and client in parallel should give you:
- server spawns a cube when client connects
- client can send inputs to the server to control the cube
- the movements of the cube in the server world are replicated to the client (and to other clients) !

In the next section, we will see a couple more systems.