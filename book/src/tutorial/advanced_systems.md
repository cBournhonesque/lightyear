# Advanced systems

In this section we will see how we can add client-prediction and entity-interpolation to make the game feel more responsive and smooth.

## Client prediction

If we wait for the server to:
- receive the client input
- move the player entity
- replicate the update back to the client

We will have a delay of at least 1 RTT (round-trip-delay) before we see the impact of our inputs on the player entity.
This can feel very sluggish/laggy, which is why often games will use [client-side prediction](https://www.gabrielgambetta.com/client-side-prediction-server-reconciliation.html).
Another issue is that the entity on the client will only be updated whenever we receive a packet from the server. Usually the packet send rate is much lower than one packet per frame,
for example it can be on the order of 10 packet per second. If the server's packet_send_rate is low, the entity will appear to stutter.

The solution is to run the same simulation systems on the client as on the server, but only for the entities that the client predicts.
This is "client-prediction": we move the client-controlled entity immediately according to our user inputs, and then we correct the position
when we receive the actual state of the entity from the server. (if there is a mismatch)

To do this in lightyear, you will need to change a few things:

- enable prediction for the component in your protocol;
- enable the application's global prediction pipeline with a `PredictionManager` resource;
- mark the entity as predicted, either with `PredictionTarget` on the send-side or by adding
  `Predicted` on the receive-side.

First, in your protocol, mark the component as predicted:

```rust,ignore
app.component::<PlayerPosition>().replicate()
    .predict();
```

Then, enable the application's prediction pipeline. `ClientPlugins` installs the prediction systems, but the systems only run while the application has a `PredictionManager` resource. The shared example helpers enable it automatically; otherwise insert the resource during application setup:

```rust,ignore
app.insert_resource(PredictionManager::default());
```

Finally, choose which replicated entities should be predicted. If the sender knows which clients
should predict the entity, add a `PredictionTarget` component on the send-side. Usually, the client
that 'controls' the entity will be predicting it.
```rust,ignore
let entity = commands
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        ))
        .id();
```

If the receiver decides locally that an entity should be predicted, add `Predicted` on the
receive-side instead:

```rust,ignore
commands.entity(replicated_entity).insert(Predicted);
```

Once prediction is enabled for an entity, the receive-side entity gets a `Predicted` marker. There is only one entity: its live components hold the predicted values, and each predicted component also gets two history buffers on the same entity — `ConfirmedHistory<C>` (authoritative states received from the server) and `PredictionHistory<C>` (what the client simulated). In most cases you just query the live component value.

The predicted entity lives a few ticks in the future (at least 1 RTT), enough ticks
so that the client inputs for tick `N` have time to arrive on the server before the server processes tick `N`.

Whenever the player sends an input, we can apply the inputs **instantly** to the predicted entity; which is the one that we
show to the player. After roughly 1 RTT, we receive the actual state of the entity from the server, which lands in the `ConfirmedHistory`.
If it mismatches what we predicted for that tick, we perform a **rollback**: we reset the entity to the confirmed state,
and re-run all the ticks that happened since the last server update was received. In particular, we will re-apply all the client inputs that were added
since the last server update.


Then, on the client, you need to make sure that you also run the same simulation logic as the server, for the `Predicted` entities. This is very important!
The client must be 'predicting' what the entity will do even though it doesn't have perfect information because it doesn't know the inputs of other players.
Most of the time the prediction will be correct, and we successfully erased the lag between the user input and the entity movement.
Sometimes the prediction will be wrong, in which case lightyear will trigger a rollback and re-run the simulation since the tick that was wrong.


We will add a new system on the client that also applies the user inputs. 
It is very similar to the server system, we also listen for the `InputEvent` event. It also needs to run in the `FixedUpdate` schedule to work correctly.

On the client:
```rust,noplayground
fn player_movement(
    mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
) {
    for (position, input) in position_query.iter_mut() {
        shared::shared_movement_behaviour(position, input);
    }
}
app.add_systems(FixedUpdate, player_movement);
```

Now you can see why it's a good idea to use shared logic between the client and server for the movement system: by using a shared function, we can ensure
that the client and server will run the same logic for the player movement, which is crucial for client-side prediction to work correctly.

Now you can try running the server and client again. The predicted cube should
move immediately when you send an input on the client, with no waiting for the server round-trip. (In `simple_box` the client also renders remote players as interpolated copies, so with two clients you'll see your own player predicted and the other player interpolated.)


## Snapshot interpolation

Client-side prediction works well for entities that the player predicts, but what about entities that are not controlled by the player?
There are two solutions to make updates smooth for those entities:
- predict them as well, but there might be much bigger mis-predictions because we don't have access to other player's inputs
- display those entities slightly behind the 'Confirmed' entity, and interpolate between the last two confirmed states

The second approach is called 'interpolation', and is the one we will use in this tutorial. You can read this Valve [article](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking#Entity_interpolation) that explains it pretty well.

To do this, there are again two places to update:

In your protocol, register an interpolation function for the component, which specifies how to interpolate between two states:
```rust,ignore
pub type LerpFn<C> = fn(start: C, other: C, t: f32) -> C;
```
If your type implements the `Ease` trait from bevy, you can also call `add_linear_interpolation`. This is what we will do here.

```rust,ignore
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct PlayerPosition(pub Vec2);

impl Ease for PlayerPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            PlayerPosition(Vec2::lerp(start.0, end.0, t))
        })
    }
}

app.component::<PlayerPosition>().replicate()
    .predict()
    .add_linear_interpolation();
```
(In `simple_box` the component has both `.predict()` and `.add_linear_interpolation()`: the owning client predicts it, everyone else interpolates it.)

Then, when replicating the entity, you can also specify which clients should predict the entity by adding a `InterpolationTarget` component.
Usually, the clients that don't control an entity will be interpolating it.
```rust,ignore
let entity = commands
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ))
        .id();
```


If interpolation is enabled for an entity, it gets an `Interpolated` marker. Same deal as prediction: one entity, whose live component holds the interpolated value while a `ConfirmedHistory<C>` on the same entity buffers the authoritative snapshots. Every frame the live value is re-sampled by blending between the last two confirmed states.

The interpolated entity is sampled a few ticks in the past.
We want it to live slightly in the past so that we always have at least 2 confirmed states to interpolate between.

Now if you run a server and two clients, each player should see the other's player slightly in the past, but with movements that are interpolated smoothly between server updates.



## Conclusion

We have now covered the basics of lightyear, and you should be able to build a server-authoritative multiplayer game
with client-side prediction and entity interpolation!
