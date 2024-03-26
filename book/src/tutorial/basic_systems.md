# Adding basic functionality

## Initialization

On the client, we can run this system on `Startup`:

```rust,noplayground
pub(crate) fn init(
    mut commands: Commands,
    mut client: ResMut<Client<MyProtocol>>,
) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Client",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
    client.connect();
}
app.add_systems(Startup, init);
```

This spawns a camera, but also call `client.connect()` which will start the connection process with the server.

On the server we can just spawn a camera:

```rust,noplayground
pub(crate) fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Server",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
}

app.add_systems(Startup, init);
```

## Defining replicated entities

We want to have the following flow:

- client connects to server
- server spawns a player entity for the client
- server then keeps replicating the state of that entity to the client
- client can send inputs to the server to control the player entity (any updates will be replicated back to the client)

We will start by defining what a player entity is.
We create a bundle that contains all the components that we want to replicate. These components must be part of
the `ComponentProtocol` enum that
we defined earlier in order to be replicated.

(In general it is useful to create separate bundles for components that we want to replicate, and components
that are only used on the client or server. For example, on the client a lot of components that are only used for
rendering (particles, etc.)
don't need to be included in the replicated bundle.)

```rust,noplayground
// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
    replicate: Replicate,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
            replicate: Replicate {
                ..default()
            },
        }
    }
}
```

We added an extra special component called `Replicate`. Only entities that have this component will be replicated.
This component also lets us specify some extra parameters for the replication:

- `actions_channel`: which channel to use to replicate entity actions. I define `EntityActions` as entity events that
  need
  exclusive `World` access (entity spawn/despawn, component insertion/removal). By default, an OrderedReliable channel
  is used.
- `updates_channel`: which channel to use to replicate entity updates. I define `EntityUpdates` as entity events that
  don't need
  exclusive `World` access (component updates). By default, an SequencedUnreliable channel is used.
- `replication_target`: who will the entity be replicated to? By default, the entity will be replicated to all clients,
  but you can use this
  to have a more fine-grained control
- `prediction_target`/`interpolation_target`: we will mention this later.

If you remove the `Replicate` component from an entity, any updates to that entity won't be replicated anymore. (However
the client entity won't get despawned)

Currently there is no way to specify for a given entity whether some components should be replicated or not.
This might be added in the future.

## Spawning entities on server

Most networking events are available on both the client and server as `bevy` `Events`, and can be read
every frame using the `EventReader` `SystemParam`. This is what we will use to spawn a player on the server
when a new client gets connected.

```rust,noplayground
#[derive(Resource, Default)]
struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
}
pub(crate) fn handle_connections(
    mut connections: EventReader<ConnectEvent>,
    mut disconnections: EventReader<DisconnectEvent>,
    mut global: ResMut<Global>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.context();
        // Generate pseudo random color from client id.
        let h = (((client_id * 30) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let entity = commands.spawn(PlayerBundle::new(
            *client_id,
            Vec2::ZERO,
            Color::hsl(h, s, l),
        ));
        // Add a mapping from client id to entity id
        global
            .client_id_to_entity_id
            .insert(*client_id, entity.id());
    }
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        if let Some(entity) = global.client_id_to_entity_id.remove(client_id) {
            commands.entity(entity).despawn();
        }
    }
}
app.init_resource::<Global>();
app.add_systems(Update, handle_connections);
```

`ConnectEvent` and `DisconnectEvent` are events that are sent when a client connects/disconnects from the server.
The `context()` method returns the `ClientId` of the client that connected/disconnected.

We also create a map to keep track of which client is associated with which entity.

## Add inputs to client

Then we want to be able to handle inputs from the user.
We add a system that reads keypresses/mouse movements and converts them into `Inputs` that we can give to the `Client`.
Inputs need to be handled with `client.add_input()`, which does some extra bookkeeping to make sure that an input on
tick `n`
for the client will be handled on the server on the same tick `n`. Inputs are also stored in a buffer for
client-prediction.

```rust,noplayground
pub(crate) fn buffer_input(mut client: ResMut<Client<MyProtocol>>, keypress: Res<Input<KeyCode>>) {
    let mut input = Direction {
        up: false,
        down: false,
        left: false,
        right: false,
    };
    if keypress.pressed(KeyCode::W) || keypress.pressed(KeyCode::Up) {
        input.up = true;
    }
    if keypress.pressed(KeyCode::S) || keypress.pressed(KeyCode::Down) {
        input.down = true;
    }
    if keypress.pressed(KeyCode::A) || keypress.pressed(KeyCode::Left) {
        input.left = true;
    }
    if keypress.pressed(KeyCode::D) || keypress.pressed(KeyCode::Right) {
        input.right = true;
    }
    if !direction.is_none() {
        return client.add_input(Inputs::Direction(direction));
    }
    if keypress.pressed(KeyCode::Delete) {
        // currently, inputs is an enum and we can only add one input per tick
        return client.add_input(Inputs::Delete);
    }
    if keypress.pressed(KeyCode::Space) {
        return client.add_input(Inputs::Spawn);
    }
    // always remember to send an input message
    return client.add_input(Inputs::None);
}
app.add_systems(
    FixedUpdate,
    buffer_input.in_set(InputSystemSet::BufferInputs),
);
```

`add_input` needs to be called in the `InputSystemSet::BufferInputs` `SystemSet`.
Some systems need to be run in specific `SystemSet`s because of the ordering of some operations is important for the
crate
to work properly. See [SystemSet Ordering](../concepts/bevy_integration/system_order.md) for more information.

## Handle inputs on server

Once we correctly `add_inputs` on the client, we can start reading them on the server to control the player entity.

We define a function that specifies how a given input updates a given player entity:

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
Similarly, we have an event `InputEvent<I: UserAction>` that will give us on every tick the input that was sent by the
client.
We can use the `context()` method to get the `ClientId` of the client that sent the input,
and `input()` to get the actual input.

```rust,noplayground
pub(crate) fn movement(
    mut position_query: Query<&mut PlayerPosition>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    global: Res<Global>,
    server: Res<Server<MyProtocol>>,
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
app.add_systems(FixedMain, movement);
```

Any fixed-update simulation system (physics, etc.) must run in the `FixedMain` `Schedule` to behave correctly.

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
- the movements of the cube in the server world are replicated to the client !

In the next section, we will see a couple more systems.