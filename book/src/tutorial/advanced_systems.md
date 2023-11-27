# Advanced systems

## Messages

You can send messages in both directions by using the `buffer_send` method on the `Client` or `Server` resources.

On the receiving side, you can use the `EventReader<MessageEvents<M>>` `SystemParam` to read the messages that arrived since the last frame.


## Client prediction

If we wait for the server to:
- receive the client input
- move the player entity
- replicate the update back to the client

We will have a delay of at least 1 RTT (round-trip-delay) before we see the impact of our inputs on the player entity.
This can feel very sluggish/laggy, which is why often games will use [client-side prediction](https://www.gabrielgambetta.com/client-side-prediction-server-reconciliation.html).
Another issue is is that the entity on the client will only be updated whenever we receive a packet. If the server's packet_send_rate is low,
the entity will appear to stutter.


In lightyear, this is enabled by setting a `prediction_target` on the `Replicate` component, which lets you specify
which clients will predict the entity.

If prediction is enabled for a client, the client will spawn a local copy of the entity along with a marker component called `Predicted`.
The entity that is directly replicated from the server will have a marker component called `Confirmed` (because it is only updated when we receive a new server update packet).

The `Predicted` entity lives on a different timeline than the `Confirmed` entity: it lives a few ticks in the future (at least 1 RTT), enough ticks
so that the client inputs for tick `N` have time to arrive on the server before the server processes tick `N`.

Whenever the player sends an input, we can apply the inputs **instantly** to the `Predicted` entity; which is the only one that we 
show to the player. After roughly 1 RTT, we receive the actual state of the entity from the server, which is used to update the `Confirmed` entity.
If there is a mismatch between the `Confirmed` and `Predicted` entities, we perform a **rollback**: we reset the `Predicted` entity to the state of the `Confirmed` entity,
and re-run all the ticks that happened since the last server update was received. In particular, we will re-apply all the client inputs that were added 
since the last server update.

Let us enable prediction for the entity that is controlled by the player. We have to first modify our `Replicate` component:
```rust,noplayground
impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
            replicate: Replicate {
                // Enable prediction!
                prediction_target: NetworkTarget::Only(id),
                ..default()
            },
        }
    }
}
```

Then we will apply on the client the same simulation systems as on the server, but only for the `Predicted` entities:
```rust,noplayground
pub(crate) fn movement(
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    if PlayerPosition::mode() != ComponentSyncMode::Full {
        return;
    }
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            for mut position in position_query.iter_mut() {
                shared_movement_behaviour(&mut position, input);
            }
        }
    }
}
app.add_systems(FixedUpdate, movement.in_set(FixedUpdateSet::Main));
```
Don't forget that fixed-timestep simulation systems must run in the `FixedUpdateSet::Main` `SystemSet`.

You might be wondering what the `ComponentSyncMode` is.
This crate specifies 3 different modes for synchronizing components between the `Confirmed` and `Predicted` entities:
- Full: we apply client-side prediction with rollback
- Simple: the server-updates are copied from `Confirmed` to `Predicted` whenever we have an update
- Once: the initial components from `Confirmed` are copied to `Predicted`, but after that we never update the component on `Predicted` again

Let's modify our protocol accordingly:
```rust,noplayground
#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    #[sync(once)]
    PlayerId(PlayerId),
    #[sync(full)]
    PlayerPosition(PlayerPosition),
    #[sync(once)]
    PlayerColor(PlayerColor),
}
```

The `PlayedId` never changes to we can set it to `once` as an optimization.
We want to modify the `PlayerColor` on the `Predicted` entity so that we can distinguish `Predicted` vs `Confirmed`, so we need
to set the mode to `once`.
For the `PlayerPosition`, we want to apply client-side prediction, so we set the mode to `full`.

And here is the system to change the color of the `Predicted` entity:
```rust,noplayground
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        color.0.set_s(0.2);
    }
}
```

Now you can try running the server and client again; you should see 2 cubes for the client; the `Predicted` cube should 
move immediately when you send an input on the client.


## Entity interpolation

Client-side prediction works well for entities that we predict







