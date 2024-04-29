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

In lightyear, this is enabled by setting a `prediction_target` on the `Replicate` component, which lets you specify
which clients will predict the entity.

If prediction is enabled for an entity, the client will spawn a local copy of the entity along with a marker component called `Predicted`.
The entity that is directly replicated from the server will have a marker component called `Confirmed` (because it is only updated when we receive a new server update packet).

The `Predicted` entity lives on a different timeline than the `Confirmed` entity: it lives a few ticks in the future (at least 1 RTT), enough ticks
so that the client inputs for tick `N` have time to arrive on the server before the server processes tick `N`.

Whenever the player sends an input, we can apply the inputs **instantly** to the `Predicted` entity; which is the only one that we 
show to the player. After roughly 1 RTT, we receive the actual state of the entity from the server, which is used to update the `Confirmed` entity.
If there is a mismatch between the `Confirmed` and `Predicted` entities, we perform a **rollback**: we reset the `Predicted` entity to the state of the `Confirmed` entity,
and re-run all the ticks that happened since the last server update was received. In particular, we will re-apply all the client inputs that were added 
since the last server update.

As the `Confirmed` and `Predicted` entities are 2 separate entities, you will need to specify how the components that are received on the `Confirmed` entity (replicated from the server) are copied to the `Predicted` entity.
To do this, you will need to specify a [ComponentSyncMode](https://docs.rs/lightyear/latest/lightyear/client/components/enum.ComponentSyncMode.html) for each component in the `ComponentProtocol` enum.
There are 3 different modes:
- Full: we apply client-side prediction with rollback
- Simple: the server-updates are copied from Confirmed to Predicted whenever we have an update
- Once: the components are copied only once from the Confirmed entity to the Predicted entity
  
You will need to modify your `ComponentProtocol` to specify the prediction behaviour for each component;
if you don't specify one, the component won't be copied from the `Confirmed` to the `Predicted` entity.


On the server, we will update our entity-spawning system to add client-prediction:
```rust,noplayground
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
        let replicate = Replicate {
          prediction_target: NetworkTarget::Single(client_id),
          ..default()
        };
        let entity = commands.spawn((PlayerBundle::new(client_id, Vec2::ZERO), replicate));
        
        // Add a mapping from client id to entity id
        global.client_id_to_entity_id.insert(client_id, entity.id());
    }
```

The only change is the line `prediction_target: NetworkTarget::Single(client_id)`; we are specifying which clients should be predicting this entity.
Those clients will spawn a copy of the entity with the `Predicted` component added.


Then, on the client, you need to make sure that you also run the same simulation logic as the server, for the `Predicted` entities.
We will add a new system on the client that also applies the user inputs. 
It is very similar to the server system, we also listen for the `InputEvent` event. It also needs to run in the `FixedUpdate` schedule to work correctly.

On the client:
```rust,noplayground
fn player_movement(
    /// Note: we only apply inputs to the `Predicted` entity
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            for position in position_query.iter_mut() {
                shared_movement_behaviour(position, input);
            }
        }
    }
}
app.add_systems(FixedUpdate, movement);
```

Now you can try running the server and client again; you should see 2 cubes for the client; the `Predicted` cube should 
move immediately when you send an input on the client. The `Confirmed` cube only moves when we receive a packet from the server.


## Entity interpolation

Client-side prediction works well for entities that the player predicts, but what about entities that are not controlled by the player?
There are two solutions to make updates smooth for those entities:
- predict them as well, but there might be much bigger mis-predictions because we don't have access to other player's inputs
- display those entities slightly behind the 'Confirmed' entity, and interpolate between the last two confirmed states

The second approach is called 'interpolation', and is the one we will use in this tutorial. You can read this Valve [article](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking#Entity_interpolation) that explains it pretty well.

You will need to modify your `ComponentProtocol` to specify the interpolation behaviour for each component;
if you don't specify one, the component won't be copied from the `Confirmed` to the `Interpolated` entity.
You will also need to provide an interpolation function; or you can use the default linear interpolation.

On the server, we will update our entity-spawning system to add entity-interpolation:
```rust,noplayground
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
        let replicate = Replicate {
          prediction_target: NetworkTarget::Single(client_id),
          interpolation_target: NetworkTarget::AllExceptSingle(client_id),
          ..default()
        };
        let entity = commands.spawn((PlayerBundle::new(client_id, Vec2::ZERO), replicate));
        
        // Add a mapping from client id to entity id
        global.client_id_to_entity_id.insert(client_id, entity.id());
    }
```

This time we interpolate for entities that are not controlled by the player, so we use `NetworkTarget::AllExceptSingle(id)`.

If interpolation is enabled for an entity, the client will spawn a local copy of the entity along with a marker component called `Interpolated`.
The entity that is directly replicated from the server will have a marker component called `Confirmed` (because it is only updated when we receive a new server update packet).

The `Interpolated` entity lives on a different timeline than the `Confirmed` entity: it lives a few ticks in the past.
We want it to live slightly in the past so that we always have at least 2 confirmed states to interpolate between.

And that's it! The `ComponentSyncMode::Full` mode is required on a component to run interpolation.

Now if you run a server and two clients, each player should see the other's player slightly in the past, but with movements that are interpolated smoothly between server updates.



## Conclusion

We have now covered the basics of lightyear, and you should be able to build a server-authoritative multiplayer game
with client-side prediction and entity interpolation!