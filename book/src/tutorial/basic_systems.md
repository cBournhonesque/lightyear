# Adding basic functionality

What we want to achieve is this:
- when a client connects to the server, the server spawns a player entity for that client
- that entity gets replicated to all clients
- a client can send inputs to move the player entity that corresponds to them

## Replicating an entity

As we saw earlier, the [`Server`] will spawn a new entity whenever a new client connects to it.
That entity will have a [`Link`] component that represents the connection to the client, as well as the [`LinkOf`] component that links it to the [`Server`].

However it is your responsibility to customize that connection with extra components, such as [`ReplicationSender`] or [`MessageManager`], to handle the replication and message sending/receiving.
This can be done using triggers:
```rust,ignore
pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.target()).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::from("Client"),
    ));
}
```

When the [`Link`] is established (`Linked` is added) we are still not connected: we will send a few packets to authenticate the client according to the netcode protocol.
Only after the authentication is successful will the [`Connected`] component be added to the client entity.

When that happens we can start adding game behaviour:
```rust,ignore
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = query.get(trigger.target()) else {
        return;
    };
    let client_id = client_id.0;
    let entity = commands
        .spawn((
            PlayerBundle::new(client_id, Vec2::ZERO),
            // we replicate the Player entity to all clients that are connected to this server
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    info!(
        "Create player entity {:?} for client {:?}",
        entity, client_id
    );
}
```

We do this by listening to `Connected` component being added on the entity.
We can access the id of the client that connected by using the `RemoteId` component, which is added to the entity when the connection is established and contains the client's [`PeerId`]
(the inverse mapping from `PeerId` to `Entity` is stored in the `PeerMetadata` resource)

Finally we are free to spawn an entity for that player, that we can replicate using the `Replicate` component.
On that component you need to specify the `NetworkTarget` to which the entity should be replicated.

That's it! Now all the clients that match that `NetworkTarget` will receive the entity and its components that were added to the protocol.


(you can learn more in the [replicate](../concepts/replication/replicate.md) page)

There are tons of extra components you can added when replicating an entity to control how the replication works.

## Timelines

Ticks are the fundamental unit of time in lightyear, and are used to synchronize the client and server. Ticks are incremented by 1 every time the `FixedMain` schedule runs.
The `LocalTimeline` component on your `Link` entities can be used to retrieve the current tick for that link. You will notice that the server only has the `LocalTimeline` component,
while the client has multiple timelines:
- the `LocalTimeline` component, which is incremented by the `FixedMain` schedule
- the `RemoteTimeline` component, which is the client's view of the server's timeline. This is updated every time the client receives a packet from the server.
- the `InputTimeline` component, which is the timeline where the client buffers inputs. In most cases, this will be identical to the `LocalTimeline`.

lightyear will make sure that your `InputTimeline` and `RemoteTimeline` are always in sync with the server's `LocalTimeline`.


## Handle client inputs

In general it is a good idea (for reasons we will see later) to have a shared function between the client and server that handles the inputs.

If we take our `Inputs` struct from earlier, it can look like this:

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

### Sending inputs

Then we want to be able to handle inputs from the user. Inputs are stored in a component called `ActionState<I>`.

Note that the inputs are tick-synced, which means that your input for tick T is guaranteed to be processed by the server on tick T.
(this is achieved by storing the inputs in a buffer on the server, and processing them only when the correct tick is reached)


We need a system that reads keypresses/mouse movements and converts them into inputs that you will write into the `ActionState<I>` component.
```rust,ignore
pub(crate) fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
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
        // we always set the value. Setting it to None means that the input was missing, it's not the same
        // as saying that the input was 'no keys pressed'
        action_state.value = Some(Inputs::Direction(direction));
    }
}
```

The `InputMarker<I>` component is used to identify the entity that the local client is controlling.
(other clients might replicate to you an entity with the `ActionState<I>` component but no `InputMarker<I>` because it can be useful to have access to their inputs. Since the `InputMarker<I>` is 
not present, your inputs won't modify their `ActionState<I>` component)

On every tick, you can buffer the input for the local client by updating the `ActionState<I>` component. This has to be done in the `FixedPreUpdate` schedule and in the `WriteClientInputs` SystemSet.


### Receiving inputs

On the server, you can simply read the inputs from the `ActionState<I>` component, and apply game logic based on them.
Remember to run this in the `FixedUpdate` schedule, as inputs are tick-synced!

As a rule of thumb, any simulation system (physics, etc.) must run in the `FixedUpdate` `Schedule` to behave correctly.

```rust,ignore
fn movement(
    mut position_query: Query<&mut PlayerPosition, &ActionState<Inputs>>,
) {
    for (position, inputs) in position_query.iter_mut() {
        if let Some(inputs) = &inputs.value {
            shared::shared_movement_behaviour(position, inputs);
        }
    }
}
```


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

In the next section, we will see some more advanced replication techniques.